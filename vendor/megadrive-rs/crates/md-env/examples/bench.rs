//! Speed of the environment and a look at what it reads, with a simple agent.
//!
//! ```text
//! cargo run -p md-env --release --example bench -- ROM [--state FILE]
//!     [--steps N] [--frame-skip N] [--watch N] [--seed N]
//!     [--policy random|right|hop] [--threads N] [--fm-synthesis]
//!     [--check-silent N] [--find-early died|completed] [--still N]
//!     [--trace ADDR,...] [--jump-penalty P]
//! ```
//! `--state` is a save state of the same ROM (the frontend's `.stateN`
//! files): every episode begins there. `--watch N` prints Sonic 1's
//! variables every N steps, to compare with what the game shows (lives,
//! rings, X growing when going right). Sonic 1 only for now.
//!
//! `--policy` picks who plays: `random` (default), `right` (holds right) or
//! `hop` (right, jumping every other step). From a state near the end of a
//! level, `right` shows what finishing it looks like in the variables.
//!
//! `--threads N` runs N environments at once, one per thread, each playing
//! `--steps` steps: the total speed is what parallel training can expect
//! (the first thread alone prints `--watch`).
//!
//! The YM2612 sound synthesis is off, as in training; `--fm-synthesis`
//! turns it on to compare the speed. `--check-silent N` plays the same N
//! random actions with and without it and checks that the game sees no
//! difference (work RAM, Z80 RAM, video memories, picture at every step).
//!
//! `--find-early died|completed` plays until the first episode that ends
//! that way, keeping a copy of the work RAM per step, then lists the bytes
//! that stayed still and changed once or twice late in the episode, earliest
//! first: candidates for an earlier signal than lives or act (md_env::ramscan).
//! A candidate must have stayed still at least `--still` steps (10) first:
//! what the game sets up as an episode begins is left out.
//! `--trace FFD024,FFF7D6` prints those bytes at every step.

use std::time::Instant;

use md_env::sonic1::SonicState;
use md_env::{End, Env, EnvConfig, Game, Sonic1};

struct Args {
    rom: String,
    state: Option<String>,
    steps: u64,
    frame_skip: u32,
    watch: u64,
    seed: u64,
    policy: Policy,
    threads: usize,
    fm_synthesis: bool,
    jump_penalty: f32,
    check_silent: u64,
    find_early: Option<End>,
    trace: Vec<u32>,
    still: usize,
}

#[derive(Clone, Copy)]
enum Policy {
    Random,
    Right,
    Hop,
}

/// Indices into `Sonic1::ACTIONS`.
const ACTION_RIGHT: usize = 1;
const ACTION_RIGHT_JUMP: usize = 2;

fn parse() -> Result<Args, String> {
    let mut args = Args {
        rom: String::new(),
        state: None,
        steps: 10_000,
        frame_skip: 4,
        watch: 0,
        seed: 1,
        policy: Policy::Random,
        threads: 1,
        fm_synthesis: false,
        jump_penalty: 0.0,
        check_silent: 0,
        find_early: None,
        trace: Vec::new(),
        still: 10,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        let number = |text: String, name: &str| {
            text.parse::<u64>()
                .map_err(|_| format!("{name}: not a number: {text}"))
        };
        match arg.as_str() {
            "--state" => args.state = Some(value("--state")?),
            "--steps" => args.steps = number(value("--steps")?, "--steps")?,
            "--frame-skip" => {
                args.frame_skip = number(value("--frame-skip")?, "--frame-skip")? as u32
            }
            "--watch" => args.watch = number(value("--watch")?, "--watch")?,
            "--policy" => {
                args.policy = match value("--policy")?.as_str() {
                    "random" => Policy::Random,
                    "right" => Policy::Right,
                    "hop" => Policy::Hop,
                    other => return Err(format!("--policy: unknown policy {other}")),
                }
            }
            "--fm-synthesis" => args.fm_synthesis = true,
            "--jump-penalty" => {
                let text = value("--jump-penalty")?;
                args.jump_penalty = match text.parse::<f32>() {
                    Ok(p) if p.is_finite() && p >= 0.0 => p,
                    _ => return Err(format!("--jump-penalty: 0 or more, not {text}")),
                }
            }
            "--find-early" => {
                args.find_early = Some(match value("--find-early")?.as_str() {
                    "died" => End::Died,
                    "completed" => End::Completed,
                    other => return Err(format!("--find-early: died or completed, not {other}")),
                })
            }
            "--still" => args.still = number(value("--still")?, "--still")? as usize,
            "--trace" => {
                args.trace = value("--trace")?
                    .split(',')
                    .map(|a| {
                        let hex = a.trim().trim_start_matches("0x").trim_start_matches("0X");
                        u32::from_str_radix(hex, 16)
                            .map_err(|_| format!("--trace: not an address: {a}"))
                    })
                    .collect::<Result<_, _>>()?
            }
            "--check-silent" => {
                args.check_silent = number(value("--check-silent")?, "--check-silent")?
            }
            "--threads" => args.threads = number(value("--threads")?, "--threads")? as usize,
            "--seed" => args.seed = number(value("--seed")?, "--seed")?.max(1),
            _ if args.rom.is_empty() && !arg.starts_with("--") => args.rom = arg,
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    if args.rom.is_empty() {
        return Err(
            "usage: bench ROM [--state FILE] [--steps N] [--frame-skip N] [--watch N] [--seed N] [--policy random|right|hop] [--threads N] [--fm-synthesis] [--check-silent N] [--find-early died|completed] [--still N] [--trace ADDR,...] [--jump-penalty P]"
                .into(),
        );
    }
    Ok(args)
}

/// xorshift64: a random action without a dependency.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn show(step: u64, s: &SonicState) {
    println!(
        "step {step:>7}  x {:>5} y {:>4}  speed {:>6},{:>6}  camera {:>5}  limit {:>5}  rings {:>3}  lives {}  zone {} act {}  score {}  ground {:>6} angle {:>3} air {} jump {} held {} danger {}",
        s.x,
        s.y,
        s.speed_x,
        s.speed_y,
        s.camera_x,
        s.limit_right,
        s.rings,
        s.lives,
        s.zone,
        s.act,
        s.score,
        s.inertia,
        s.angle,
        u8::from(s.status & md_env::sonic1::STATUS_IN_AIR != 0),
        s.jumping,
        u8::from(s.joypad_held & md_env::sonic1::BUTTONS_JUMP != 0),
        match s.dangers[0] {
            Some(d) => format!(
                "{}{:+},{:+}",
                if d.badnik { "badnik" } else { "hurt" },
                d.dx,
                d.dy
            ),
            None => "-".into(),
        }
    );
}

/// What one agent did.
#[derive(Default, Clone, Copy)]
struct Tally {
    reward: f32,
    episodes: u64,
    max_x: u16,
    /// Jumps taken (the jump flag going up).
    jumps: u64,
}

/// Plays `args.steps` steps on its own environment (the watch output only
/// when `watch` is set: one thread prints).
fn play(
    args: &Args,
    rom: &[u8],
    state: Option<&[u8]>,
    seed: u64,
    watch: bool,
) -> Result<Tally, String> {
    let config = EnvConfig {
        frame_skip: args.frame_skip,
        fm_synthesis: args.fm_synthesis,
        ..EnvConfig::default()
    };
    let game = Sonic1 {
        jump_penalty: args.jump_penalty,
        ..Sonic1::default()
    };
    let mut env = Env::new(rom, game, config).map_err(|e| e.to_string())?;
    if let Some(state) = state {
        env.set_start_state(state.to_vec())
            .map_err(|e| e.to_string())?;
    }
    let watch_thread = watch;
    let watch = if watch { args.watch } else { 0 };
    let mut gray = vec![0u8; env.gray_size()];
    let mut seed = seed;
    let mut tally = Tally::default();
    env.reset();
    if watch > 0 {
        show(0, &env.state());
    }
    for step in 1..=args.steps {
        let action = match args.policy {
            Policy::Random => (next(&mut seed) % Sonic1::ACTIONS.len() as u64) as usize,
            Policy::Right => ACTION_RIGHT,
            Policy::Hop if step % 2 == 0 => ACTION_RIGHT_JUMP,
            Policy::Hop => ACTION_RIGHT,
        };
        let before = env.state().jumping;
        let result = env.step(action);
        if before == 0 && env.state().jumping != 0 {
            tally.jumps += 1;
        }
        env.observe_gray(&mut gray);
        tally.reward += result.reward;
        tally.max_x = tally.max_x.max(env.state().x);
        if watch > 0 && step % watch == 0 {
            show(step, &env.state());
        }
        if !args.trace.is_empty() && watch_thread {
            trace(step, &env, &args.trace);
        }
        if result.done() {
            tally.episodes += 1;
            if watch > 0 {
                println!("step {step:>7}  episode over: {:?}", result.end);
                show(step, &env.state());
            }
            env.reset();
        }
    }
    Ok(tally)
}

/// Prints the bytes at `addrs` (68000 addresses of work RAM).
fn trace(step: u64, env: &Env<Sonic1>, addrs: &[u32]) {
    let ram = env.console().bus().work_ram();
    let values: Vec<String> = addrs
        .iter()
        .map(|&a| format!("{:06X}={:02X}", a | 0xFF_0000, ram[(a & 0xFFFF) as usize]))
        .collect();
    println!("step {step:>7}  {}", values.join("  "));
}

/// Copies of the work RAM kept per step by `--find-early`.
const FIND_WINDOW: usize = 400;
/// Candidates shown by `--find-early`.
const FIND_SHOWN: usize = 30;

/// Plays until the first episode ending as `kind`, then lists the bytes
/// that changed late in it (md_env::ramscan::late_changes).
fn find_early(
    args: &Args,
    rom: &[u8],
    state: Option<&[u8]>,
    kind: End,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = EnvConfig {
        frame_skip: args.frame_skip,
        ..EnvConfig::default()
    };
    let mut env = Env::new(rom, Sonic1::default(), config)?;
    if let Some(state) = state {
        env.set_start_state(state.to_vec())?;
    }
    let mut seed = args.seed;
    let mut snapshots: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
    env.reset();
    snapshots.push_back(env.console().bus().work_ram().to_vec());
    let mut episode_step = 0u64;
    for step in 1..=args.steps {
        let action = match args.policy {
            Policy::Random => (next(&mut seed) % Sonic1::ACTIONS.len() as u64) as usize,
            Policy::Right => ACTION_RIGHT,
            Policy::Hop if step % 2 == 0 => ACTION_RIGHT_JUMP,
            Policy::Hop => ACTION_RIGHT,
        };
        let result = env.step(action);
        episode_step += 1;
        if snapshots.len() == FIND_WINDOW {
            snapshots.pop_front();
        }
        snapshots.push_back(env.console().bus().work_ram().to_vec());
        if !result.done() {
            continue;
        }
        if result.end == Some(kind) {
            let first = episode_step + 1 - snapshots.len() as u64;
            let copies: Vec<&Vec<u8>> = snapshots.iter().collect();
            let found = md_env::ramscan::late_changes(&copies, args.still.max(1), 2);
            println!(
                "episode ended {kind:?} at episode step {episode_step} (current detection); RAM kept for steps {first}..{episode_step}"
            );
            println!(
                "bytes still for at least {} steps, then changed once or twice, earliest first ({} found):",
                args.still.max(1),
                found.len()
            );
            for c in found.iter().take(FIND_SHOWN) {
                println!(
                    "  {:06X}  step {:>5}  {:02X} -> {:02X}  ({} change{})",
                    c.addr,
                    first + c.step as u64,
                    c.before,
                    c.after,
                    c.changes,
                    if c.changes > 1 { "s" } else { "" }
                );
            }
            return Ok(());
        }
        env.reset();
        snapshots.clear();
        snapshots.push_back(env.console().bus().work_ram().to_vec());
        episode_step = 0;
    }
    println!("no episode ended {kind:?} within {} steps", args.steps);
    Ok(())
}

/// What the game can see or change, to compare two runs.
fn visible(env: &Env<Sonic1>) -> (Vec<u8>, u64) {
    let console = env.console();
    let bus = console.bus();
    let mut bytes = bus.work_ram().to_vec();
    bytes.extend_from_slice(bus.z80_space().ram());
    bytes.extend_from_slice(bus.vdp().vram());
    bytes.extend(bus.vdp().cram().iter().flat_map(|w| w.to_be_bytes()));
    bytes.extend(bus.vdp().vsram().iter().flat_map(|w| w.to_be_bytes()));
    (bytes, console.frame_hash())
}

/// Plays the same random actions with and without the FM synthesis and
/// compares what the game sees at every step.
fn check_silent(
    args: &Args,
    rom: &[u8],
    state: Option<&[u8]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let make = |fm_synthesis| -> Result<Env<Sonic1>, Box<dyn std::error::Error>> {
        let config = EnvConfig {
            frame_skip: args.frame_skip,
            fm_synthesis,
            ..EnvConfig::default()
        };
        let mut env = Env::new(rom, Sonic1::default(), config)?;
        if let Some(state) = state {
            env.set_start_state(state.to_vec())?;
        }
        env.reset();
        Ok(env)
    };
    let (mut full, mut silent) = (make(true)?, make(false)?);
    let mut seed = args.seed;
    let mut episodes = 0u64;
    for step in 1..=args.check_silent {
        let action = (next(&mut seed) % Sonic1::ACTIONS.len() as u64) as usize;
        let (a, b) = (full.step(action), silent.step(action));
        if a != b || visible(&full) != visible(&silent) {
            println!("check-silent: DIFFERENT at step {step} (action {action})");
            return Ok(());
        }
        if a.done() {
            episodes += 1;
            full.reset();
            silent.reset();
        }
    }
    println!(
        "check-silent: identical over {} steps ({episodes} episodes): the game sees no difference without FM synthesis",
        args.check_silent
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse()?;
    let rom = std::fs::read(&args.rom)?;
    let state = args.state.as_ref().map(std::fs::read).transpose()?;
    if args.check_silent > 0 {
        return check_silent(&args, &rom, state.as_deref());
    }
    if let Some(kind) = args.find_early {
        return find_early(&args, &rom, state.as_deref(), kind);
    }
    let threads = args.threads.max(1);
    let begin = Instant::now();
    // One environment per thread, each with its own random sequence.
    let tallies: Vec<Result<Tally, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let (args, rom, state) = (&args, &rom, state.as_deref());
                let seed = args.seed.wrapping_add(i as u64 * 0x9E37_79B9).max(1);
                scope.spawn(move || play(args, rom, state, seed, i == 0))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("agent thread"))
            .collect()
    });
    let seconds = begin.elapsed().as_secs_f64();
    let tallies = tallies.into_iter().collect::<Result<Vec<_>, _>>()?;
    let steps = args.steps * threads as u64;
    let frames = steps * u64::from(args.frame_skip);
    println!(
        "FM synthesis {}",
        if args.fm_synthesis { "on" } else { "off" }
    );
    println!(
        "{steps} steps ({frames} frames, {threads} thread{}) in {seconds:.2} s: {:.0} steps/s, {:.0} frames/s ({:.1}x real time), {:.0} steps/s per thread",
        if threads > 1 { "s" } else { "" },
        steps as f64 / seconds,
        frames as f64 / seconds,
        frames as f64 / seconds / 60.0,
        steps as f64 / seconds / threads as f64,
    );
    let reward: f32 = tallies.iter().map(|t| t.reward).sum();
    let episodes: u64 = tallies.iter().map(|t| t.episodes).sum();
    let max_x = tallies.iter().map(|t| t.max_x).max().unwrap_or(0);
    let jumps: u64 = tallies.iter().map(|t| t.jumps).sum();
    println!("agent: reward {reward:.2}, {episodes} episodes, furthest x {max_x}, {jumps} jumps");
    Ok(())
}

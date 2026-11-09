//! The AI player: a trained model (python/agent_server.py, Stable-Baselines3)
//! chooses the buttons of pad 1 while Sonic 1 plays in the window. As in
//! training (crates/md-env), it decides every 4 frames from what it sees
//! (the numbers read in RAM, or the grey picture), and its episodes loop:
//! after a death, the end of the level or 300 steps without a new best X,
//! the game goes back to where the AI took over.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, bail};
use md_core::{Buttons, Console};
use md_env::frame::{GRAY_SIDE, GRAY_SIZE, gray_downscale};
use md_env::sonic1::SonicState;
use md_env::{End, FeatureSet, Game, Sonic1};

/// Frames an action is held, as in training (`EnvConfig::frame_skip`).
pub const FRAME_SKIP: u32 = 4;
/// Steps without a new best X before the episode starts over, as in
/// training (`train_ppo.py --stall-steps`).
pub const STALL_STEPS: u64 = 300;

/// The repository (python/, .venv/), from where this crate was built.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Python of the training packages: $MDENV_PYTHON, else the
/// repository's .venv.
pub fn default_python() -> PathBuf {
    std::env::var_os("MDENV_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join(".venv/bin/python"))
}

/// The model's server: $MDENV_AGENT_SERVER (tests use a stand-in), else
/// python/agent_server.py.
pub fn server_script() -> PathBuf {
    std::env::var_os("MDENV_AGENT_SERVER")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("python/agent_server.py"))
}

/// What the model looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// `Sonic1::features`, the basic or the extended set.
    Ram(FeatureSet),
    /// The picture, `GRAY_SIDE`² grey levels.
    Image,
}

/// The model's process, spoken to line by line (agent_server.py protocol).
pub struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pub observation: Observation,
    pub actions: usize,
    line: String,
}

impl AgentProcess {
    /// Starts `python agent_server.py model`.
    pub fn spawn(python: &Path, model: &Path, deterministic: bool) -> anyhow::Result<Self> {
        let mut command = Command::new(python);
        command.arg(server_script()).arg(model);
        if deterministic {
            command.arg("--deterministic");
        }
        Self::start(command).with_context(|| {
            format!(
                "starting the AI ({} {}): is the training environment installed (.venv)?",
                python.display(),
                server_script().display()
            )
        })
    }

    /// Runs `command` and reads its `ready` line.
    pub fn start(mut command: Command) -> anyhow::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        let mut agent = AgentProcess {
            child,
            stdin,
            stdout,
            observation: Observation::Ram(FeatureSet::Basic),
            actions: 0,
            line: String::new(),
        };
        let ready = agent.read_line()?;
        let words: Vec<&str> = ready.split_whitespace().collect();
        let (observation, size, actions) = match words[..] {
            ["ready", kind, size, actions] => (
                kind,
                size.parse::<usize>().ok(),
                actions.parse::<usize>().ok(),
            ),
            _ => bail!("unexpected greeting from the AI: {ready:?}"),
        };
        agent.observation = match (observation, size) {
            ("ram", Some(n)) if n == FeatureSet::Basic.count() => {
                Observation::Ram(FeatureSet::Basic)
            }
            ("ram", Some(n)) if n == FeatureSet::Extended.count() => {
                Observation::Ram(FeatureSet::Extended)
            }
            ("ram", Some(n)) if n == FeatureSet::Full.count() => Observation::Ram(FeatureSet::Full),
            ("ram", Some(n)) if n == FeatureSet::Objects.count() => {
                Observation::Ram(FeatureSet::Objects)
            }
            ("ram", Some(n)) if n == FeatureSet::Hazards.count() => {
                Observation::Ram(FeatureSet::Hazards)
            }
            ("image", Some(n)) if n == GRAY_SIZE => Observation::Image,
            _ => bail!("the AI looks at {observation} {size:?}: not what this emulator gives"),
        };
        agent.actions = match actions {
            Some(n) if n == Sonic1::ACTIONS.len() => n,
            _ => bail!(
                "the AI has {actions:?} actions, Sonic 1 has {}",
                Sonic1::ACTIONS.len()
            ),
        };
        Ok(agent)
    }

    fn read_line(&mut self) -> anyhow::Result<String> {
        self.line.clear();
        if self.stdout.read_line(&mut self.line)? == 0 {
            let status = self.child.try_wait().ok().flatten();
            bail!(
                "the AI stopped ({})",
                status.map_or("running".into(), |s| s.to_string())
            );
        }
        Ok(self.line.trim().to_string())
    }

    /// Asks for an action, `request` being the `obs ...` line.
    fn ask(&mut self, request: &str) -> anyhow::Result<usize> {
        writeln!(self.stdin, "{request}").context("the AI stopped listening")?;
        self.stdin.flush().context("the AI stopped listening")?;
        let answer = self.read_line()?;
        match answer.split_whitespace().collect::<Vec<_>>()[..] {
            ["action", n] => match n.parse::<usize>() {
                Ok(n) if n < self.actions => Ok(n),
                _ => bail!("the AI chose an unknown action: {answer:?}"),
            },
            _ => bail!("the AI answered {answer:?}"),
        }
    }

    pub fn decide_ram(&mut self, features: &[f32]) -> anyhow::Result<usize> {
        let numbers: Vec<String> = features.iter().map(|v| v.to_string()).collect();
        self.ask(&format!("obs {}", numbers.join(" ")))
    }

    pub fn decide_image(&mut self, gray: &[u8]) -> anyhow::Result<usize> {
        self.ask(&format!("obs {}", base64(gray)))
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How an episode of the AI ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Died,
    Finished,
    /// No new best X for `STALL_STEPS` steps.
    Stuck,
    /// With `continue_acts`, after a finished act: the next act has begun
    /// (not an episode end: the game goes on from here).
    NextAct,
}

/// The AI at the controls: decisions, episodes, figures for the status line.
pub struct AgentPlayer {
    process: AgentProcess,
    game: Sonic1,
    stall_steps: u64,
    /// Frames left before the next decision.
    frames_left: u32,
    held: Buttons,
    start: SonicState,
    best_x: i64,
    stalled: u64,
    features: Vec<f32>,
    gray: Vec<u8>,
    pub last_action: usize,
    pub decisions: u64,
    pub episodes: u64,
    pub finished: u64,
    /// After a finished act, keep playing into the next one instead of
    /// starting over (`--agent-continue`).
    pub continue_acts: bool,
    /// Between a finished act and the next one (score tally, loading).
    between_acts: bool,
}

impl AgentPlayer {
    pub fn new(process: AgentProcess) -> Self {
        // The numbers the model was trained on (6, 11, 12 or 20).
        let features = match process.observation {
            Observation::Ram(set) => set,
            Observation::Image => FeatureSet::Basic,
        };
        let game = Sonic1 {
            features,
            ..Sonic1::default()
        };
        AgentPlayer {
            process,
            features: vec![0.0; game.feature_count()],
            game,
            stall_steps: STALL_STEPS,
            frames_left: 0,
            held: Buttons::NONE,
            start: SonicState::default(),
            best_x: 0,
            stalled: 0,
            gray: vec![0; GRAY_SIZE],
            last_action: 0,
            decisions: 0,
            episodes: 0,
            finished: 0,
            continue_acts: false,
            between_acts: false,
        }
    }

    #[cfg(test)]
    fn with_stall_steps(mut self, steps: u64) -> Self {
        self.stall_steps = steps;
        self
    }

    /// An episode begins from the console as it is now.
    pub fn begin_episode(&mut self, console: &Console) {
        self.start = self.game.read(console.bus().work_ram());
        self.best_x = self.game.progress(&self.start);
        self.stalled = 0;
        self.frames_left = 0;
        self.held = Buttons::NONE;
        self.between_acts = false;
    }

    /// The buttons of pad 1 for the next frame: a new decision every
    /// `FRAME_SKIP` frames, held in between.
    pub fn buttons(&mut self, console: &Console) -> anyhow::Result<Buttons> {
        if self.frames_left == 0 {
            let action = match self.process.observation {
                Observation::Ram(_) => {
                    let state = self.game.read(console.bus().work_ram());
                    self.game.features(&state, &mut self.features);
                    self.process.decide_ram(&self.features)?
                }
                Observation::Image => {
                    gray_downscale(
                        console.framebuffer(),
                        vdp::FRAME_WIDTH,
                        console.screen_width(),
                        console.screen_height(),
                        &mut self.gray,
                        GRAY_SIDE,
                        GRAY_SIDE,
                    );
                    self.process.decide_image(&self.gray)?
                }
            };
            self.last_action = action;
            self.held = Sonic1::ACTIONS[action];
            self.decisions += 1;
            self.frames_left = FRAME_SKIP;
        }
        self.frames_left -= 1;
        Ok(self.held)
    }

    /// After a frame: whether the episode is over, judged at the end of each
    /// step (every `FRAME_SKIP` frames) like in training. Counts it.
    pub fn episode_over(&mut self, console: &Console) -> Option<Outcome> {
        if self.frames_left != 0 || self.decisions == 0 {
            return None;
        }
        let now = self.game.read(console.bus().work_ram());
        if self.between_acts {
            return next_act_started(&self.start, &now).then_some(Outcome::NextAct);
        }
        let outcome = match self.game.end(&self.start, &now) {
            Some(End::Died) => Some(Outcome::Died),
            Some(End::Completed) => Some(Outcome::Finished),
            None => {
                let progress = self.game.progress(&now);
                if progress > self.best_x {
                    self.best_x = progress;
                    self.stalled = 0;
                } else {
                    self.stalled += 1;
                }
                (self.stalled >= self.stall_steps).then_some(Outcome::Stuck)
            }
        };
        if let Some(outcome) = outcome {
            self.episodes += 1;
            self.finished += u64::from(outcome == Outcome::Finished);
            self.between_acts = self.continue_acts && outcome == Outcome::Finished;
        }
        outcome
    }

    /// X now and the best of the episode, for the status line.
    pub fn position(&self, console: &Console) -> (u16, i64) {
        (self.game.read(console.bus().work_ram()).x, self.best_x)
    }
}

/// The act after `start`'s is loaded and Sonic placed in it. The act number
/// changes first, while Sonic still has the old act's position; the level
/// RAM is then cleared (x, y, right limit at 0) before the new act sets them
/// (seen on Green Hill Zone, python/next_act.py).
fn next_act_started(start: &SonicState, now: &SonicState) -> bool {
    (now.zone, now.act) != (start.zone, start.act)
        && now.limit_right > 0
        && now.x > 0
        && now.y > 0
        && now.x < start.limit_right
}

/// Names of `Sonic1::ACTIONS` (python/megadrive_env ACTION_NAMES).
pub const ACTION_NAMES: [&str; 7] = [
    "none",
    "right",
    "right+jump",
    "jump",
    "down",
    "left",
    "left+jump",
];

#[cfg(test)]
mod tests {
    use super::*;
    use md_core::testrom::pad_reader;
    use md_core::{Cartridge, MdConfig};
    use md_env::sonic1::addr;

    fn fake(args: &[&str]) -> anyhow::Result<AgentProcess> {
        let mut command = Command::new("python3");
        command
            .arg(repo().join("python/tests/fake_agent.py"))
            .args(args);
        AgentProcess::start(command)
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0x00, 0x80]), "/wCA");
    }

    #[test]
    fn the_protocol_with_numbers_and_pictures() {
        let mut ram = fake(&["ram"]).unwrap();
        assert_eq!(
            (ram.observation, ram.actions),
            (Observation::Ram(FeatureSet::Basic), 7)
        );
        assert_eq!(
            ram.decide_ram(&[0.5, 0.25, 0.0, -1.0, 1e-3, 1.0]).unwrap(),
            2
        );
        let mut extended = fake(&["ram11"]).unwrap();
        assert_eq!(extended.observation, Observation::Ram(FeatureSet::Extended));
        let mut player = AgentPlayer::new(extended);
        assert_eq!(player.features.len(), 11, "the model's 11 numbers");
        assert!(
            player.buttons(&console()).is_ok(),
            "11 numbers sent and accepted"
        );
        extended = fake(&["ram11"]).unwrap();
        assert!(
            extended.decide_ram(&[0.0; 6]).is_err(),
            "6 numbers refused by an 11-number model"
        );
        let full = fake(&["ram12"]).unwrap();
        assert_eq!(full.observation, Observation::Ram(FeatureSet::Full));
        let mut player = AgentPlayer::new(full);
        assert_eq!(player.features.len(), 12, "the model's 12 numbers");
        assert!(
            player.buttons(&console()).is_ok(),
            "12 numbers sent and accepted"
        );
        assert!(
            fake(&["ram12"]).unwrap().decide_ram(&[0.0; 11]).is_err(),
            "11 numbers refused by a 12-number model"
        );
        let objects = fake(&["ram20"]).unwrap();
        assert_eq!(objects.observation, Observation::Ram(FeatureSet::Objects));
        let mut player = AgentPlayer::new(objects);
        assert_eq!(player.features.len(), 20);
        assert!(
            player.buttons(&console()).is_ok(),
            "20 numbers sent and accepted"
        );
        let hazards = fake(&["ram24"]).unwrap();
        assert_eq!(hazards.observation, Observation::Ram(FeatureSet::Hazards));
        let mut player = AgentPlayer::new(hazards);
        assert_eq!(player.features.len(), 24);
        assert!(
            player.buttons(&console()).is_ok(),
            "24 numbers sent and accepted"
        );
        let mut image = fake(&["image"]).unwrap();
        assert_eq!(image.observation, Observation::Image);
        assert_eq!(image.decide_image(&vec![7u8; GRAY_SIZE]).unwrap(), 3);
    }

    #[test]
    fn a_dead_or_wrong_process_is_an_error() {
        let mut once = fake(&["ram", "--die-after", "1"]).unwrap();
        assert!(once.decide_ram(&[0.0; 6]).is_ok());
        let error = once.decide_ram(&[0.0; 6]).unwrap_err().to_string();
        assert!(error.contains("stopped"), "{error}");
        let mut wrong = fake(&["ram"]).unwrap();
        assert!(
            wrong.decide_ram(&[0.0; 3]).is_err(),
            "the server refuses 3 numbers"
        );
        let missing = AgentProcess::start(Command::new("/nonexistent/python"));
        assert!(missing.is_err());
    }

    fn console() -> Console {
        Console::new(
            Cartridge::from_bytes(&pad_reader()).unwrap(),
            MdConfig::default(),
        )
    }

    #[test]
    fn one_decision_every_four_frames_held_in_between() {
        let mut console = console();
        let mut player = AgentPlayer::new(fake(&["ram"]).unwrap());
        player.begin_episode(&console);
        let mut pressed = Vec::new();
        for _ in 0..8 {
            pressed.push(player.buttons(&console).unwrap());
            console.run_frame();
            assert_eq!(player.episode_over(&console), None);
        }
        assert_eq!(player.decisions, 2);
        assert!(
            pressed.iter().all(|&b| b == Sonic1::ACTIONS[2]),
            "right+jump, held"
        );
        assert_eq!(ACTION_NAMES[player.last_action], "right+jump");
    }

    #[test]
    fn episodes_end_by_death_level_end_or_stalling() {
        let mut console = console();
        // Sonic alive, timer running, so that the end rules apply.
        let set = |console: &mut Console, address: u32, value: u8| {
            console.bus_mut().write_work_ram(address, value)
        };
        let setup = |console: &mut Console| {
            set(console, addr::ROUTINE, 2);
            set(console, addr::TIME_COUNT, 1);
            set(console, addr::LIVES, 3);
        };
        let step = |player: &mut AgentPlayer, console: &mut Console| -> Option<Outcome> {
            let mut outcome = None;
            for _ in 0..FRAME_SKIP {
                player.buttons(console).unwrap();
                // The test ROM never writes these bytes: they stay as set.
                console.run_frame();
                outcome = outcome.or(player.episode_over(console));
            }
            outcome
        };
        let mut player = AgentPlayer::new(fake(&["ram"]).unwrap()).with_stall_steps(3);
        setup(&mut console);
        player.begin_episode(&console);
        set(&mut console, addr::X + 1, 50); // x grows: progress
        assert_eq!(step(&mut player, &mut console), None);
        set(&mut console, addr::ROUTINE, 6);
        assert_eq!(step(&mut player, &mut console), Some(Outcome::Died));
        setup(&mut console);
        player.begin_episode(&console);
        set(&mut console, addr::TIME_COUNT, 0);
        assert_eq!(step(&mut player, &mut console), Some(Outcome::Finished));
        setup(&mut console);
        player.begin_episode(&console);
        let ends: Vec<Option<Outcome>> = (0..3).map(|_| step(&mut player, &mut console)).collect();
        assert_eq!(
            ends,
            [None, None, Some(Outcome::Stuck)],
            "x still for 3 steps"
        );
        assert_eq!((player.episodes, player.finished), (3, 1));

        // Going on to the next act: no end while the tally runs, however
        // long, then NextAct once the next act is loaded.
        let mut player = AgentPlayer::new(fake(&["ram"]).unwrap()).with_stall_steps(3);
        player.continue_acts = true;
        setup(&mut console);
        // As at the end of Green Hill Zone 1: right limit 9216, Sonic past it.
        set(&mut console, addr::LIMIT_RIGHT, 0x24);
        set(&mut console, addr::X, 0x25);
        set(&mut console, addr::Y + 1, 0x80);
        player.begin_episode(&console);
        set(&mut console, addr::TIME_COUNT, 0);
        assert_eq!(step(&mut player, &mut console), Some(Outcome::Finished));
        let tally: Vec<Option<Outcome>> = (0..5).map(|_| step(&mut player, &mut console)).collect();
        assert_eq!(tally, [None; 5], "no stall during the tally");
        set(&mut console, addr::ACT, 1);
        assert_eq!(step(&mut player, &mut console), None, "old position still");
        set(&mut console, addr::X, 0x00);
        set(&mut console, addr::X + 1, 0x50);
        assert_eq!(step(&mut player, &mut console), Some(Outcome::NextAct));
        assert_eq!(
            (player.episodes, player.finished),
            (1, 1),
            "NextAct is no episode"
        );
    }
}

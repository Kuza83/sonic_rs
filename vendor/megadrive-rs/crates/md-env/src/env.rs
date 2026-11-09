//! The environment: the emulator, a game, and the `reset` / `step` loop.

use md_core::{Cartridge, CartridgeError, Console, MdConfig, Region, StateError};

use crate::frame::{GRAY_SIDE, GRAY_SIZE, gray_downscale};
use crate::game::{End, Game};

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error("ROM: {0}")]
    Rom(#[from] CartridgeError),
    #[error("start state: {0}")]
    State(#[from] StateError),
    #[error("frame_skip must be at least 1")]
    NoFrames,
}

#[derive(Debug, Clone, Copy)]
pub struct EnvConfig {
    /// Frames the buttons of one action are held (4 is the usual choice:
    /// 15 decisions a second, the game still reads the buttons every frame).
    pub frame_skip: u32,
    /// Steps after which an episode is cut short (`StepResult::truncated`).
    pub max_steps: Option<u64>,
    /// An episode is also cut short when `Game::progress` has not beaten its
    /// best for this many steps (the player is stuck).
    pub stall_steps: Option<u64>,
    /// Pad port the agent plays on (0 = port 1).
    pub port: usize,
    /// Console region; detected from the ROM header when None, like the
    /// frontend (a state made there loads here).
    pub region: Option<Region>,
    /// YM2612 sound synthesis. Off by default: the game runs the same
    /// (`Console::set_fm_synthesis`), several times faster, and an agent
    /// does not listen.
    pub fm_synthesis: bool,
}

impl Default for EnvConfig {
    fn default() -> Self {
        EnvConfig {
            frame_skip: 4,
            max_steps: None,
            stall_steps: None,
            port: 0,
            region: None,
            fm_synthesis: false,
        }
    }
}

/// What one `step` gives back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    pub reward: f32,
    /// The episode ended by the game's rules.
    pub end: Option<End>,
    /// The episode ran out of steps (`EnvConfig::max_steps`).
    pub truncated: bool,
}

impl StepResult {
    /// The episode is over, whatever the reason: `reset` before the next step.
    pub fn done(&self) -> bool {
        self.end.is_some() || self.truncated
    }
}

pub struct Env<G: Game> {
    console: Console,
    game: G,
    config: EnvConfig,
    /// Machine state each episode begins from: the power-on state, or one
    /// saved at a chosen moment of the game (`set_start_state`).
    start_state: Vec<u8>,
    start: G::State,
    prev: G::State,
    steps: u64,
    features: Vec<f32>,
    memory: G::Memory,
    /// Best `Game::progress` of the episode and the steps since it was beaten.
    best_progress: i64,
    stalled: u64,
}

impl<G: Game> Env<G> {
    /// An environment for the ROM image `rom`. Episodes begin at power-on
    /// until `set_start_state` says otherwise.
    pub fn new(rom: &[u8], game: G, config: EnvConfig) -> Result<Self, EnvError> {
        if config.frame_skip == 0 {
            return Err(EnvError::NoFrames);
        }
        let cart = Cartridge::from_bytes(rom)?;
        let region = config
            .region
            .unwrap_or_else(|| Region::detect(cart.region_field()));
        let mut console = Console::new(
            cart,
            MdConfig {
                region,
                ..MdConfig::default()
            },
        );
        console.set_fm_synthesis(config.fm_synthesis);
        let start_state = console.save_state();
        let start = game.read(console.bus().work_ram());
        Ok(Env {
            console,
            config,
            start_state,
            start,
            prev: start,
            steps: 0,
            features: vec![0.0; game.feature_count()],
            memory: G::Memory::default(),
            best_progress: game.progress(&start),
            stalled: 0,
            game,
        })
    }

    /// Makes `state` (a save state of the same ROM, such as the frontend's
    /// `.stateN` files) the start of every episode. The console goes to it
    /// at once; an invalid state leaves the environment as it was.
    pub fn set_start_state(&mut self, state: Vec<u8>) -> Result<(), EnvError> {
        self.console.load_state(&state)?;
        self.start_state = state;
        self.reset();
        Ok(())
    }

    /// Begins an episode: the machine returns to the start state, no button
    /// held. Returns the features at that moment.
    pub fn reset(&mut self) -> &[f32] {
        self.console
            .load_state(&self.start_state)
            .expect("the start state was loaded once already");
        self.console
            .set_buttons(self.config.port, md_core::Buttons::NONE);
        self.start = self.game.read(self.console.bus().work_ram());
        self.prev = self.start;
        self.steps = 0;
        self.memory = G::Memory::default();
        self.best_progress = self.game.progress(&self.start);
        self.stalled = 0;
        self.game.features(&self.start, &mut self.features);
        &self.features
    }

    /// Holds the buttons of action `action` (an index of `G::ACTIONS`) for
    /// `frame_skip` frames. After an episode is over (`StepResult::done`),
    /// `reset` first.
    pub fn step(&mut self, action: usize) -> StepResult {
        assert!(
            action < G::ACTIONS.len(),
            "action {action} out of {}",
            G::ACTIONS.len()
        );
        self.console
            .set_buttons(self.config.port, G::ACTIONS[action]);
        for _ in 0..self.config.frame_skip {
            self.console.run_frame();
        }
        let now = self.game.read(self.console.bus().work_ram());
        let reward = self.game.reward(&mut self.memory, &self.prev, &now);
        let end = self.game.end(&self.start, &now);
        self.steps += 1;
        let progress = self.game.progress(&now);
        if progress > self.best_progress {
            self.best_progress = progress;
            self.stalled = 0;
        } else {
            self.stalled += 1;
        }
        let out_of_steps = self.config.max_steps.is_some_and(|max| self.steps >= max);
        let stuck = self.config.stall_steps.is_some_and(|n| self.stalled >= n);
        let truncated = end.is_none() && (out_of_steps || stuck);
        self.prev = now;
        self.game.features(&now, &mut self.features);
        StepResult {
            reward,
            end,
            truncated,
        }
    }

    /// A save state of the machine as it is now (for `set_start_state`).
    pub fn save_state(&mut self) -> Vec<u8> {
        self.console.save_state()
    }

    /// The game's numbers after the last `reset` or `step`.
    pub fn features(&self) -> &[f32] {
        &self.features
    }

    /// The picture as 84 × 84 grey levels into `out` (`GRAY_SIZE` bytes).
    pub fn observe_gray(&self, out: &mut [u8]) {
        gray_downscale(
            self.console.framebuffer(),
            vdp::FRAME_WIDTH,
            self.console.screen_width(),
            self.console.screen_height(),
            out,
            GRAY_SIDE,
            GRAY_SIDE,
        );
    }

    /// The game's variables as they are now.
    pub fn state(&self) -> G::State {
        self.prev
    }

    /// Steps taken since the last `reset`.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn console(&self) -> &Console {
        &self.console
    }

    pub const fn action_count(&self) -> usize {
        G::ACTIONS.len()
    }

    pub const fn gray_size(&self) -> usize {
        GRAY_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ram_u8;
    use md_core::Buttons;
    use md_core::testrom::{PAD_READS, pad_reader};

    /// A game on the pad-reading test ROM: the two bytes it stores for the
    /// pad (TH low `?0SA00DU`, TH high `?1CBRLDU`, 0 = pressed).
    struct Toy;

    impl Game for Toy {
        type State = (u8, u8);
        type Memory = ();
        const ACTIONS: &'static [Buttons] = &[Buttons::NONE, Buttons::RIGHT, Buttons::START];

        fn read(&self, ram: &[u8]) -> (u8, u8) {
            (ram_u8(ram, PAD_READS), ram_u8(ram, PAD_READS + 1))
        }
        /// Right held is progress. Only in a real TH-high read (bit 6 set):
        /// the RAM before the first read is all zeros, which would read as
        /// every button held.
        fn progress(&self, s: &(u8, u8)) -> i64 {
            i64::from(s.1 & 0x40 != 0 && s.1 & 0x08 == 0)
        }
        fn reward(&self, _: &mut (), _: &(u8, u8), now: &(u8, u8)) -> f32 {
            // Right is bit 3 of the TH-high byte.
            if now.1 & 0x08 == 0 { 1.0 } else { 0.0 }
        }
        fn end(&self, _: &(u8, u8), now: &(u8, u8)) -> Option<End> {
            // Start is bit 5 of the TH-low byte.
            (now.0 & 0x20 == 0).then_some(End::Completed)
        }
        fn feature_count(&self) -> usize {
            2
        }
        fn features(&self, s: &(u8, u8), out: &mut [f32]) {
            out[0] = f32::from(s.0) / 255.0;
            out[1] = f32::from(s.1) / 255.0;
        }
    }

    fn env(config: EnvConfig) -> Env<Toy> {
        Env::new(&pad_reader(), Toy, config).unwrap()
    }

    #[test]
    fn a_step_runs_frame_skip_frames() {
        let mut e = env(EnvConfig::default());
        e.reset();
        let before = e.console().frame_count();
        e.step(0);
        assert_eq!(e.console().frame_count(), before + 4);
        let mut one = env(EnvConfig {
            frame_skip: 1,
            ..EnvConfig::default()
        });
        one.reset();
        let before = one.console().frame_count();
        one.step(0);
        assert_eq!(one.console().frame_count(), before + 1);
        assert!(matches!(
            Env::new(
                &pad_reader(),
                Toy,
                EnvConfig {
                    frame_skip: 0,
                    ..EnvConfig::default()
                }
            ),
            Err(EnvError::NoFrames)
        ));
    }

    #[test]
    fn actions_reach_the_game_through_the_pad() {
        let mut e = env(EnvConfig::default());
        e.reset();
        let idle = e.step(0);
        assert_eq!((idle.reward, idle.end), (0.0, None), "nothing pressed");
        let right = e.step(1);
        assert_eq!(right.reward, 1.0, "right pressed reads as a low bit");
        assert_eq!(e.state().1 & 0x08, 0);
        let released = e.step(0);
        assert_eq!(released.reward, 0.0, "and released");
        let start = e.step(2);
        assert_eq!(start.end, Some(End::Completed));
        assert!(start.done());
        assert!((e.features()[0] - f32::from(e.state().0) / 255.0).abs() < 1e-6);
    }

    #[test]
    fn episodes_are_cut_at_max_steps_unless_they_ended() {
        let mut e = env(EnvConfig {
            max_steps: Some(3),
            ..EnvConfig::default()
        });
        e.reset();
        let flags: Vec<bool> = (0..3).map(|_| e.step(0).truncated).collect();
        assert_eq!(flags, [false, false, true]);
        assert_eq!(e.steps(), 3);
        e.reset();
        assert_eq!(e.steps(), 0);
        let ended = e.step(2);
        assert!(
            ended.end.is_some() && !ended.truncated,
            "ending is not truncating"
        );
    }

    #[test]
    fn stuck_episodes_are_cut_after_stall_steps() {
        let mut e = env(EnvConfig {
            stall_steps: Some(3),
            ..EnvConfig::default()
        });
        e.reset();
        // Nothing pressed: no progress, cut at the third step.
        let cut: Vec<bool> = (0..3).map(|_| e.step(0).truncated).collect();
        assert_eq!(cut, [false, false, true]);
        e.reset();
        assert!(!e.step(0).truncated);
        assert!(
            !e.step(1).truncated,
            "right: a new best, the count starts over"
        );
        let after: Vec<bool> = (0..3).map(|_| e.step(0).truncated).collect();
        assert_eq!(
            after,
            [false, false, true],
            "three steps without beating it"
        );
        let mut never = env(EnvConfig::default());
        never.reset();
        assert!(
            (0..20).all(|_| !never.step(0).truncated),
            "no stall_steps, no cut"
        );
    }

    #[test]
    fn reset_is_deterministic() {
        let mut e = env(EnvConfig::default());
        // The whole machine, not just the picture (this ROM's is blank).
        let play = |e: &mut Env<Toy>| {
            e.reset();
            for action in [1, 1, 0, 2, 0, 1] {
                e.step(action);
            }
            e.console().clone().save_state()
        };
        let first = play(&mut e);
        let second = play(&mut e);
        assert!(first == second, "same actions, same machine state");
        let mut other = env(EnvConfig::default());
        assert!(play(&mut other) == first, "and across environments");
        e.reset();
        e.step(0);
        assert!(
            e.console().clone().save_state() != first,
            "different actions differ"
        );
    }

    #[test]
    fn a_chosen_start_state_begins_every_episode() {
        let mut e = env(EnvConfig::default());
        e.reset();
        for _ in 0..5 {
            e.step(0);
        }
        let state = {
            let mut console = e.console().clone();
            console.save_state()
        };
        let frame = e.console().frame_count();
        e.set_start_state(state).unwrap();
        e.step(1);
        e.reset();
        assert_eq!(e.console().frame_count(), frame, "back at the saved moment");
        assert!(e.set_start_state(vec![1, 2, 3]).is_err(), "garbage refused");
        e.reset();
        assert_eq!(e.console().frame_count(), frame, "and the old state kept");
    }

    #[test]
    fn fm_synthesis_is_off_unless_asked() {
        assert!(!env(EnvConfig::default()).console().fm_synthesis());
        let mut e = env(EnvConfig {
            fm_synthesis: true,
            ..EnvConfig::default()
        });
        assert!(e.console().fm_synthesis());
        e.reset();
        assert!(
            e.console().fm_synthesis(),
            "kept across reset (not in the state)"
        );
    }

    #[test]
    fn the_grey_picture_has_the_agent_size() {
        let mut e = env(EnvConfig::default());
        e.reset();
        e.step(0);
        let mut out = vec![0u8; e.gray_size()];
        e.observe_gray(&mut out);
        assert_eq!(out.len(), 84 * 84);
        assert_eq!(e.action_count(), 3);
    }
}

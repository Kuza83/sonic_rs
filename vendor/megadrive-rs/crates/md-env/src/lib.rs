//! The emulator as a reinforcement learning environment: `reset`, `step`
//! (hold an action's buttons for a few frames), an observation (the picture
//! reduced to grey levels, or numbers read from work RAM), a reward and an
//! end of episode. No SDL and no Python here: the bindings come on top.
//!
//! What belongs to one game (RAM addresses, actions, reward) is a `Game`;
//! `sonic1::Sonic1` is the first.

pub mod env;
pub mod frame;
pub mod game;
pub mod ramscan;
pub mod sonic1;

pub use env::{Env, EnvConfig, EnvError, StepResult};
pub use game::{End, Game};
pub use sonic1::{FeatureSet, RewardMode, Sonic1};

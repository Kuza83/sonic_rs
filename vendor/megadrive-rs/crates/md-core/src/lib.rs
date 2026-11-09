//! Mega Drive core: cartridge, memory bus, I/O and (later) system orchestration.

pub mod audio;
pub mod bus;
pub mod cartridge;
pub mod cheats;
pub mod console;
pub mod debug;
pub mod fm;
pub mod input;
pub mod io;
pub mod memory_map;
pub mod testrom;
mod z80_bus;
pub mod z80_space;

pub use bus::MdBus;
pub use cartridge::{Cartridge, CartridgeError, RomHeader};
pub use console::{Console, Hook};
pub use input::{Buttons, PadKind};
pub use io::{MdConfig, Region};
pub use savestate::StateError;

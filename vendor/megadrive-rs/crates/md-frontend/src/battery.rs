//! Cartridge SRAM kept in a `.srm` file next to the ROM.
//!
//! Loaded at start; written when the game has modified it, at most every
//! `SAVE_INTERVAL` while running, and when the emulator quits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use md_core::Console;

const SAVE_INTERVAL: Duration = Duration::from_secs(10);
const EXTENSION: &str = "srm";

pub struct Battery {
    path: PathBuf,
    unsaved: bool,
    last_save: Instant,
}

impl Battery {
    /// SRAM file of the ROM at `rom`, loaded into `console` if it exists.
    /// `None` when the cartridge has no SRAM.
    pub fn open(rom: &Path, console: &mut Console) -> anyhow::Result<Option<Self>> {
        if console.sram().is_none() {
            return Ok(None);
        }
        let path = rom.with_extension(EXTENSION);
        if path.exists() {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            console.load_sram(&bytes);
        }
        Ok(Some(Self {
            path,
            unsaved: false,
            last_save: Instant::now(),
        }))
    }

    /// Called once per frame: saves modified SRAM every `SAVE_INTERVAL`.
    pub fn tick(&mut self, console: &mut Console) -> anyhow::Result<()> {
        self.unsaved |= console.take_sram_dirty();
        if self.unsaved && self.last_save.elapsed() >= SAVE_INTERVAL {
            self.save(console)?;
        }
        Ok(())
    }

    /// Saves pending changes (on quit).
    pub fn flush(&mut self, console: &mut Console) -> anyhow::Result<()> {
        self.unsaved |= console.take_sram_dirty();
        if self.unsaved {
            self.save(console)?;
        }
        Ok(())
    }

    fn save(&mut self, console: &Console) -> anyhow::Result<()> {
        if let Some(data) = console.sram() {
            std::fs::write(&self.path, data)
                .with_context(|| format!("writing {}", self.path.display()))?;
        }
        self.unsaved = false;
        self.last_save = Instant::now();
        Ok(())
    }
}

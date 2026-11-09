//! Battery-backed cartridge RAM (SRAM).
//!
//! Declared in the ROM header at $1B0 (Plutiedev "ROM header"): "RA", a type
//! byte ($F8 = saved, odd bytes; $F0 even; $E0 word; $A0/$B0/$B8 volatile),
//! $20, then the first and last addresses where it shows up (e.g. $200001 -
//! $20FFFF for the common odd-byte chip).
//!
//! Mapping and storage follow Genesis Plus GX (`cart_hw/sram.c`,
//! `md_cart.c`): a 64 KiB buffer filled with $FF at power on, indexed by
//! address & $FFFF over the 64 KiB block holding the start address (so odd-
//! and even-byte chips need no special case: the unused bytes are simply
//! never accessed). SRAM placed beyond the end of the ROM is visible from
//! power on; SRAM overlapping the ROM (games > 2 MiB) is only visible once
//! the game writes bit 0 of $A130F1 (Plutiedev "Saving with SRAM"); bit 1
//! of that register write-protects it.

use super::RomHeader;

pub const SRAM_SIZE: usize = 0x1_0000;
const SRAM_INDEX_MASK: u32 = 0xFFFF;
/// Power-on contents (GPGX sram_init: "assume backup RAM is initialized to 0xFF").
const SRAM_FILL: u8 = 0xFF;
/// Header marker of the extra memory field.
const SRAM_MARKER: &[u8; 2] = b"RA";
/// SRAM must start inside the cartridge area ($000000-$3FFFFF).
const CART_AREA_END: u32 = 0x40_0000;
/// $A130F1 bits (GPGX mapper_sega_w).
const CTRL_MAPPED: u8 = 0x01;
const CTRL_WRITE_PROTECT: u8 = 0x02;

#[derive(Debug, Clone)]
pub struct Sram {
    data: Box<[u8; SRAM_SIZE]>,
    /// 64 KiB block ($xx0000) where the SRAM appears.
    block: u32,
    mapped: bool,
    /// Visibility at power on (SRAM beyond the ROM end), restored by resets.
    mapped_at_reset: bool,
    write_protected: bool,
    /// Written since the last `take_dirty`.
    dirty: bool,
}

impl Sram {
    /// SRAM declared by the header, if any. `rom_len` decides whether it is
    /// visible at power on.
    pub fn from_header(header: &RomHeader, rom_len: usize) -> Option<Self> {
        if &header.extra_memory[..2] != SRAM_MARKER {
            return None;
        }
        let start = u32::from_be_bytes(header.extra_memory[4..8].try_into().unwrap()) & 0xFF_FFFF;
        if start >= CART_AREA_END {
            // Not a cartridge address (e.g. $FF0000 = work RAM declared as
            // extra memory): no SRAM. GPGX applies per-game fixes here.
            return None;
        }
        let mapped = start as usize >= rom_len;
        Some(Self {
            data: Box::new([SRAM_FILL; SRAM_SIZE]),
            block: start & !SRAM_INDEX_MASK,
            mapped,
            mapped_at_reset: mapped,
            write_protected: false,
            dirty: false,
        })
    }

    /// Whether `addr` is currently served by the SRAM.
    pub fn claims(&self, addr: u32) -> bool {
        self.mapped && addr & !SRAM_INDEX_MASK == self.block
    }

    pub fn read8(&self, addr: u32) -> u8 {
        self.data[(addr & SRAM_INDEX_MASK) as usize]
    }

    pub fn write8(&mut self, addr: u32, value: u8) {
        if !self.write_protected {
            self.data[(addr & SRAM_INDEX_MASK) as usize] = value;
            self.dirty = true;
        }
    }

    /// $A130F1 written.
    pub fn control(&mut self, value: u8) {
        self.mapped = value & CTRL_MAPPED != 0;
        self.write_protected = value & CTRL_WRITE_PROTECT != 0;
    }

    /// Cartridge reset: register back to its power-on state, contents kept.
    pub fn reset(&mut self) {
        self.mapped = self.mapped_at_reset;
        self.write_protected = false;
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..]
    }

    /// Restores saved contents (shorter files fill the start of the buffer).
    pub fn load(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(SRAM_SIZE);
        self.data[..n].copy_from_slice(&bytes[..n]);
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

impl savestate::State for Sram {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.data)?;
        v.field(&mut self.mapped)?;
        v.field(&mut self.write_protected)?;
        v.field(&mut self.dirty)
    }
}

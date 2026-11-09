//! Cartridge loading: file format detection, header parsing, checksum.

mod checksum;
mod format;
mod header;
mod mapper;
mod sram;

use std::path::Path;

pub use header::RomHeader;
pub use mapper::Mapper;
pub use sram::{SRAM_SIZE, Sram};

/// Cartridge control registers ($A130xx, the /TIME range).
const TIME_SRAM_CONTROL: u32 = 0xA1_30F1;
const TIME_BANK_FIRST: u32 = 0xA1_30F3;
const TIME_BANK_LAST: u32 = 0xA1_30FF;

/// Smallest image that still holds the vector table and the full header.
pub const MIN_ROM_SIZE: usize = 0x200;

#[derive(Debug, thiserror::Error)]
pub enum CartridgeError {
    #[error("cannot read ROM file: {0}")]
    Io(#[from] std::io::Error),
    #[error("ROM too small: {0} bytes (minimum {MIN_ROM_SIZE})")]
    TooSmall(usize),
    #[error("SMD image size {0} is not 512 + a multiple of 16 KiB")]
    InvalidSmdSize(usize),
}

/// The cartridge board: ROM, optional SRAM, Sega bank mapper.
#[derive(Debug, Clone)]
pub struct Cartridge {
    rom: Box<[u8]>,
    header: RomHeader,
    sram: Option<Sram>,
    mapper: Mapper,
    /// Cheat patches: (even ROM offset, word), sorted by offset.
    rom_patches: Vec<(usize, u16)>,
}

impl Cartridge {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, CartridgeError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// Builds a cartridge from a raw `.bin` or interleaved `.smd` image.
    pub fn from_bytes(data: &[u8]) -> Result<Self, CartridgeError> {
        let mut rom = format::to_linear(data)?;
        if rom.len() < MIN_ROM_SIZE {
            return Err(CartridgeError::TooSmall(rom.len()));
        }
        // The 68000 fetches words: pad odd-sized dumps so every word is complete.
        if rom.len() % 2 != 0 {
            rom.push(0);
        }
        let header = RomHeader::parse(&rom);
        let sram = Sram::from_header(&header, rom.len());
        let mapper = Mapper::new(rom.len());
        Ok(Self {
            rom: rom.into_boxed_slice(),
            header,
            sram,
            mapper,
            rom_patches: Vec::new(),
        })
    }

    /// Byte at cartridge address `addr` ($000000-$3FFFFF): SRAM when mapped
    /// there, else ROM through the bank mapper (0 beyond the dump; the real
    /// value depends on the board).
    pub fn read8(&self, addr: u32) -> u8 {
        if let Some(sram) = self.sram.as_ref().filter(|s| s.claims(addr)) {
            return sram.read8(addr);
        }
        let offset = self.mapper.rom_offset(addr);
        if !self.rom_patches.is_empty()
            && let Ok(i) = self
                .rom_patches
                .binary_search_by_key(&(offset & !1), |&(at, _)| at)
        {
            let [high, low] = self.rom_patches[i].1.to_be_bytes();
            return if offset & 1 == 0 { high } else { low };
        }
        self.rom.get(offset).copied().unwrap_or(0)
    }

    /// Replaces the cheat patches: each (ROM offset, word) makes the ROM word
    /// at offset & !1 read as `word`. The last patch of an offset wins.
    pub fn set_rom_patches(&mut self, patches: &[(u32, u16)]) {
        self.rom_patches.clear();
        for &(offset, word) in patches {
            let offset = offset as usize & !1;
            match self
                .rom_patches
                .binary_search_by_key(&offset, |&(at, _)| at)
            {
                Ok(i) => self.rom_patches[i].1 = word,
                Err(i) => self.rom_patches.insert(i, (offset, word)),
            }
        }
    }

    /// Byte written to the cartridge area: only SRAM takes it.
    pub fn write8(&mut self, addr: u32, value: u8) {
        if let Some(sram) = self.sram.as_mut().filter(|s| s.claims(addr)) {
            sram.write8(addr, value);
        }
    }

    /// Byte written to the /TIME range ($A130xx): SRAM control at $A130F1,
    /// bank registers at $A130F3-$A130FF (odd addresses; GPGX default_time_w).
    pub fn write_time(&mut self, addr: u32, value: u8) {
        match addr {
            TIME_SRAM_CONTROL => {
                if let Some(sram) = &mut self.sram {
                    sram.control(value);
                }
            }
            TIME_BANK_FIRST..=TIME_BANK_LAST if addr & 1 == 1 => self.mapper.write(addr, value),
            _ => {}
        }
    }

    /// Cartridge reset (/VRES): mapper back to linear, SRAM register to its
    /// power-on state; SRAM contents are battery-backed and kept (GPGX
    /// md_cart_reset).
    pub fn reset(&mut self) {
        self.mapper = Mapper::new(self.rom.len());
        if let Some(sram) = &mut self.sram {
            sram.reset();
        }
    }

    /// FNV-1a 64 of the ROM image: identifies the cartridge in save states.
    pub fn rom_hash(&self) -> u64 {
        crate::console::fnv1a(&self.rom)
    }

    pub fn sram(&self) -> Option<&Sram> {
        self.sram.as_ref()
    }

    pub fn sram_mut(&mut self) -> Option<&mut Sram> {
        self.sram.as_mut()
    }

    pub fn mapper(&self) -> &Mapper {
        &self.mapper
    }

    pub fn mapper_mut(&mut self) -> &mut Mapper {
        &mut self.mapper
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// Raw country field of the header, $1F0-$1F3 (`Region::detect`).
    pub fn region_field(&self) -> &[u8] {
        &self.rom[header::offset::REGION..header::offset::REGION + 4]
    }

    pub fn header(&self) -> &RomHeader {
        &self.header
    }

    pub fn computed_checksum(&self) -> u16 {
        checksum::compute(&self.rom)
    }

    /// Many homebrews ship with a wrong checksum: a mismatch is informative only.
    pub fn checksum_ok(&self) -> bool {
        self.computed_checksum() == self.header.checksum
    }
}

#[cfg(test)]
pub(crate) mod test_rom {
    /// Builds a minimal linear ROM image with a recognizable header.
    pub fn build(size: usize) -> Vec<u8> {
        let mut rom = vec![0u8; size];
        rom[0x000..0x004].copy_from_slice(&0x00FF_FE00u32.to_be_bytes());
        rom[0x004..0x008].copy_from_slice(&0x0000_0200u32.to_be_bytes());
        put(&mut rom, 0x100, b"SEGA MEGA DRIVE ");
        put(&mut rom, 0x110, b"(C)TEST 2026.OCT");
        put(&mut rom, 0x120, &pad::<48>(b"DOMESTIC NAME"));
        put(&mut rom, 0x150, &pad::<48>(b"OVERSEAS NAME"));
        put(&mut rom, 0x180, b"GM 00000000-00");
        put(&mut rom, 0x190, &pad::<16>(b"J6"));
        rom[0x1A0..0x1A4].copy_from_slice(&0u32.to_be_bytes());
        rom[0x1A4..0x1A8].copy_from_slice(&(size as u32 - 1).to_be_bytes());
        rom[0x1A8..0x1AC].copy_from_slice(&0x00FF_0000u32.to_be_bytes());
        rom[0x1AC..0x1B0].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());
        put(&mut rom, 0x1B0, &pad::<12>(b""));
        put(&mut rom, 0x1BC, &pad::<12>(b""));
        put(&mut rom, 0x1C8, &pad::<40>(b"NOTES"));
        put(&mut rom, 0x1F0, b"JUE");
        put(&mut rom, 0x1F3, &pad::<13>(b""));
        rom
    }

    fn put(rom: &mut [u8], at: usize, bytes: &[u8]) {
        rom[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn pad<const N: usize>(s: &[u8]) -> [u8; N] {
        let mut out = [b' '; N];
        out[..s.len()].copy_from_slice(s);
        out
    }
}

/// Board state only: the ROM itself is identified by the state header.
impl savestate::State for Cartridge {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"CART", |v| {
            let mut pages = *self.mapper.pages();
            v.field(&mut pages)?;
            self.mapper.set_pages(pages);
            let mut has_sram = self.sram.is_some();
            v.field(&mut has_sram)?;
            match (&mut self.sram, has_sram) {
                (Some(sram), true) => v.state(sram),
                (None, false) => Ok(()),
                _ => Err(savestate::StateError::InvalidValue("SRAM presence")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_raw_image_and_checks_checksum() {
        let mut rom = test_rom::build(0x400);
        rom[0x200] = 0x12;
        rom[0x201] = 0x34;
        rom[0x18E..0x190].copy_from_slice(&0x1234u16.to_be_bytes());
        let cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.rom(), &rom[..]);
        assert_eq!(cart.computed_checksum(), 0x1234);
        assert!(cart.checksum_ok());
    }

    #[test]
    fn checksum_mismatch_is_reported_not_fatal() {
        let rom = test_rom::build(0x400);
        let cart = Cartridge::from_bytes(&rom).unwrap();
        // Header checksum is 0, data is all zero: matches. Corrupt it.
        let mut bad = rom.clone();
        bad[0x300] = 1;
        let bad = Cartridge::from_bytes(&bad).unwrap();
        assert!(cart.checksum_ok());
        assert!(!bad.checksum_ok());
    }

    #[test]
    fn rejects_too_small_image() {
        assert!(matches!(
            Cartridge::from_bytes(&[0; 0x100]),
            Err(CartridgeError::TooSmall(0x100))
        ));
    }

    #[test]
    fn odd_sized_image_is_padded() {
        let rom = test_rom::build(0x201);
        let cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.rom().len(), 0x202);
    }
}

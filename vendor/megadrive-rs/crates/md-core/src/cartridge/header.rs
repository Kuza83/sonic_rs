//! ROM header at $100–$1FF plus the reset vectors at $000/$004.
//! Field layout: Plutiedev, "ROM header".

pub(super) mod offset {
    pub const INITIAL_SSP: usize = 0x000;
    pub const INITIAL_PC: usize = 0x004;
    pub const SYSTEM_TYPE: usize = 0x100;
    pub const COPYRIGHT: usize = 0x110;
    pub const DOMESTIC_TITLE: usize = 0x120;
    pub const OVERSEAS_TITLE: usize = 0x150;
    pub const SERIAL: usize = 0x180;
    pub const CHECKSUM: usize = 0x18E;
    pub const DEVICE_SUPPORT: usize = 0x190;
    pub const ROM_RANGE: usize = 0x1A0;
    pub const RAM_RANGE: usize = 0x1A8;
    pub const EXTRA_MEMORY: usize = 0x1B0;
    pub const MODEM: usize = 0x1BC;
    pub const NOTES: usize = 0x1C8;
    pub const REGION: usize = 0x1F0;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomHeader {
    pub initial_ssp: u32,
    pub initial_pc: u32,
    pub system_type: String,
    pub copyright: String,
    pub domestic_title: String,
    pub overseas_title: String,
    pub serial: String,
    pub checksum: u16,
    pub device_support: String,
    pub rom_start: u32,
    pub rom_end: u32,
    pub ram_start: u32,
    pub ram_end: u32,
    /// Raw 12-byte SRAM descriptor, decoded in J11.
    pub extra_memory: [u8; 12],
    pub modem: String,
    pub notes: String,
    pub region: String,
}

impl RomHeader {
    /// `rom` must be at least `MIN_ROM_SIZE` bytes (checked by the caller).
    pub fn parse(rom: &[u8]) -> Self {
        use offset::*;
        Self {
            initial_ssp: be32(rom, INITIAL_SSP),
            initial_pc: be32(rom, INITIAL_PC),
            system_type: text(rom, SYSTEM_TYPE, 16),
            copyright: text(rom, COPYRIGHT, 16),
            domestic_title: text(rom, DOMESTIC_TITLE, 48),
            overseas_title: text(rom, OVERSEAS_TITLE, 48),
            serial: text(rom, SERIAL, 14),
            checksum: u16::from_be_bytes([rom[CHECKSUM], rom[CHECKSUM + 1]]),
            device_support: text(rom, DEVICE_SUPPORT, 16),
            rom_start: be32(rom, ROM_RANGE),
            rom_end: be32(rom, ROM_RANGE + 4),
            ram_start: be32(rom, RAM_RANGE),
            ram_end: be32(rom, RAM_RANGE + 4),
            extra_memory: rom[EXTRA_MEMORY..EXTRA_MEMORY + 12].try_into().unwrap(),
            modem: text(rom, MODEM, 12),
            notes: text(rom, NOTES, 40),
            region: text(rom, REGION, 3),
        }
    }
}

fn be32(rom: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(rom[at..at + 4].try_into().unwrap())
}

/// Header strings are space-padded ASCII; some dumps use NULs or Shift-JIS,
/// so decode lossily and trim both kinds of padding.
fn text(rom: &[u8], at: usize, len: usize) -> String {
    String::from_utf8_lossy(&rom[at..at + len])
        .trim_matches(|c: char| c == ' ' || c == '\0')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_rom;

    #[test]
    fn parses_every_field() {
        let mut rom = test_rom::build(0x10000);
        rom[0x18E..0x190].copy_from_slice(&0xBEEFu16.to_be_bytes());
        rom[0x1B0..0x1BC].copy_from_slice(b"RA\xF8\x20\x00\x20\x00\x01\x00\x20\x3F\xFF");
        let h = RomHeader::parse(&rom);
        assert_eq!(h.initial_ssp, 0x00FF_FE00);
        assert_eq!(h.initial_pc, 0x0000_0200);
        assert_eq!(h.system_type, "SEGA MEGA DRIVE");
        assert_eq!(h.copyright, "(C)TEST 2026.OCT");
        assert_eq!(h.domestic_title, "DOMESTIC NAME");
        assert_eq!(h.overseas_title, "OVERSEAS NAME");
        assert_eq!(h.serial, "GM 00000000-00");
        assert_eq!(h.checksum, 0xBEEF);
        assert_eq!(h.device_support, "J6");
        assert_eq!(h.rom_start, 0);
        assert_eq!(h.rom_end, 0xFFFF);
        assert_eq!(h.ram_start, 0x00FF_0000);
        assert_eq!(h.ram_end, 0x00FF_FFFF);
        assert_eq!(&h.extra_memory[..2], b"RA");
        assert_eq!(h.modem, "");
        assert_eq!(h.notes, "NOTES");
        assert_eq!(h.region, "JUE");
    }

    #[test]
    fn nul_padding_is_trimmed() {
        let mut rom = test_rom::build(0x200);
        rom[0x120..0x150].fill(0);
        rom[0x120..0x123].copy_from_slice(b"ABC");
        assert_eq!(RomHeader::parse(&rom).domestic_title, "ABC");
    }
}

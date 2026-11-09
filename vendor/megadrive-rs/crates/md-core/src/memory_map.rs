//! 68000 memory map of the Mega Drive (Plutiedev; Genesis Plus GX core/mem68k.c).

pub const ROM_START: u32 = 0x00_0000;
pub const ROM_END: u32 = 0x3F_FFFF;

pub const Z80_AREA_START: u32 = 0xA0_0000;
pub const Z80_AREA_END: u32 = 0xA0_FFFF;

pub const IO_START: u32 = 0xA1_0000;
pub const IO_END: u32 = 0xA1_001F;

/// Z80 bus request: write bit 8 (word) / bit 0 of the even byte;
/// reads return 0 in that bit once the bus is granted.
pub const Z80_BUSREQ: u32 = 0xA1_1100;
/// Z80 /RESET line: 0 = held in reset, 1 = running (same bit layout as BUSREQ).
pub const Z80_RESET: u32 = 0xA1_1200;
/// /TIME range: cartridge registers (SRAM control, bank mapper).
pub const TIME_START: u32 = 0xA1_3000;
pub const TIME_END: u32 = 0xA1_30FF;
/// TMSS register: writing "SEGA" (two words) unlocks the VDP on TMSS models.
pub const TMSS: u32 = 0xA1_4000;
pub const TMSS_END: u32 = 0xA1_4003;

pub const VDP_START: u32 = 0xC0_0000;
pub const VDP_END: u32 = 0xC0_001F;

pub const WORK_RAM_START: u32 = 0xE0_0000;
pub const WORK_RAM_END: u32 = 0xFF_FFFF;
pub const WORK_RAM_SIZE: usize = 0x1_0000;
/// 64 KiB mirrored across $E00000–$FFFFFF.
pub const WORK_RAM_MASK: u32 = 0xFFFF;

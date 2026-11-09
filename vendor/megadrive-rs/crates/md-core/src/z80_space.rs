//! Z80 address space as seen from the 68000 ($A00000–$A0FFFF).
//!
//! Decoding follows Genesis Plus GX core/mem68k.c (z80_read_byte/z80_write_byte),
//! switching on address bits 13–14:
//! - 0, 1: Z80 RAM (8 KiB, mirrored)
//! - 2   : YM2612 (4 ports, stub until J9)
//! - 3   : bank register at $6000–$60FF, VDP window at $7F00 (68000 lockup on
//!   hardware), anything else reads $FF.

const Z80_RAM_SIZE: usize = 0x2000;
const Z80_RAM_MASK: u32 = 0x1FFF;
const REGION_SHIFT: u32 = 13;
const REGION_MASK: u32 = 3;
const REGION_YM2612: u32 = 2;
const REGION_MISC: u32 = 3;
const BANK_REGISTER_PAGE: u32 = 0x60;
const MISC_UNMAPPED_READ: u8 = 0xFF;

/// Whether a Z80-area address (either CPU's view) selects the YM2612.
pub fn is_ym2612(addr: u32) -> bool {
    (addr >> REGION_SHIFT) & REGION_MASK == REGION_YM2612
}

#[derive(Debug, Clone)]
pub struct Z80Space {
    ram: Box<[u8; Z80_RAM_SIZE]>,
    /// 9-bit bank address, shifted in one bit per write (bit 0 of the data).
    bank: u16,
}

impl Z80Space {
    pub fn new() -> Self {
        Self {
            ram: Box::new([0; Z80_RAM_SIZE]),
            bank: 0,
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        match (addr >> REGION_SHIFT) & REGION_MASK {
            REGION_YM2612 => 0, // routed to the YM2612 by the bus
            REGION_MISC => MISC_UNMAPPED_READ,
            _ => self.ram[(addr & Z80_RAM_MASK) as usize],
        }
    }

    pub fn write8(&mut self, addr: u32, value: u8) {
        match (addr >> REGION_SHIFT) & REGION_MASK {
            REGION_YM2612 => {} // routed to the YM2612 by the bus
            REGION_MISC => {
                if (addr >> 8) & 0x7F == BANK_REGISTER_PAGE {
                    self.shift_bank_bit(value & 1);
                }
            }
            _ => self.ram[(addr & Z80_RAM_MASK) as usize] = value,
        }
    }

    /// Bank back to $000000 (reset; GPGX gen_reset: zbank = 0).
    pub fn reset_bank(&mut self) {
        self.bank = 0;
    }

    /// The bank register is a 9-bit shift register filled LSB-first:
    /// each write shifts the new bit in at bit 8 (GPGX gen_zbank_w).
    fn shift_bank_bit(&mut self, bit: u8) {
        self.bank = ((self.bank >> 1) | ((bit as u16) << 8)) & 0x1FF;
    }

    pub fn bank(&self) -> u16 {
        self.bank
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram[..]
    }

    /// 68000 address of the Z80 bank window ($8000-$FFFF) for `addr`.
    pub fn bank_address(&self, addr: u16) -> u32 {
        ((self.bank as u32) << 15) | (addr as u32 & 0x7FFF)
    }
}

impl Default for Z80Space {
    fn default() -> Self {
        Self::new()
    }
}

impl savestate::State for Z80Space {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"ZRAM", |v| {
            v.field(&mut self.ram)?;
            v.field(&mut self.bank)?;
            self.bank &= 0x1FF;
            Ok(())
        })
    }
}

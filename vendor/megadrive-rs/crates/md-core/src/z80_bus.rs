//! The bus as seen by the Z80 (Plutiedev "Using the Z80"; Genesis Plus GX
//! z80_memory_r/w):
//! - $0000-$3FFF: 8 KiB sound RAM (mirrored), shared with the 68000;
//! - $4000-$5FFF: YM2612; $6000-$60FF: bank register;
//! - $7F00-$7F1F: VDP ports (PSG at $7F11), same decoding as the 68000's;
//! - $8000-$FFFF: 32 KiB window on the 68000 bus at bank × $8000.
//!
//! The Z80 has no I/O devices on the Mega Drive: ports read $FF.

use m68k::Bus as _;

use crate::bus::MdBus;

const BANK_WINDOW_START: u16 = 0x8000;
const VDP_WINDOW_MASK: u16 = 0xFF00;
const VDP_WINDOW: u16 = 0x7F00;
const VDP_PORT_MASK: u16 = 0x1F;
const VDP_BASE_68K: u32 = 0x00C0_0000;
/// Undriven Z80 I/O port.
const OPEN_PORT: u8 = 0xFF;

/// Borrowing view of the machine bus for one stretch of Z80 execution.
pub(crate) struct Z80View<'a> {
    pub md: &'a mut MdBus,
}

impl z80::Bus for Z80View<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        if addr >= BANK_WINDOW_START {
            let target = self.md.z80_space().bank_address(addr);
            return self.md.read8(target);
        }
        if addr & VDP_WINDOW_MASK == VDP_WINDOW {
            return self.md.read8(VDP_BASE_68K | (addr & VDP_PORT_MASK) as u32);
        }
        if crate::z80_space::is_ym2612(addr as u32) {
            return self.md.ym_read(addr as u32);
        }
        self.md.z80_space().read8(addr as u32)
    }

    fn write(&mut self, addr: u16, value: u8) {
        if addr >= BANK_WINDOW_START {
            let target = self.md.z80_space().bank_address(addr);
            self.md.write8(target, value);
        } else if addr & VDP_WINDOW_MASK == VDP_WINDOW {
            self.md
                .write8(VDP_BASE_68K | (addr & VDP_PORT_MASK) as u32, value);
        } else if crate::z80_space::is_ym2612(addr as u32) {
            self.md.ym_write(addr as u32, value);
        } else {
            self.md.z80_space_mut().write8(addr as u32, value);
        }
    }

    fn port_in(&mut self, _port: u16) -> u8 {
        OPEN_PORT
    }

    fn port_out(&mut self, _port: u16, _value: u8) {}
}

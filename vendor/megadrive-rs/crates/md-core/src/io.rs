//! I/O chip ($A10000–$A1001F) and system control registers (Z80 bus, TMSS).

use crate::input::{Buttons, Gamepad, PadKind};

/// I/O register indices: register n lives at $A10001 + 2n (odd addresses).
/// Genesis Plus GX, core/io_ctrl.c.
mod reg {
    pub const VERSION: usize = 0x0;
    /// Data registers of ports 1, 2 and expansion ($A10003/5/7).
    pub const DATA_1: usize = 0x1;
    pub const DATA_EXP: usize = 0x3;
    /// Control (direction) registers, 3 registers after their data register.
    pub const CTRL_OFFSET: usize = 3;
    pub const CTRL_1: usize = 0x4;
    pub const CTRL_EXP: usize = 0x6;
    pub const COUNT: usize = 0x10;
    /// Serial TxData registers reset to $FF, $FF, $FB.
    pub const TX_DATA_A: usize = 0x7;
    pub const TX_DATA_B: usize = 0xA;
    pub const TX_DATA_C: usize = 0xD;
}

/// Version register ($A10001) bits (GPGX core/system.h REGION_*, core/io_ctrl.c).
mod version {
    pub const OVERSEAS: u8 = 0x80;
    pub const PAL: u8 = 0x40;
    /// Set when no Mega-CD is attached.
    pub const NO_EXPANSION: u8 = 0x20;
    /// Hardware revision 1 = console with TMSS.
    pub const HW_REV_TMSS: u8 = 0x01;
}

const TMSS_KEY: &[u8; 4] = b"SEGA";
/// Number of controller ports (1, 2, expansion).
pub const PORT_COUNT: usize = 3;
/// What an empty port drives: every input pulled high (GPGX `dummy_read`).
const EMPTY_PORT: u8 = 0x7F;
/// Data bit 7 is not wired to the connector: it always reads the latch.
const DATA_LATCH_ONLY: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Region {
    JapanNtsc,
    JapanPal,
    #[default]
    Usa,
    Europe,
}

/// Region bits of the header's country field (GPGX loadrom.c get_region).
const COUNTRY_JAPAN_NTSC: u8 = 1;
const COUNTRY_JAPAN_PAL: u8 = 2;
const COUNTRY_USA: u8 = 4;
const COUNTRY_EUROPE: u8 = 8;

impl Region {
    /// Console region for a cartridge whose header country field
    /// ($1F0-$1F3) is `field`, as Genesis Plus GX picks it (loadrom.c
    /// get_region): "EUR"/"JAP"/"USA" prefixes, else letters U, J/K, E, else
    /// hex digits of the newer bit mask (1 Japan NTSC, 2 Japan PAL, 4 USA,
    /// 8 Europe); when several are allowed, USA > Japan NTSC > Europe >
    /// Japan PAL; USA by default. GPGX's per-game overrides (product codes)
    /// are not reproduced.
    pub fn detect(field: &[u8]) -> Region {
        let starts = |p: &[u8]| field.len() >= 3 && &field[..3] == p;
        let country = if starts(b"eur") || starts(b"EUR") || starts(b"Eur") {
            COUNTRY_EUROPE
        } else if starts(b"jap") || starts(b"JAP") {
            COUNTRY_JAPAN_NTSC
        } else if starts(b"usa") || starts(b"USA") {
            COUNTRY_USA
        } else {
            field.iter().take(4).fold(0, |bits, &c| {
                bits | match c.to_ascii_uppercase() {
                    b'U' => COUNTRY_USA,
                    b'J' | b'K' => COUNTRY_JAPAN_NTSC,
                    b'E' => COUNTRY_EUROPE,
                    c @ 0..=15 => c,
                    c @ b'0'..=b'9' => c - b'0',
                    c @ b'A'..=b'F' => c - b'A' + 10,
                    _ => 0,
                }
            })
        };
        if country & COUNTRY_USA != 0 {
            Region::Usa
        } else if country & COUNTRY_JAPAN_NTSC != 0 {
            Region::JapanNtsc
        } else if country & COUNTRY_EUROPE != 0 {
            Region::Europe
        } else if country & COUNTRY_JAPAN_PAL != 0 {
            Region::JapanPal
        } else {
            Region::Usa
        }
    }

    pub fn is_pal(self) -> bool {
        matches!(self, Region::JapanPal | Region::Europe)
    }

    fn version_bits(self) -> u8 {
        match self {
            Region::JapanNtsc => 0,
            Region::JapanPal => version::PAL,
            Region::Usa => version::OVERSEAS,
            Region::Europe => version::OVERSEAS | version::PAL,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MdConfig {
    pub region: Region,
    /// Emulate a TMSS console (hardware revision ≥ 1). Off by default.
    pub tmss: bool,
}

#[derive(Debug, Clone)]
pub struct Io {
    regs: [u8; reg::COUNT],
    z80_busreq: bool,
    /// Z80 /RESET released (Z80 running). Held in reset at power-on (GPGX gen_reset: zstate = 0).
    z80_running: bool,
    tmss_enabled: bool,
    tmss_key: [u8; 4],
    pads: [Option<Gamepad>; PORT_COUNT],
}

impl Io {
    pub fn new(config: MdConfig) -> Self {
        let mut regs = [0u8; reg::COUNT];
        regs[reg::VERSION] = config.region.version_bits()
            | version::NO_EXPANSION
            | if config.tmss { version::HW_REV_TMSS } else { 0 };
        regs[reg::TX_DATA_A] = 0xFF;
        regs[reg::TX_DATA_B] = 0xFF;
        regs[reg::TX_DATA_C] = 0xFB;
        Self {
            regs,
            z80_busreq: false,
            z80_running: false,
            tmss_enabled: config.tmss,
            tmss_key: [0; 4],
            // Two 3-button pads plugged in, nothing on the expansion port.
            pads: [
                Some(Gamepad::new(PadKind::ThreeButton)),
                Some(Gamepad::new(PadKind::ThreeButton)),
                None,
            ],
        }
    }

    /// Plugs a pad of `kind` into `port` (0 = port 1), or unplugs it.
    pub fn connect(&mut self, port: usize, kind: Option<PadKind>) {
        self.pads[port] = kind.map(Gamepad::new);
    }

    pub fn set_buttons(&mut self, port: usize, buttons: Buttons) {
        if let Some(pad) = &mut self.pads[port] {
            pad.set_buttons(buttons);
        }
    }

    /// Once per scanline (6-button sequence timeout).
    pub fn refresh_line(&mut self) {
        for pad in self.pads.iter_mut().flatten() {
            pad.refresh_line();
        }
    }

    /// Reads I/O register at `offset` = address & 0x1F. Both bytes of a word
    /// read return the register (GPGX ctrl_io_read_word duplicates it).
    /// Data ports mix the latch (output bits and D7) with the connector inputs
    /// (GPGX io_68k_read: `mask = 0x80 | ctrl`).
    pub fn read(&self, offset: u32) -> u8 {
        let index = Self::index(offset);
        if (reg::DATA_1..=reg::DATA_EXP).contains(&index) {
            let mask = DATA_LATCH_ONLY | self.regs[index + reg::CTRL_OFFSET];
            let input = self.pads[index - reg::DATA_1]
                .as_ref()
                .map_or(EMPTY_PORT, Gamepad::read);
            return (self.regs[index] & mask) | (input & !mask);
        }
        self.regs[index]
    }

    /// Byte writes only land on odd addresses (GPGX ctrl_io_write_byte);
    /// word writes use the low byte. The version register is read-only.
    /// Data and control writes are forwarded to the plugged device.
    pub fn write(&mut self, offset: u32, value: u8) {
        let index = Self::index(offset);
        if index == reg::VERSION {
            return;
        }
        self.regs[index] = value;
        let port = match index {
            reg::DATA_1..=reg::DATA_EXP => index - reg::DATA_1,
            reg::CTRL_1..=reg::CTRL_EXP => index - reg::CTRL_1,
            _ => return,
        };
        let data = self.regs[reg::DATA_1 + port];
        let ctrl = self.regs[reg::CTRL_1 + port];
        if let Some(pad) = &mut self.pads[port] {
            pad.write(data, ctrl);
        }
    }

    fn index(offset: u32) -> usize {
        ((offset >> 1) & 0xF) as usize
    }

    /// The 68000 owns the Z80 bus only when BUSREQ is asserted and the Z80 is
    /// out of reset (GPGX zstate == 3; Plutiedev "Using the Z80").
    pub fn z80_bus_granted(&self) -> bool {
        self.z80_busreq && self.z80_running
    }

    pub fn set_z80_busreq(&mut self, asserted: bool) {
        self.z80_busreq = asserted;
    }

    pub fn set_z80_reset_released(&mut self, released: bool) {
        self.z80_running = released;
    }

    /// Pad plugged into `port`, if any.
    pub fn pad_kind(&self, port: usize) -> Option<PadKind> {
        self.pads[port].as_ref().map(|p| p.kind())
    }

    pub fn z80_running(&self) -> bool {
        self.z80_running
    }

    pub fn z80_busreq(&self) -> bool {
        self.z80_busreq
    }

    /// Word write into the TMSS register; `offset` is 0 or 2.
    pub fn write_tmss(&mut self, offset: u32, value: u16) {
        let at = (offset & 2) as usize;
        self.tmss_key[at..at + 2].copy_from_slice(&value.to_be_bytes());
    }

    /// VDP is accessible unless the console has TMSS and "SEGA" was not written yet.
    pub fn vdp_unlocked(&self) -> bool {
        !self.tmss_enabled || &self.tmss_key == TMSS_KEY
    }
}

impl savestate::State for Io {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"IO  ", |v| {
            v.field(&mut self.regs)?;
            v.field(&mut self.z80_busreq)?;
            v.field(&mut self.z80_running)?;
            v.field(&mut self.tmss_enabled)?;
            v.field(&mut self.tmss_key)?;
            for pad in &mut self.pads {
                let mut present = pad.is_some();
                v.field(&mut present)?;
                if present {
                    v.state(pad.get_or_insert_with(|| Gamepad::new(PadKind::ThreeButton)))?;
                } else {
                    *pad = None;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_from_the_header_country_field() {
        use Region::*;
        for (field, expected) in [
            (&b"JUE "[..], Usa), // USA preferred over Japan and Europe
            (b"J   ", JapanNtsc),
            (b"K   ", JapanNtsc),
            (b"E   ", Europe),
            (b"JE  ", JapanNtsc), // Japan NTSC preferred over Europe
            (b"EU  ", Usa),
            (b"1   ", JapanNtsc), // newer hex bit mask
            (b"2   ", JapanPal),
            (b"4   ", Usa),
            (b"8   ", Europe),
            (b"9   ", JapanNtsc),
            (b"A   ", Europe), // 2 + 8: Europe preferred over Japan PAL
            (b"F   ", Usa),
            (b"EUR ", Europe), // prefixes
            (b"Europe", Europe),
            (b"JAP ", JapanNtsc),
            (b"usa ", Usa),
            (b"    ", Usa), // nothing: USA
            (b"", Usa),
        ] {
            assert_eq!(
                Region::detect(field),
                expected,
                "{:?}",
                String::from_utf8_lossy(field)
            );
        }
    }
}

//! Programmer-visible registers, plus the internal ones the tests observe:
//! WZ (MEMPTR), Q and P ("The Undocumented Z80 Documented", Sean Young).

/// Flag bits of F.
pub mod flag {
    pub const C: u8 = 1 << 0;
    pub const N: u8 = 1 << 1;
    pub const PV: u8 = 1 << 2;
    /// Undocumented copy of result bit 3.
    pub const X: u8 = 1 << 3;
    pub const H: u8 = 1 << 4;
    /// Undocumented copy of result bit 5.
    pub const Y: u8 = 1 << 5;
    pub const Z: u8 = 1 << 6;
    pub const S: u8 = 1 << 7;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// Alternate set AF', BC', DE', HL'.
    pub af_alt: u16,
    pub bc_alt: u16,
    pub de_alt: u16,
    pub hl_alt: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    pub i: u8,
    /// Refresh register: bit 7 is only changed by LD R,A.
    pub r: u8,
    /// Internal MEMPTR register, leaks into the X/Y flags of BIT n,(HL).
    pub wz: u16,
    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,
    /// F as set by the last instruction if it changed the flags, 0 otherwise
    /// (needed by SCF/CCF).
    pub q: u8,
    /// Set by LD A,I / LD A,R (interrupt acceptance quirk on P/V).
    pub p: bool,
    /// The previous instruction was EI: interrupts wait one more instruction.
    pub ei: bool,
}

macro_rules! pair {
    ($get:ident, $set:ident, $hi:ident, $lo:ident) => {
        pub fn $get(&self) -> u16 {
            u16::from_be_bytes([self.$hi, self.$lo])
        }
        pub fn $set(&mut self, value: u16) {
            [self.$hi, self.$lo] = value.to_be_bytes();
        }
    };
}

impl Registers {
    pair!(af, set_af, a, f);
    pair!(bc, set_bc, b, c);
    pair!(de, set_de, d, e);
    pair!(hl, set_hl, h, l);

    /// R after one opcode fetch (M1): the low 7 bits count, bit 7 is kept.
    pub fn bump_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_pairs() {
        let mut r = Registers::default();
        r.set_hl(0x1234);
        assert_eq!((r.h, r.l), (0x12, 0x34));
        r.set_af(0xABCD);
        assert_eq!(r.af(), 0xABCD);
    }

    #[test]
    fn r_keeps_bit_7() {
        let mut r = Registers {
            r: 0xFF,
            ..Default::default()
        };
        r.bump_r();
        assert_eq!(r.r, 0x80);
    }
}

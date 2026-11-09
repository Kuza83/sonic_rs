//! Programmer-visible registers of the 68000.

/// Status register bits (M68000 Programmer's Reference Manual §1.3.2).
pub mod sr {
    pub const C: u16 = 1 << 0;
    pub const V: u16 = 1 << 1;
    pub const Z: u16 = 1 << 2;
    pub const N: u16 = 1 << 3;
    pub const X: u16 = 1 << 4;
    pub const INT_MASK: u16 = 0x0700;
    pub const INT_SHIFT: u16 = 8;
    /// Supervisor state.
    pub const S: u16 = 1 << 13;
    /// Trace mode.
    pub const T: u16 = 1 << 15;
    /// Bits that physically exist on the 68000; the others always read as 0.
    pub const IMPLEMENTED: u16 = T | S | INT_MASK | X | N | Z | V | C;
    /// Condition code register (low byte of SR).
    pub const CCR: u16 = X | N | Z | V | C;
}

/// `a[7]` is always the *active* stack pointer; the inactive one lives in
/// `inactive_sp` and is swapped whenever SR.S changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Registers {
    pub d: [u32; 8],
    pub a: [u32; 8],
    inactive_sp: u32,
    pub pc: u32,
    sr: u16,
}

impl Registers {
    pub fn sr(&self) -> u16 {
        self.sr
    }

    /// Writes SR, swapping stack pointers when the supervisor bit changes.
    pub fn set_sr(&mut self, value: u16) {
        let value = value & sr::IMPLEMENTED;
        if (value ^ self.sr) & sr::S != 0 {
            std::mem::swap(&mut self.a[7], &mut self.inactive_sp);
        }
        self.sr = value;
    }

    pub fn supervisor(&self) -> bool {
        self.sr & sr::S != 0
    }

    pub fn usp(&self) -> u32 {
        if self.supervisor() {
            self.inactive_sp
        } else {
            self.a[7]
        }
    }

    pub fn ssp(&self) -> u32 {
        if self.supervisor() {
            self.a[7]
        } else {
            self.inactive_sp
        }
    }

    pub fn set_usp(&mut self, value: u32) {
        if self.supervisor() {
            self.inactive_sp = value;
        } else {
            self.a[7] = value;
        }
    }

    pub fn set_ssp(&mut self, value: u32) {
        if self.supervisor() {
            self.a[7] = value;
        } else {
            self.inactive_sp = value;
        }
    }
}

impl savestate::State for Registers {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.d)?;
        v.field(&mut self.a)?;
        v.field(&mut self.inactive_sp)?;
        v.field(&mut self.pc)?;
        v.field(&mut self.sr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sr_unimplemented_bits_read_zero() {
        let mut r = Registers::default();
        r.set_sr(0xFFFF);
        assert_eq!(r.sr(), 0xA71F);
    }

    #[test]
    fn stack_pointers_swap_on_mode_change() {
        let mut r = Registers::default();
        r.set_sr(sr::S);
        r.set_ssp(0x1000);
        r.set_usp(0x2000);
        assert_eq!(r.a[7], 0x1000);
        r.set_sr(0);
        assert_eq!(r.a[7], 0x2000);
        assert_eq!(r.ssp(), 0x1000);
        assert_eq!(r.usp(), 0x2000);
    }
}

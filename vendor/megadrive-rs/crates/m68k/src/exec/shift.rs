//! Shifts and rotates: ASL/ASR, LSL/LSR, ROXL/ROXR, ROL/ROR.
//! Flags: M68000 PRM, instruction pages (V only for ASL: set if the MSB
//! changes at any time during the shift).

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::Operand;
use crate::instruction::{Ea, ShiftCount, ShiftKind, Size};
use crate::registers::sr;

/// Register shifts: base internal cycles before the per-bit cost.
const SHIFT_BASE_SHORT: u32 = 2;
const SHIFT_BASE_LONG: u32 = 4;
const SHIFT_PER_BIT: u32 = 2;
/// Register counts are taken modulo 64.
const REGISTER_COUNT_MASK: u32 = 63;

impl Cpu {
    /// Shifts `value` by `count` bits and updates X/N/Z/V/C.
    fn shift(&mut self, kind: ShiftKind, left: bool, size: Size, value: u32, count: u32) -> u32 {
        let (mask, msb) = (size.mask(), size.msb());
        let old_sr = self.regs.sr();
        let mut v = value & mask;
        let mut x = old_sr & sr::X != 0;
        let mut carry = false;
        let mut overflow = false;
        for _ in 0..count {
            let out = if left { v & msb != 0 } else { v & 1 != 0 };
            let msb_before = v & msb;
            v = match (kind, left) {
                (ShiftKind::Arithmetic | ShiftKind::Logical, true) => v << 1,
                (ShiftKind::Arithmetic, false) => (v >> 1) | msb_before,
                (ShiftKind::Logical, false) => v >> 1,
                (ShiftKind::Rotate, true) => (v << 1) | out as u32,
                (ShiftKind::Rotate, false) => (v >> 1) | if out { msb } else { 0 },
                (ShiftKind::RotateExtend, true) => (v << 1) | x as u32,
                (ShiftKind::RotateExtend, false) => (v >> 1) | if x { msb } else { 0 },
            } & mask;
            if kind == ShiftKind::Arithmetic && left && v & msb != msb_before {
                overflow = true;
            }
            carry = out;
            if kind != ShiftKind::Rotate {
                x = out;
            }
        }
        // A zero count leaves X alone and clears C, except ROXd where C = X.
        if count == 0 && kind == ShiftKind::RotateExtend {
            carry = x;
        }
        let mut ccr = old_sr & sr::X;
        if count > 0 && kind != ShiftKind::Rotate {
            ccr = if x { sr::X } else { 0 };
        }
        if v & msb != 0 {
            ccr |= sr::N;
        }
        if v == 0 {
            ccr |= sr::Z;
        }
        if overflow {
            ccr |= sr::V;
        }
        if carry {
            ccr |= sr::C;
        }
        self.regs.set_sr((old_sr & !sr::CCR) | ccr);
        v
    }

    pub(super) fn op_shift<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: ShiftKind,
        left: bool,
        size: Size,
        count: ShiftCount,
        reg: u8,
    ) -> Res {
        let count = match count {
            ShiftCount::Immediate(n) => n as u32,
            ShiftCount::Register(r) => self.regs.d[r as usize] & REGISTER_COUNT_MASK,
        };
        let value = self.shift(kind, left, size, self.regs.d[reg as usize], count);
        self.write_data_reg(reg, size, value);
        self.fetch(bus)?;
        let base = if size == Size::Long {
            SHIFT_BASE_LONG
        } else {
            SHIFT_BASE_SHORT
        };
        self.idle(base + SHIFT_PER_BIT * count);
        Ok(())
    }

    pub(super) fn op_shift_mem<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: ShiftKind,
        left: bool,
        dst: Ea,
    ) -> Res {
        let Operand::Memory(addr) = self.resolve(bus, dst, Size::Word)? else {
            unreachable!("memory shift destination is memory alterable");
        };
        let value = self.read(bus, addr, Size::Word)?;
        let value = self.shift(kind, left, Size::Word, value, 1);
        self.fetch(bus)?;
        self.write(bus, addr, Size::Word, value)
    }
}

#[cfg(test)]
mod tests {
    use crate::bus::{Bus, FlatBus};
    use crate::cpu::Cpu;
    use crate::registers::sr;

    fn run(opcode: u16, d0: u32, d1: u32) -> Cpu {
        let mut bus = FlatBus::new();
        let mut cpu = Cpu::new();
        cpu.regs.set_sr(sr::S);
        cpu.regs.pc = 0x1000;
        cpu.prefetch = [opcode, 0x4E71];
        cpu.regs.d[0] = d0;
        cpu.regs.d[1] = d1;
        bus.write16(0x1004, 0x4E71);
        cpu.step(&mut bus);
        cpu
    }

    /// The sign bit keeps being shifted out: C and X end up set (M68000 PRM,
    /// ASR: "C — Set according to the last bit shifted out").
    #[test]
    fn asr_oversized_negative_sets_carry_and_extend() {
        // asr.b d1,d0 with a count of 12
        let cpu = run(0xE220, 0x80, 12);
        assert_eq!(cpu.regs.d[0] & 0xFF, 0xFF);
        assert_eq!(cpu.regs.sr() & (sr::C | sr::X), sr::C | sr::X);
    }

    #[test]
    fn asl_sets_overflow_when_msb_changes() {
        // asl.b #1,d0 : 0x40 -> 0x80
        let cpu = run(0xE300, 0x40, 0);
        assert_eq!(cpu.regs.d[0] & 0xFF, 0x80);
        assert_ne!(cpu.regs.sr() & sr::V, 0);
    }

    #[test]
    fn roxl_zero_count_copies_x_to_c() {
        let mut bus = FlatBus::new();
        let mut cpu = Cpu::new();
        cpu.regs.set_sr(sr::S | sr::X);
        cpu.prefetch = [0xE330, 0x4E71]; // roxl.b d1,d0
        cpu.regs.d[1] = 64; // count 64 mod 64 = 0
        bus.write16(0x0004, 0x4E71);
        cpu.step(&mut bus);
        assert_eq!(cpu.regs.sr() & (sr::C | sr::X), sr::C | sr::X);
    }
}

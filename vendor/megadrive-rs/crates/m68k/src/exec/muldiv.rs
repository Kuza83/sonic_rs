//! MULU/MULS/DIVU/DIVS.
//!
//! Timing is data dependent:
//! - MULU: 38 + 2n cycles, n = number of 1 bits in the source; MULS: 38 + 2n,
//!   n = number of 01/10 pairs in (source << 1) (M68000 UM §8, footnotes).
//! - DIVU/DIVS: Jorge Cwik's cycle-exact algorithms ("68000 Divide timing",
//!   also used by Genesis Plus GX and MAME).

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::exceptions::vector;
use crate::instruction::{Ea, MulDivOp, Size};
use crate::registers::sr;

/// MUL internal cycles before the per-bit cost (the prefetch takes the other 4).
const MUL_BASE_INTERNAL: u32 = 34;
/// Idle cycles between reading a zero divisor and the exception frame.
const ZERO_DIVIDE_INTERNAL: u32 = 8;
/// Cycles of the final prefetch, included in the published DIV totals.
const PREFETCH_CYCLES: u32 = 4;

/// DIVU total cycles (register source); 10 on overflow.
pub fn divu_cycles(dividend: u32, divisor: u16) -> u32 {
    if (dividend >> 16) >= divisor as u32 {
        return 10;
    }
    let hdivisor = (divisor as u32) << 16;
    let mut dividend = dividend;
    let mut mcycles = 38;
    for _ in 0..15 {
        let carry = dividend & 0x8000_0000 != 0;
        dividend <<= 1;
        if carry {
            dividend = dividend.wrapping_sub(hdivisor);
        } else {
            mcycles += 2;
            if dividend >= hdivisor {
                dividend -= hdivisor;
                mcycles -= 1;
            }
        }
    }
    mcycles * 2
}

/// DIVS total cycles (register source). Signed overflow detected after the
/// division takes the full time; only absolute overflow exits early.
pub fn divs_cycles(dividend: i32, divisor: i16) -> u32 {
    let mut mcycles = if dividend < 0 { 7 } else { 6 };
    let abs_dividend = dividend.unsigned_abs();
    let abs_divisor = divisor.unsigned_abs() as u32;
    if abs_dividend >> 16 >= abs_divisor {
        return (mcycles + 2) * 2;
    }
    let mut aquot = abs_dividend / abs_divisor;
    mcycles += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            mcycles -= 1;
        } else {
            mcycles += 1;
        }
    }
    for _ in 0..15 {
        if aquot as i16 >= 0 {
            mcycles += 1;
        }
        aquot <<= 1;
    }
    mcycles * 2
}

impl Cpu {
    pub(super) fn op_muldiv<B: Bus>(&mut self, bus: &mut B, op: MulDivOp, src: Ea, dst: u8) -> Res {
        let start_pc = self.regs.pc;
        let (_, source) = self.resolve_read(bus, src, Size::Word)?;
        let source = source as u16;
        let d = self.regs.d[dst as usize];
        match op {
            MulDivOp::Mulu | MulDivOp::Muls => {
                let (result, n) = if op == MulDivOp::Mulu {
                    ((d as u16 as u32) * source as u32, source.count_ones())
                } else {
                    let r = (d as u16 as i16 as i32) * (source as i16 as i32);
                    let pairs = ((source as u32) << 1 ^ source as u32) & 0xFFFF;
                    (r as u32, pairs.count_ones())
                };
                self.regs.d[dst as usize] = result;
                self.set_logic_flags(result, Size::Long);
                self.fetch(bus)?;
                self.idle(MUL_BASE_INTERNAL + 2 * n);
                Ok(())
            }
            MulDivOp::Divu | MulDivOp::Divs => {
                if source == 0 {
                    return self.zero_divide(bus, start_pc);
                }
                let total = if op == MulDivOp::Divu {
                    self.divu(d, source)
                } else {
                    self.divs(d, source)
                };
                self.idle(total - PREFETCH_CYCLES);
                self.fetch(bus).map(drop)
            }
        }
    }

    fn divu(&mut self, dividend: u32, divisor: u16) -> u32 {
        let cycles = divu_cycles(dividend, divisor);
        let quotient = dividend / divisor as u32;
        if quotient > 0xFFFF {
            self.set_div_overflow();
        } else {
            let remainder = dividend % divisor as u32;
            self.store_division(quotient, remainder);
        }
        cycles
    }

    fn divs(&mut self, dividend: u32, divisor: u16) -> u32 {
        let (dividend, divisor) = (dividend as i32, divisor as i16);
        let cycles = divs_cycles(dividend, divisor);
        let quotient = dividend as i64 / divisor as i64;
        if quotient != quotient as i16 as i64 {
            self.set_div_overflow();
        } else {
            let remainder = dividend as i64 % divisor as i64;
            self.store_division(quotient as u32, remainder as u32);
        }
        cycles
    }

    /// Quotient in the low word, remainder in the high word.
    fn store_division(&mut self, quotient: u32, remainder: u32) {
        let dst = ((self.ir >> 9) & 7) as usize;
        self.regs.d[dst] = (remainder << 16) | (quotient & 0xFFFF);
        self.set_logic_flags(quotient, Size::Word);
    }

    /// Overflow: destination unchanged, V set, C cleared. N and Z are
    /// undefined in the PRM; SingleStepTests shows them unchanged (all 1423
    /// overflow cases of DIVU/DIVS).
    fn set_div_overflow(&mut self) {
        let s = self.regs.sr() & !sr::C;
        self.regs.set_sr(s | sr::V);
    }

    /// The stacked PC is the address of the next instruction (M68000 PRM,
    /// DIVU/DIVS: "the processor initiates exception processing").
    fn zero_divide<B: Bus>(&mut self, bus: &mut B, start_pc: u32) -> Res {
        let ext_words = (self.regs.pc.wrapping_sub(start_pc)) / 2;
        let next_instruction = start_pc.wrapping_add(2 + 2 * ext_words);
        let s = self.regs.sr() & !(sr::N | sr::Z | sr::V | sr::C);
        self.regs.set_sr(s);
        self.idle(ZERO_DIVIDE_INTERNAL);
        self.group2_exception(bus, vector::ZERO_DIVIDE, next_instruction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divu_cycle_bounds() {
        assert_eq!(divu_cycles(0x0001_0000, 1), 10); // overflow
        let all = (1..=0xFFFFu32).map(|d| divu_cycles(0x1234, d as u16));
        assert!(all.clone().all(|c| (76..=136).contains(&c)));
    }
}

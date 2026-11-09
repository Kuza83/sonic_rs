//! ABCD/SBCD/NBCD.
//!
//! Result and flags, including the N and V flags the PRM leaves undefined,
//! follow Flamewing's hardware-verified BCD algorithms (documented in his
//! 68000 BCD test suite, and used by MAME/Genesis Plus GX).

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::Operand;
use crate::instruction::{BcdOp, Ea, Size};
use crate::registers::sr;

/// Register forms: internal cycles after the prefetch.
const BCD_REG_INTERNAL: u32 = 2;
/// Memory forms: internal cycles of the predecrement.
const PREDEC_CALC: u32 = 2;

struct BcdResult {
    value: u8,
    carry: bool,
    overflow: bool,
}

fn abcd(xx: u8, yy: u8, x: bool) -> BcdResult {
    let ss = xx.wrapping_add(yy).wrapping_add(x as u8);
    // Binary carries out of each nibble: (s & d) | (~r & d) | (s & ~r).
    let bc = ((xx & yy) | (!ss & xx) | (!ss & yy)) & 0x88;
    // Decimal carries of both nibbles (bit 8 kept by the wider addition).
    let dc = ((((ss as u16 + 0x66) ^ ss as u16) & 0x110) >> 1) as u8;
    let corf = (bc | dc) - ((bc | dc) >> 2);
    let rr = ss.wrapping_add(corf);
    BcdResult {
        value: rr,
        carry: (bc | (ss & !rr)) & 0x80 != 0,
        overflow: (!ss & rr) & 0x80 != 0,
    }
}

fn sbcd(xx: u8, yy: u8, x: bool) -> BcdResult {
    let dd = xx.wrapping_sub(yy).wrapping_sub(x as u8);
    // Binary borrows of each nibble: (s & ~d) | (r & ~d) | (s & r).
    let bc = ((!xx & yy) | (dd & !xx) | (dd & yy)) & 0x88;
    let corf = bc - (bc >> 2);
    let rr = dd.wrapping_sub(corf);
    BcdResult {
        value: rr,
        carry: (bc | (!dd & rr)) & 0x80 != 0,
        overflow: (dd & !rr) & 0x80 != 0,
    }
}

impl Cpu {
    /// X/C from the decimal carry, Z only cleared, N from bit 7, V as computed.
    fn set_bcd_flags(&mut self, r: &BcdResult) {
        let old = self.regs.sr();
        let mut ccr = old & sr::Z;
        if r.value != 0 {
            ccr = 0;
        }
        if r.carry {
            ccr |= sr::X | sr::C;
        }
        if r.overflow {
            ccr |= sr::V;
        }
        if r.value & 0x80 != 0 {
            ccr |= sr::N;
        }
        self.regs.set_sr((old & !sr::CCR) | ccr);
    }

    fn bcd(&mut self, op: BcdOp, dst: u8, src: u8) -> u8 {
        let x = self.regs.sr() & sr::X != 0;
        let r = match op {
            BcdOp::Abcd => abcd(dst, src, x),
            BcdOp::Sbcd => sbcd(dst, src, x),
        };
        self.set_bcd_flags(&r);
        r.value
    }

    pub(super) fn op_bcd<B: Bus>(
        &mut self,
        bus: &mut B,
        op: BcdOp,
        memory: bool,
        rx: u8,
        ry: u8,
    ) -> Res {
        if !memory {
            let (d, s) = (
                self.regs.d[rx as usize] as u8,
                self.regs.d[ry as usize] as u8,
            );
            let value = self.bcd(op, d, s);
            self.write_data_reg(rx, Size::Byte, value as u32);
            self.fetch(bus)?;
            self.idle(BCD_REG_INTERNAL);
            return Ok(());
        }
        self.idle(PREDEC_CALC);
        let src_addr = self.predec_byte(ry);
        let s = self.read(bus, src_addr, Size::Byte)? as u8;
        let dst_addr = self.predec_byte(rx);
        let d = self.read(bus, dst_addr, Size::Byte)? as u8;
        let value = self.bcd(op, d, s);
        self.fetch(bus)?;
        self.write(bus, dst_addr, Size::Byte, value as u32)
    }

    /// -(An) for a byte: A7 stays word aligned.
    fn predec_byte(&mut self, reg: u8) -> u32 {
        let step = if reg == 7 { 2 } else { 1 };
        let addr = self.regs.a[reg as usize].wrapping_sub(step);
        self.regs.a[reg as usize] = addr;
        addr
    }

    pub(super) fn op_nbcd<B: Bus>(&mut self, bus: &mut B, dst: Ea) -> Res {
        match self.resolve(bus, dst, Size::Byte)? {
            Operand::DataReg(r) => {
                let value = self.bcd(BcdOp::Sbcd, 0, self.regs.d[r as usize] as u8);
                self.write_data_reg(r, Size::Byte, value as u32);
                self.fetch(bus)?;
                self.idle(BCD_REG_INTERNAL);
                Ok(())
            }
            Operand::Memory(addr) => {
                let d = self.read(bus, addr, Size::Byte)? as u8;
                let value = self.bcd(BcdOp::Sbcd, 0, d);
                self.fetch(bus)?;
                self.write(bus, addr, Size::Byte, value as u32)
            }
            _ => unreachable!("NBCD destination is data alterable"),
        }
    }
}

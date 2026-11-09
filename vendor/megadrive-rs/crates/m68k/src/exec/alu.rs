//! Arithmetic and logic: ADD/SUB/CMP/AND/OR/EOR in all their forms,
//! ADDX/SUBX, CMPM, NEG/NEGX/NOT and the immediate CCR/SR operations.
//!
//! Flag rules: M68000 PRM §3.7 (condition code computation) and each
//! instruction page. Bus order and internal cycles: SingleStepTests/680x0.

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::{Operand, sign_extend};
use crate::instruction::{AluOp, Ea, Size, UnaryOp};
use crate::registers::sr;

/// Extra internal cycles of long operations ending in a data or address register.
const LONG_REG_SHORT: u32 = 2;
const LONG_REG_FULL: u32 = 4;
/// ANDI/ORI/EORI to CCR/SR: internal cycles before the prefetch queue reload.
const STATUS_IMM_INTERNAL: u32 = 8;
/// Predecrement of ADDX/SUBX memory forms.
const PREDEC_CALC: u32 = 2;

/// Result and condition codes of an ALU operation.
struct AluResult {
    value: u32,
    ccr: u16,
}

fn msb(value: u32, size: Size) -> bool {
    value & size.msb() != 0
}

fn nz(value: u32, size: Size) -> u16 {
    let value = value & size.mask();
    let mut ccr = 0;
    if msb(value, size) {
        ccr |= sr::N;
    }
    if value == 0 {
        ccr |= sr::Z;
    }
    ccr
}

/// dst + src + x, with N/Z/V/C (and X = C).
fn add(dst: u32, src: u32, x: u32, size: Size) -> AluResult {
    let mask = size.mask() as u64;
    let wide = (dst as u64 & mask) + (src as u64 & mask) + x as u64;
    let value = wide as u32 & size.mask();
    let mut ccr = nz(value, size);
    if wide > mask {
        ccr |= sr::C | sr::X;
    }
    if msb((src ^ value) & (dst ^ value), size) {
        ccr |= sr::V;
    }
    AluResult { value, ccr }
}

/// dst - src - x, with N/Z/V/C (and X = C).
fn sub(dst: u32, src: u32, x: u32, size: Size) -> AluResult {
    let mask = size.mask() as u64;
    let (d, s) = (dst as u64 & mask, (src as u64 & mask) + x as u64);
    let value = d.wrapping_sub(s) as u32 & size.mask();
    let mut ccr = nz(value, size);
    if s > d {
        ccr |= sr::C | sr::X;
    }
    if msb((dst ^ src) & (dst ^ value), size) {
        ccr |= sr::V;
    }
    AluResult { value, ccr }
}

impl Cpu {
    fn x_bit(&self) -> u32 {
        (self.regs.sr() & sr::X != 0) as u32
    }

    /// Replaces the bits of `affected` in the CCR with those of `ccr`.
    fn update_ccr(&mut self, affected: u16, ccr: u16) {
        let sr = self.regs.sr();
        self.regs.set_sr((sr & !affected) | (ccr & affected));
    }

    /// Applies `op` and updates the flags; returns the value to write back
    /// (`None` for CMP).
    fn alu(&mut self, op: AluOp, size: Size, dst: u32, src: u32) -> Option<u32> {
        let nzvc = sr::N | sr::Z | sr::V | sr::C;
        match op {
            AluOp::Add => {
                let r = add(dst, src, 0, size);
                self.update_ccr(nzvc | sr::X, r.ccr);
                Some(r.value)
            }
            AluOp::Sub => {
                let r = sub(dst, src, 0, size);
                self.update_ccr(nzvc | sr::X, r.ccr);
                Some(r.value)
            }
            AluOp::Cmp => {
                let r = sub(dst, src, 0, size);
                self.update_ccr(nzvc, r.ccr);
                None
            }
            AluOp::And | AluOp::Or | AluOp::Eor => {
                let value = match op {
                    AluOp::And => dst & src,
                    AluOp::Or => dst | src,
                    _ => dst ^ src,
                } & size.mask();
                self.update_ccr(nzvc, nz(value, size));
                Some(value)
            }
        }
    }

    /// Long operations into a data register spend 2 or 4 extra cycles:
    /// 4 when the source is a register or immediate, 2 from memory
    /// (CMP always 2).
    fn long_to_reg_extra(&mut self, op: AluOp, size: Size, src_in_memory: bool) {
        if size != Size::Long {
            return;
        }
        let extra = if op == AluOp::Cmp || src_in_memory {
            LONG_REG_SHORT
        } else {
            LONG_REG_FULL
        };
        self.idle(extra);
    }

    pub(super) fn op_alu_to_reg<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        src: Ea,
        dst: u8,
    ) -> Res {
        let (src_op, value) = self.resolve_read(bus, src, size)?;
        let d = self.regs.d[dst as usize];
        if let Some(result) = self.alu(op, size, d, value) {
            self.write_data_reg(dst, size, result);
        }
        self.fetch(bus)?;
        self.long_to_reg_extra(op, size, matches!(src_op, Operand::Memory(_)));
        Ok(())
    }

    /// Read-modify-write of a memory operand: read, prefetch, write back
    /// (long results low word first).
    fn rmw_memory<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        modify: impl FnOnce(&mut Self, u32) -> Option<u32>,
    ) -> Res {
        let old = self.read(bus, addr, size)?;
        let new = modify(self, old);
        self.fetch(bus)?;
        if let Some(new) = new {
            self.write_low_first(bus, addr, size, new)?;
        }
        Ok(())
    }

    pub(super) fn op_alu_to_ea<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        src: u8,
        dst: Ea,
    ) -> Res {
        let s = self.regs.d[src as usize];
        match self.resolve(bus, dst, size)? {
            Operand::DataReg(r) => {
                // EOR Dn,Dn
                let d = self.regs.d[r as usize];
                if let Some(result) = self.alu(op, size, d, s) {
                    self.write_data_reg(r, size, result);
                }
                self.fetch(bus)?;
                self.long_to_reg_extra(op, size, false);
                Ok(())
            }
            Operand::Memory(addr) => {
                self.rmw_memory(bus, addr, size, |cpu, d| cpu.alu(op, size, d, s))
            }
            _ => unreachable!("Dn,<ea> destination is data alterable"),
        }
    }

    pub(super) fn op_alu_imm<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        dst: Ea,
    ) -> Res {
        let Operand::Immediate(imm) = self.resolve(bus, Ea::Immediate, size)? else {
            unreachable!()
        };
        match self.resolve(bus, dst, size)? {
            Operand::DataReg(r) => {
                let d = self.regs.d[r as usize];
                if let Some(result) = self.alu(op, size, d, imm) {
                    self.write_data_reg(r, size, result);
                }
                self.fetch(bus)?;
                self.long_to_reg_extra(op, size, false);
                Ok(())
            }
            Operand::Memory(addr) => {
                self.rmw_memory(bus, addr, size, |cpu, d| cpu.alu(op, size, d, imm))
            }
            _ => unreachable!("immediate destination is data alterable"),
        }
    }

    pub(super) fn op_alu_quick<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        data: u8,
        dst: Ea,
    ) -> Res {
        let data = data as u32;
        match self.resolve(bus, dst, size)? {
            Operand::DataReg(r) => {
                let d = self.regs.d[r as usize];
                if let Some(result) = self.alu(op, size, d, data) {
                    self.write_data_reg(r, size, result);
                }
                self.fetch(bus)?;
                if size == Size::Long {
                    self.idle(LONG_REG_FULL);
                }
                Ok(())
            }
            Operand::AddrReg(r) => {
                // Always a 32-bit operation, flags untouched.
                let a = self.regs.a[r as usize];
                self.regs.a[r as usize] = match op {
                    AluOp::Add => a.wrapping_add(data),
                    _ => a.wrapping_sub(data),
                };
                self.fetch(bus)?;
                // Observed: 4 extra cycles for .w, 2 for .l.
                self.idle(if size == Size::Word {
                    LONG_REG_FULL
                } else {
                    LONG_REG_SHORT
                });
                Ok(())
            }
            Operand::Memory(addr) => {
                self.rmw_memory(bus, addr, size, |cpu, d| cpu.alu(op, size, d, data))
            }
            Operand::Immediate(_) => unreachable!("quick destination is alterable"),
        }
    }

    /// ADDA/SUBA/CMPA: word sources are sign-extended, the operation is 32-bit.
    pub(super) fn op_alu_addr<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        src: Ea,
        dst: u8,
    ) -> Res {
        let (src_op, value) = self.resolve_read(bus, src, size)?;
        let value = sign_extend(value, size);
        let a = self.regs.a[dst as usize];
        match op {
            AluOp::Add => self.regs.a[dst as usize] = a.wrapping_add(value),
            AluOp::Sub => self.regs.a[dst as usize] = a.wrapping_sub(value),
            _ => {
                self.alu(AluOp::Cmp, Size::Long, a, value);
            }
        }
        self.fetch(bus)?;
        let in_memory = matches!(src_op, Operand::Memory(_));
        let extra = match op {
            AluOp::Cmp => LONG_REG_SHORT,
            _ if size == Size::Long && in_memory => LONG_REG_SHORT,
            _ => LONG_REG_FULL,
        };
        self.idle(extra);
        Ok(())
    }

    /// ADDX/SUBX: Z is only cleared (never set), so multi-precision chains work.
    fn alu_extended(&mut self, op: AluOp, size: Size, dst: u32, src: u32) -> u32 {
        let x = self.x_bit();
        let r = if op == AluOp::Add {
            add(dst, src, x, size)
        } else {
            sub(dst, src, x, size)
        };
        let keep_z = self.regs.sr() & sr::Z;
        let ccr = (r.ccr & !sr::Z) | if r.value == 0 { keep_z } else { 0 };
        self.update_ccr(sr::X | sr::N | sr::Z | sr::V | sr::C, ccr);
        r.value
    }

    /// Predecrement read for ADDX/SUBX: long operands are read low word first.
    fn read_predec<B: Bus>(&mut self, bus: &mut B, reg: u8, size: Size) -> Res<(u32, u32)> {
        let r = reg as usize;
        if size == Size::Long {
            let lo_addr = self.regs.a[r].wrapping_sub(2);
            self.regs.a[r] = lo_addr;
            let lo = self.read_word(bus, lo_addr)? as u32;
            let hi_addr = lo_addr.wrapping_sub(2);
            self.regs.a[r] = hi_addr;
            let hi = self.read_word(bus, hi_addr)? as u32;
            Ok((hi_addr, (hi << 16) | lo))
        } else {
            let step = if reg == 7 && size == Size::Byte {
                2
            } else {
                size.bytes()
            };
            let addr = self.regs.a[r].wrapping_sub(step);
            self.regs.a[r] = addr;
            Ok((addr, self.read(bus, addr, size)?))
        }
    }

    pub(super) fn op_alu_x<B: Bus>(
        &mut self,
        bus: &mut B,
        op: AluOp,
        size: Size,
        memory: bool,
        rx: u8,
        ry: u8,
    ) -> Res {
        if !memory {
            let (d, s) = (self.regs.d[rx as usize], self.regs.d[ry as usize]);
            let value = self.alu_extended(op, size, d, s);
            self.write_data_reg(rx, size, value);
            self.fetch(bus)?;
            if size == Size::Long {
                self.idle(LONG_REG_FULL);
            }
            return Ok(());
        }
        self.idle(PREDEC_CALC);
        let (_, s) = self.read_predec(bus, ry, size)?;
        let (addr, d) = self.read_predec(bus, rx, size)?;
        let value = self.alu_extended(op, size, d, s);
        if size == Size::Long {
            // Low word, prefetch, high word.
            self.write_word(bus, addr.wrapping_add(2), value as u16)?;
            self.fetch(bus)?;
            self.write_word(bus, addr, (value >> 16) as u16)
        } else {
            self.fetch(bus)?;
            self.write(bus, addr, size, value)
        }
    }

    pub(super) fn op_cmpm<B: Bus>(&mut self, bus: &mut B, size: Size, ax: u8, ay: u8) -> Res {
        let (_, s) = self.resolve_read(bus, Ea::PostInc(ay), size)?;
        let (_, d) = self.resolve_read(bus, Ea::PostInc(ax), size)?;
        self.alu(AluOp::Cmp, size, d, s);
        self.fetch(bus).map(drop)
    }

    fn unary(&mut self, op: UnaryOp, size: Size, value: u32) -> u32 {
        match op {
            UnaryOp::Neg => {
                let r = sub(0, value, 0, size);
                self.update_ccr(sr::X | sr::N | sr::Z | sr::V | sr::C, r.ccr);
                r.value
            }
            UnaryOp::Negx => self.alu_extended(AluOp::Sub, size, 0, value),
            UnaryOp::Not => {
                let r = !value & size.mask();
                self.update_ccr(sr::N | sr::Z | sr::V | sr::C, nz(r, size));
                r
            }
        }
    }

    pub(super) fn op_unary<B: Bus>(
        &mut self,
        bus: &mut B,
        op: UnaryOp,
        size: Size,
        dst: Ea,
    ) -> Res {
        match self.resolve(bus, dst, size)? {
            Operand::DataReg(r) => {
                let value = self.unary(op, size, self.regs.d[r as usize]);
                self.write_data_reg(r, size, value);
                self.fetch(bus)?;
                if size == Size::Long {
                    self.idle(LONG_REG_SHORT);
                }
                Ok(())
            }
            Operand::Memory(addr) => {
                self.rmw_memory(bus, addr, size, |cpu, d| Some(cpu.unary(op, size, d)))
            }
            _ => unreachable!("unary destination is data alterable"),
        }
    }

    fn logic16(op: AluOp, a: u16, b: u16) -> u16 {
        match op {
            AluOp::And => a & b,
            AluOp::Or => a | b,
            _ => a ^ b,
        }
    }

    /// After a status register write the 68000 reloads its whole prefetch
    /// queue from the next instruction.
    pub(super) fn finish_status_write<B: Bus>(&mut self, bus: &mut B, internal: u32) -> Res {
        self.idle(internal);
        self.regs.pc = self.regs.pc.wrapping_add(2);
        self.prefetch[0] = self.read_program(bus, self.regs.pc)?;
        self.prefetch[1] = self.read_program(bus, self.regs.pc.wrapping_add(2))?;
        Ok(())
    }

    pub(super) fn op_ccr_imm<B: Bus>(&mut self, bus: &mut B, op: AluOp) -> Res {
        let imm = self.fetch(bus)? & sr::CCR;
        let sr = self.regs.sr();
        let ccr = Self::logic16(op, sr & sr::CCR, imm);
        self.regs.set_sr((sr & !sr::CCR) | ccr);
        self.finish_status_write(bus, STATUS_IMM_INTERNAL)
    }

    pub(super) fn op_sr_imm<B: Bus>(&mut self, bus: &mut B, op: AluOp) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        let imm = self.fetch(bus)?;
        let sr = Self::logic16(op, self.regs.sr(), imm);
        self.regs.set_sr(sr);
        self.finish_status_write(bus, STATUS_IMM_INTERNAL)
    }
}

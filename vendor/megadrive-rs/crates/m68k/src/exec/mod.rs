//! Instruction execution, one module per family.

mod alu;
mod bcd;
mod bit;
mod data_move;
mod flow;
pub(crate) mod muldiv;
mod shift;
mod system;
mod trap;

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::exceptions::vector;
use crate::instruction::{Instruction, Size};
use crate::registers::sr;

impl Cpu {
    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B, instr: Instruction) -> Res {
        use Instruction::*;
        match instr {
            Illegal => self.instruction_exception(bus, vector::ILLEGAL_INSTRUCTION),
            LineA => self.instruction_exception(bus, vector::LINE_A),
            LineF => self.instruction_exception(bus, vector::LINE_F),
            Trap { vector } => self.op_trap(bus, vector),
            Trapv => self.op_trapv(bus),
            Chk { src, dst } => self.op_chk(bus, src, dst),
            Reset => self.op_reset(bus),
            Stop => self.op_stop(bus),
            Nop => self.fetch(bus).map(drop),
            Move { size, src, dst } => self.op_move(bus, size, src, dst),
            Movea { size, src, dst } => self.op_movea(bus, size, src, dst),
            Moveq { value, dst } => self.op_moveq(bus, value, dst),
            Lea { src, dst } => self.op_lea(bus, src, dst),
            Pea { src } => self.op_pea(bus, src),
            Clr { size, dst } => self.op_clr(bus, size, dst),
            Tst { size, dst } => self.op_tst(bus, size, dst),
            Exg { mode, rx, ry } => self.op_exg(bus, mode, rx, ry),
            Swap { reg } => self.op_swap(bus, reg),
            Ext { size, reg } => self.op_ext(bus, size, reg),
            AluToReg { op, size, src, dst } => self.op_alu_to_reg(bus, op, size, src, dst),
            AluToEa { op, size, src, dst } => self.op_alu_to_ea(bus, op, size, src, dst),
            AluImm { op, size, dst } => self.op_alu_imm(bus, op, size, dst),
            AluQuick {
                op,
                size,
                data,
                dst,
            } => self.op_alu_quick(bus, op, size, data, dst),
            AluAddr { op, size, src, dst } => self.op_alu_addr(bus, op, size, src, dst),
            AluX {
                op,
                size,
                memory,
                rx,
                ry,
            } => self.op_alu_x(bus, op, size, memory, rx, ry),
            Cmpm { size, ax, ay } => self.op_cmpm(bus, size, ax, ay),
            Unary { op, size, dst } => self.op_unary(bus, op, size, dst),
            CcrImm { op } => self.op_ccr_imm(bus, op),
            SrImm { op } => self.op_sr_imm(bus, op),
            Shift {
                kind,
                left,
                size,
                count,
                reg,
            } => self.op_shift(bus, kind, left, size, count, reg),
            ShiftMem { kind, left, dst } => self.op_shift_mem(bus, kind, left, dst),
            Bit { op, bit, dst } => self.op_bit(bus, op, bit, dst),
            Scc { cond, dst } => self.op_scc(bus, cond, dst),
            Tas { dst } => self.op_tas(bus, dst),
            Bcc { cond, disp } => self.op_bcc(bus, cond, disp),
            Bsr { disp } => self.op_bsr(bus, disp),
            Dbcc { cond, reg } => self.op_dbcc(bus, cond, reg),
            Jmp { dst } => self.op_jmp(bus, dst),
            Jsr { dst } => self.op_jsr(bus, dst),
            Rts => self.op_rts(bus),
            Rtr => self.op_rtr(bus),
            Rte => self.op_rte(bus),
            Link { reg } => self.op_link(bus, reg),
            Unlk { reg } => self.op_unlk(bus, reg),
            Movem {
                size,
                to_memory,
                ea,
            } => self.op_movem(bus, size, to_memory, ea),
            Movep {
                size,
                to_memory,
                dreg,
                areg,
            } => self.op_movep(bus, size, to_memory, dreg, areg),
            MoveFromSr { dst } => self.op_move_from_sr(bus, dst),
            MoveToCcr { src } => self.op_move_to_ccr(bus, src),
            MoveToSr { src } => self.op_move_to_sr(bus, src),
            MoveUsp { to_usp, reg } => self.op_move_usp(bus, to_usp, reg),
            MulDiv { op, src, dst } => self.op_muldiv(bus, op, src, dst),
            Bcd { op, memory, rx, ry } => self.op_bcd(bus, op, memory, rx, ry),
            Nbcd { dst } => self.op_nbcd(bus, dst),
        }
    }

    /// N and Z from `value`, V and C cleared, X untouched (logical results).
    pub(crate) fn set_logic_flags(&mut self, value: u32, size: Size) {
        let value = value & size.mask();
        let mut ccr = self.regs.sr() & !(sr::N | sr::Z | sr::V | sr::C);
        if value & size.msb() != 0 {
            ccr |= sr::N;
        }
        if value == 0 {
            ccr |= sr::Z;
        }
        self.regs.set_sr(ccr);
    }
}

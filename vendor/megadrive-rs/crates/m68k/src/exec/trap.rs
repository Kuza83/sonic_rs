//! TRAP, TRAPV, CHK, RESET, STOP and the opcodes that only raise exceptions.

use crate::bus::Bus;
use crate::cpu::{Cpu, CpuState, Res};
use crate::exceptions::vector;
use crate::instruction::{Ea, Size};
use crate::registers::sr;

/// TRAP: internal cycles before the exception frame.
const TRAP_INTERNAL: u32 = 4;
/// CHK: comparison time when Dn is above the bound, and otherwise.
const CHK_ABOVE_BOUND: u32 = 4;
const CHK_DEFAULT: u32 = 6;
/// RESET instruction: 132 cycles, the RESET line being asserted for 124.
const RESET_INTERNAL: u32 = 128;
/// STOP: opcode + immediate word, 4 internal cycles.
const STOP_LENGTH: u32 = 4;
const STOP_INTERNAL: u32 = 4;

impl Cpu {
    pub(super) fn op_trap<B: Bus>(&mut self, bus: &mut B, n: u8) -> Res {
        self.idle(TRAP_INTERNAL);
        let next = self.regs.pc.wrapping_add(2);
        self.group2_exception(bus, vector::TRAP_BASE + n as u32, next)
    }

    pub(super) fn op_trapv<B: Bus>(&mut self, bus: &mut B) -> Res {
        self.fetch(bus)?;
        if self.regs.sr() & sr::V != 0 {
            let next = self.regs.pc;
            return self.group2_exception(bus, vector::TRAPV, next);
        }
        Ok(())
    }

    /// CHK: traps when Dn < 0 or Dn > bound (signed words). Flags observed in
    /// SingleStepTests: V = C = 0, Z = (Dn == 0); N = 1 when Dn < 0, N = 0
    /// when Dn > bound, unchanged otherwise.
    pub(super) fn op_chk<B: Bus>(&mut self, bus: &mut B, src: Ea, dst: u8) -> Res {
        let (_, bound) = self.resolve_read(bus, src, Size::Word)?;
        self.fetch(bus)?;
        let value = self.regs.d[dst as usize] as u16 as i16;
        let bound = bound as u16 as i16;
        let old = self.regs.sr();
        let mut ccr = old & sr::X;
        if value == 0 {
            ccr |= sr::Z;
        }
        let above = value > bound;
        let negative = value < 0;
        if negative || (!above && old & sr::N != 0) {
            ccr |= sr::N;
        }
        self.regs.set_sr((old & !sr::CCR) | ccr);
        if above || negative {
            self.idle(if above { CHK_ABOVE_BOUND } else { CHK_DEFAULT });
            let next = self.regs.pc;
            return self.group2_exception(bus, vector::CHK, next);
        }
        self.idle(CHK_DEFAULT);
        Ok(())
    }

    /// RESET asserts the external RESET line; the CPU state is unchanged.
    pub(super) fn op_reset<B: Bus>(&mut self, bus: &mut B) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        self.idle(RESET_INTERNAL);
        self.fetch(bus).map(drop)
    }

    /// STOP #imm: loads SR and waits for an interrupt above the new mask.
    /// 4 cycles, no bus access (M68000 UM Table 8-12: 4(0/0)): the immediate
    /// is taken from IRC and PC is left past it. The prefetch queue is not
    /// refilled — only an interrupt or a reset leaves the stopped state, and
    /// both reload it.
    pub(super) fn op_stop<B: Bus>(&mut self, bus: &mut B) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        let status = self.irc();
        self.regs.pc = self.regs.pc.wrapping_add(STOP_LENGTH);
        self.idle(STOP_INTERNAL);
        self.regs.set_sr(status);
        self.state = CpuState::Stopped;
        Ok(())
    }
}

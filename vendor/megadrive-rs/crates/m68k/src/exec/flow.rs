//! Program flow and stack frames: Bcc/BRA/BSR, DBcc, JMP/JSR, RTS/RTR/RTE,
//! LINK/UNLK. Bus order and internal cycles: SingleStepTests/680x0.

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::sign_extend;
use crate::instruction::{Ea, Size};
use crate::registers::sr;

/// Internal cycles before a taken branch (Bcc, BSR, DBcc loop).
const BRANCH_TAKEN: u32 = 2;
/// Internal cycles of a Bcc not taken / DBcc with condition true.
const BRANCH_NOT_TAKEN: u32 = 4;
/// DBcc with an expired counter: 14 cycles, 3 reads (M68000 UM §8, DBcc).
/// The address of the middle read is not covered by any test; it is
/// modelled as internal time.
const DBCC_EXPIRED_INTERNAL: u32 = 6;
/// JMP/JSR address calculation time for each mode (no extension prefetch).
const JUMP_DISP_CALC: u32 = 2;
const JUMP_INDEX_CALC: u32 = 6;

impl Cpu {
    /// Word displacement of a branch: taken straight from IRC.
    fn branch_target(&self, disp: i8) -> u32 {
        let base = self.regs.pc.wrapping_add(2);
        let offset = if disp == 0 {
            sign_extend(self.irc() as u32, Size::Word)
        } else {
            disp as i32 as u32
        };
        base.wrapping_add(offset)
    }

    /// Skips the branch displacement words when no branch happens.
    fn branch_skip<B: Bus>(&mut self, bus: &mut B, word_disp: bool) -> Res {
        self.fetch(bus)?;
        if word_disp {
            self.fetch(bus)?;
        }
        Ok(())
    }

    pub(super) fn op_bcc<B: Bus>(&mut self, bus: &mut B, cond: u8, disp: i8) -> Res {
        if self.condition(cond) {
            self.idle(BRANCH_TAKEN);
            let target = self.branch_target(disp);
            self.branch_to(bus, target)
        } else {
            self.idle(BRANCH_NOT_TAKEN);
            self.branch_skip(bus, disp == 0)
        }
    }

    /// Pushes a long, high word first (the order used by BSR/JSR/PEA/LINK).
    fn push_long<B: Bus>(&mut self, bus: &mut B, value: u32) -> Res {
        let sp = self.regs.a[7].wrapping_sub(4);
        self.regs.a[7] = sp;
        self.write(bus, sp, Size::Long, value)
    }

    pub(super) fn op_bsr<B: Bus>(&mut self, bus: &mut B, disp: i8) -> Res {
        self.idle(BRANCH_TAKEN);
        let ext_words = if disp == 0 { 2 } else { 0 };
        let ret = self.regs.pc.wrapping_add(2 + ext_words);
        let target = self.branch_target(disp);
        self.push_long(bus, ret)?;
        self.branch_to(bus, target)
    }

    pub(super) fn op_dbcc<B: Bus>(&mut self, bus: &mut B, cond: u8, reg: u8) -> Res {
        if self.condition(cond) {
            self.idle(BRANCH_NOT_TAKEN);
            return self.branch_skip(bus, true);
        }
        let counter = (self.regs.d[reg as usize] as u16).wrapping_sub(1);
        self.write_data_reg(reg, Size::Word, counter as u32);
        if counter != 0xFFFF {
            self.idle(BRANCH_TAKEN);
            let target = self.branch_target(0);
            self.branch_to(bus, target)
        } else {
            self.idle(DBCC_EXPIRED_INTERNAL);
            self.branch_skip(bus, true)
        }
    }

    /// Target of JMP/JSR: the last extension word is used from IRC without
    /// being prefetched. Returns (target, number of extension words).
    fn jump_target<B: Bus>(&mut self, bus: &mut B, ea: Ea) -> Res<(u32, u32)> {
        let irc = self.irc() as u32;
        let ext_base = self.regs.pc.wrapping_add(2);
        Ok(match ea {
            Ea::Indirect(r) => (self.regs.a[r as usize], 0),
            Ea::Disp16(r) => {
                self.idle(JUMP_DISP_CALC);
                let base = self.regs.a[r as usize];
                (base.wrapping_add(sign_extend(irc, Size::Word)), 1)
            }
            Ea::Index8(r) => {
                self.idle(JUMP_INDEX_CALC);
                let base = self.regs.a[r as usize];
                (self.brief_ext_address(base, irc as u16), 1)
            }
            Ea::AbsShort => {
                self.idle(JUMP_DISP_CALC);
                (sign_extend(irc, Size::Word), 1)
            }
            Ea::AbsLong => {
                let hi = self.fetch(bus)? as u32;
                ((hi << 16) | self.irc() as u32, 2)
            }
            Ea::PcDisp16 => {
                self.idle(JUMP_DISP_CALC);
                (ext_base.wrapping_add(sign_extend(irc, Size::Word)), 1)
            }
            Ea::PcIndex8 => {
                self.idle(JUMP_INDEX_CALC);
                (self.brief_ext_address(ext_base, irc as u16), 1)
            }
            _ => unreachable!("JMP/JSR take control addressing modes"),
        })
    }

    pub(super) fn op_jmp<B: Bus>(&mut self, bus: &mut B, dst: Ea) -> Res {
        let (target, _) = self.jump_target(bus, dst)?;
        self.branch_to(bus, target)
    }

    /// JSR fetches the first target word, pushes the return address, then
    /// fetches the second target word.
    pub(super) fn op_jsr<B: Bus>(&mut self, bus: &mut B, dst: Ea) -> Res {
        let start = self.regs.pc;
        let (target, ext_words) = self.jump_target(bus, dst)?;
        let ret = start.wrapping_add(2 + 2 * ext_words);
        self.check_branch_target(target)?;
        let first = self.read_program(bus, target)?;
        self.regs.pc = target;
        self.push_long(bus, ret)?;
        self.prefetch[0] = first;
        self.prefetch[1] = self.read_program(bus, target.wrapping_add(2))?;
        Ok(())
    }

    pub(super) fn op_rts<B: Bus>(&mut self, bus: &mut B) -> Res {
        let sp = self.regs.a[7];
        let target = self.read(bus, sp, Size::Long)?;
        self.regs.a[7] = sp.wrapping_add(4);
        self.branch_to(bus, target)
    }

    /// Pops a status word and a PC in the 68000 order: PC high, status, PC low.
    fn pop_status_and_pc<B: Bus>(&mut self, bus: &mut B) -> Res<(u16, u32)> {
        let sp = self.regs.a[7];
        let pc_hi = self.read_word(bus, sp.wrapping_add(2))? as u32;
        let status = self.read_word(bus, sp)?;
        let pc_lo = self.read_word(bus, sp.wrapping_add(4))? as u32;
        self.regs.a[7] = sp.wrapping_add(6);
        Ok((status, (pc_hi << 16) | pc_lo))
    }

    pub(super) fn op_rtr<B: Bus>(&mut self, bus: &mut B) -> Res {
        let (ccr, target) = self.pop_status_and_pc(bus)?;
        let sr = self.regs.sr();
        self.regs.set_sr((sr & !sr::CCR) | (ccr & sr::CCR));
        self.branch_to(bus, target)
    }

    pub(super) fn op_rte<B: Bus>(&mut self, bus: &mut B) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        let (status, target) = self.pop_status_and_pc(bus)?;
        self.regs.set_sr(status);
        self.branch_to(bus, target)
    }

    pub(super) fn op_link<B: Bus>(&mut self, bus: &mut B, reg: u8) -> Res {
        let disp = sign_extend(self.fetch(bus)? as u32, Size::Word);
        let sp = self.regs.a[7].wrapping_sub(4);
        self.regs.a[7] = sp;
        // LINK A7 pushes the already decremented stack pointer.
        let value = self.regs.a[reg as usize];
        self.write(bus, sp, Size::Long, value)?;
        self.regs.a[reg as usize] = sp;
        self.regs.a[7] = self.regs.a[7].wrapping_add(disp);
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_unlk<B: Bus>(&mut self, bus: &mut B, reg: u8) -> Res {
        let frame = self.regs.a[reg as usize];
        let value = self.read(bus, frame, Size::Long)?;
        self.regs.a[7] = frame.wrapping_add(4);
        self.regs.a[reg as usize] = value;
        self.fetch(bus).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use crate::bus::{Bus, FlatBus};
    use crate::cpu::Cpu;
    use crate::registers::sr;

    /// DBcc with condition false and Dn.w = 0: falls through after 14 cycles
    /// (M68000 UM §8, DBcc "branch not taken, count expired": 14(3/0)).
    /// SingleStepTests never hits this case (random registers).
    #[test]
    fn dbcc_expired_counter_falls_through() {
        let mut bus = FlatBus::new();
        bus.write16(0x1004, 0x4E71);
        bus.write16(0x1006, 0x4E71);
        let mut cpu = Cpu::new();
        cpu.regs.set_sr(sr::S);
        cpu.regs.pc = 0x1000;
        cpu.prefetch = [0x51C8, 0xFFFE]; // dbf d0,$1000
        cpu.regs.d[0] = 0x1234_0000;
        assert_eq!(cpu.step(&mut bus), 14);
        assert_eq!(cpu.regs.d[0], 0x1234_FFFF);
        assert_eq!(cpu.regs.pc, 0x1004);
        assert_eq!(cpu.prefetch, [0x4E71, 0x4E71]);
    }
}

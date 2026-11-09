//! Unprefixed instructions (and their DD/FD forms, through `Index`).
//!
//! Timings: Zilog UM0080 instruction tables; internal cycles placed so the
//! totals match SingleStepTests/z80. MEMPTR (WZ) rules: "MEMPTR, esoteric
//! register of the Zilog Z80 CPU" (boo_boo / Vladimir Kladov), as observed by
//! the tests.

use super::{add16, alu8, dec8, inc8, sz_xy};
use crate::bus::Bus;
use crate::cpu::{Cpu, Index};
use crate::decode::decode_base;
use crate::instruction::Instruction;
use crate::registers::flag;

/// Register codes.
const REG_H: u8 = 4;
const REG_L: u8 = 5;
const REG_MEM: u8 = 6;
/// Internal cycles after the displacement of an (IX+d) operand.
const INDEX_DISP_INTERNAL: u32 = 5;

impl Cpu {
    // --- operand helpers -----------------------------------------------------

    pub(crate) fn index_value(&self, idx: Index) -> u16 {
        match idx {
            Index::Hl => self.regs.hl(),
            Index::Ix => self.regs.ix,
            Index::Iy => self.regs.iy,
        }
    }

    pub(crate) fn set_index_value(&mut self, idx: Index, value: u16) {
        match idx {
            Index::Hl => self.regs.set_hl(value),
            Index::Ix => self.regs.ix = value,
            Index::Iy => self.regs.iy = value,
        }
    }

    /// 8-bit register `r` (not 6); H/L read IXH/IXL or IYH/IYL under a prefix.
    pub(crate) fn reg8(&self, r: u8, idx: Index) -> u8 {
        match r {
            0 => self.regs.b,
            1 => self.regs.c,
            2 => self.regs.d,
            3 => self.regs.e,
            REG_H => (self.index_value(idx) >> 8) as u8,
            REG_L => self.index_value(idx) as u8,
            _ => self.regs.a,
        }
    }

    pub(crate) fn set_reg8(&mut self, r: u8, idx: Index, v: u8) {
        match r {
            0 => self.regs.b = v,
            1 => self.regs.c = v,
            2 => self.regs.d = v,
            3 => self.regs.e = v,
            REG_H => {
                let w = self.index_value(idx);
                self.set_index_value(idx, (w & 0x00FF) | ((v as u16) << 8));
            }
            REG_L => {
                let w = self.index_value(idx);
                self.set_index_value(idx, (w & 0xFF00) | v as u16);
            }
            _ => self.regs.a = v,
        }
    }

    /// Address of the (HL) operand, or (IX+d)/(IY+d): the displacement is
    /// fetched, then `internal` cycles pass; WZ takes the indexed address.
    pub(crate) fn mem_operand<B: Bus>(&mut self, bus: &mut B, idx: Index, internal: u32) -> u16 {
        if idx == Index::Hl {
            return self.regs.hl();
        }
        let d = self.fetch_byte(bus) as i8;
        self.idle(internal);
        let addr = self.index_value(idx).wrapping_add(d as i16 as u16);
        self.regs.wz = addr;
        addr
    }

    pub(crate) fn rp(&self, p: u8, idx: Index) -> u16 {
        match p {
            0 => self.regs.bc(),
            1 => self.regs.de(),
            2 => self.index_value(idx),
            _ => self.regs.sp,
        }
    }

    pub(crate) fn set_rp(&mut self, p: u8, idx: Index, v: u16) {
        match p {
            0 => self.regs.set_bc(v),
            1 => self.regs.set_de(v),
            2 => self.set_index_value(idx, v),
            _ => self.regs.sp = v,
        }
    }

    fn rp2(&self, p: u8, idx: Index) -> u16 {
        if p == 3 {
            self.regs.af()
        } else {
            self.rp(p, idx)
        }
    }

    fn set_rp2(&mut self, p: u8, idx: Index, v: u16) {
        if p == 3 {
            self.regs.set_af(v);
        } else {
            self.set_rp(p, idx, v);
        }
    }

    pub(crate) fn condition(&self, cc: u8) -> bool {
        let f = self.regs.f;
        match cc & 7 {
            0 => f & flag::Z == 0,
            1 => f & flag::Z != 0,
            2 => f & flag::C == 0,
            3 => f & flag::C != 0,
            4 => f & flag::PV == 0,
            5 => f & flag::PV != 0,
            6 => f & flag::S == 0,
            _ => f & flag::S != 0,
        }
    }

    fn relative_jump<B: Bus>(&mut self, bus: &mut B, taken: bool) {
        let d = self.fetch_byte(bus) as i8;
        if taken {
            self.idle(5);
            self.regs.pc = self.regs.pc.wrapping_add(d as i16 as u16);
            self.regs.wz = self.regs.pc;
        }
    }

    /// Undocumented X/Y of SCF/CCF: (Q ^ F | A) bits 3/5, with Q the flags
    /// written by the previous instruction (Patrik Rak's findings).
    fn scf_ccf_xy(&self) -> u8 {
        ((self.regs.q ^ self.regs.f) | self.regs.a) & (flag::X | flag::Y)
    }

    // --- execution -----------------------------------------------------------

    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B, instr: Instruction, idx: Index) {
        use Instruction::*;
        match instr {
            Unimplemented => self.halted = true,
            Nop => {}
            ExAfAf => {
                let af = self.regs.af();
                self.regs.set_af(self.regs.af_alt);
                self.regs.af_alt = af;
            }
            Djnz => {
                self.idle(1);
                self.regs.b = self.regs.b.wrapping_sub(1);
                let taken = self.regs.b != 0;
                self.relative_jump(bus, taken);
            }
            Jr => self.relative_jump(bus, true),
            JrCc(cc) => {
                let taken = self.condition(cc);
                self.relative_jump(bus, taken);
            }
            LdRpNn(p) => {
                let v = self.fetch_word(bus);
                self.set_rp(p, idx, v);
            }
            AddHlRp(p) => {
                self.idle(7);
                let hl = self.index_value(idx);
                let (r, f) = add16(hl, self.rp(p, idx), self.regs.f);
                self.regs.wz = hl.wrapping_add(1);
                self.set_index_value(idx, r);
                self.set_f(f);
            }
            LdIndirectA(p) => {
                let addr = self.rp(p, idx);
                self.write(bus, addr, self.regs.a);
                self.regs.wz = ((self.regs.a as u16) << 8) | (addr.wrapping_add(1) & 0xFF);
            }
            LdAIndirect(p) => {
                let addr = self.rp(p, idx);
                self.regs.a = self.read(bus, addr);
                self.regs.wz = addr.wrapping_add(1);
            }
            LdNnHl => {
                let nn = self.fetch_word(bus);
                self.write_word(bus, nn, self.index_value(idx));
                self.regs.wz = nn.wrapping_add(1);
            }
            LdHlNn => {
                let nn = self.fetch_word(bus);
                let v = self.read_word(bus, nn);
                self.set_index_value(idx, v);
                self.regs.wz = nn.wrapping_add(1);
            }
            LdNnA => {
                let nn = self.fetch_word(bus);
                self.write(bus, nn, self.regs.a);
                self.regs.wz = ((self.regs.a as u16) << 8) | (nn.wrapping_add(1) & 0xFF);
            }
            LdANn => {
                let nn = self.fetch_word(bus);
                self.regs.a = self.read(bus, nn);
                self.regs.wz = nn.wrapping_add(1);
            }
            IncRp(p) | DecRp(p) => {
                self.idle(2);
                let v = self.rp(p, idx);
                let v = if matches!(instr, IncRp(_)) {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.set_rp(p, idx, v);
            }
            IncR(r) | DecR(r) => {
                let op = if matches!(instr, IncR(_)) { inc8 } else { dec8 };
                if r == REG_MEM {
                    let addr = self.mem_operand(bus, idx, INDEX_DISP_INTERNAL);
                    let v = self.read(bus, addr);
                    self.idle(1);
                    let (v, f) = op(v, self.regs.f);
                    self.write(bus, addr, v);
                    self.set_f(f);
                } else {
                    let (v, f) = op(self.reg8(r, idx), self.regs.f);
                    self.set_reg8(r, idx, v);
                    self.set_f(f);
                }
            }
            LdRN(r) => {
                if r == REG_MEM {
                    if idx == Index::Hl {
                        let n = self.fetch_byte(bus);
                        self.write(bus, self.regs.hl(), n);
                    } else {
                        // LD (IX+d),n: d, n, then 2 internal cycles.
                        let addr = self.mem_operand(bus, idx, 0);
                        let n = self.fetch_byte(bus);
                        self.idle(2);
                        self.write(bus, addr, n);
                    }
                } else {
                    let n = self.fetch_byte(bus);
                    self.set_reg8(r, idx, n);
                }
            }
            Rlca | Rrca | Rla | Rra => {
                let a = self.regs.a;
                let c_in = self.regs.f & flag::C;
                let (r, c) = match instr {
                    Rlca => (a.rotate_left(1), a >> 7),
                    Rrca => (a.rotate_right(1), a & 1),
                    Rla => ((a << 1) | c_in, a >> 7),
                    _ => ((a >> 1) | (c_in << 7), a & 1),
                };
                self.regs.a = r;
                let keep = self.regs.f & (flag::S | flag::Z | flag::PV);
                self.set_f(keep | (r & (flag::X | flag::Y)) | c);
            }
            Daa => self.daa(),
            Cpl => {
                self.regs.a = !self.regs.a;
                let keep = self.regs.f & (flag::S | flag::Z | flag::PV | flag::C);
                self.set_f(keep | flag::H | flag::N | (self.regs.a & (flag::X | flag::Y)));
            }
            Scf => {
                let keep = self.regs.f & (flag::S | flag::Z | flag::PV);
                let xy = self.scf_ccf_xy();
                self.set_f(keep | xy | flag::C);
            }
            Ccf => {
                let f = self.regs.f;
                let keep = f & (flag::S | flag::Z | flag::PV);
                let h = if f & flag::C != 0 { flag::H } else { 0 };
                let c = (f & flag::C) ^ flag::C;
                let xy = self.scf_ccf_xy();
                self.set_f(keep | xy | h | c);
            }
            Halt => self.halted = true,
            LdRR(dst, src) => {
                if dst == REG_MEM {
                    // LD (HL),r / LD (IX+d),r: r is the real register.
                    let addr = self.mem_operand(bus, idx, INDEX_DISP_INTERNAL);
                    let v = self.reg8(src, Index::Hl);
                    self.write(bus, addr, v);
                } else if src == REG_MEM {
                    let addr = self.mem_operand(bus, idx, INDEX_DISP_INTERNAL);
                    let v = self.read(bus, addr);
                    self.set_reg8(dst, Index::Hl, v);
                } else {
                    let v = self.reg8(src, idx);
                    self.set_reg8(dst, idx, v);
                }
            }
            Alu(op, src) => {
                let v = if src == REG_MEM {
                    let addr = self.mem_operand(bus, idx, INDEX_DISP_INTERNAL);
                    self.read(bus, addr)
                } else {
                    self.reg8(src, idx)
                };
                self.alu_a(op, v);
            }
            AluN(op) => {
                let v = self.fetch_byte(bus);
                self.alu_a(op, v);
            }
            RetCc(cc) => {
                self.idle(1);
                if self.condition(cc) {
                    self.regs.pc = self.pop(bus);
                    self.regs.wz = self.regs.pc;
                }
            }
            Pop(p) => {
                let v = self.pop(bus);
                self.set_rp2(p, idx, v);
            }
            Ret => {
                self.regs.pc = self.pop(bus);
                self.regs.wz = self.regs.pc;
            }
            Exx => {
                let (bc, de, hl) = (self.regs.bc(), self.regs.de(), self.regs.hl());
                self.regs.set_bc(self.regs.bc_alt);
                self.regs.set_de(self.regs.de_alt);
                self.regs.set_hl(self.regs.hl_alt);
                (self.regs.bc_alt, self.regs.de_alt, self.regs.hl_alt) = (bc, de, hl);
            }
            JpHl => self.regs.pc = self.index_value(idx),
            LdSpHl => {
                self.idle(2);
                self.regs.sp = self.index_value(idx);
            }
            JpCc(cc) => {
                let nn = self.fetch_word(bus);
                self.regs.wz = nn;
                if self.condition(cc) {
                    self.regs.pc = nn;
                }
            }
            Jp => {
                let nn = self.fetch_word(bus);
                self.regs.wz = nn;
                self.regs.pc = nn;
            }
            OutNA => {
                let n = self.fetch_byte(bus);
                let a = self.regs.a;
                self.port_out(bus, ((a as u16) << 8) | n as u16, a);
                self.regs.wz = ((a as u16) << 8) | (n.wrapping_add(1) as u16);
            }
            InAN => {
                let n = self.fetch_byte(bus);
                let port = ((self.regs.a as u16) << 8) | n as u16;
                self.regs.a = self.port_in(bus, port);
                self.regs.wz = port.wrapping_add(1);
            }
            ExSpHl => {
                let sp = self.regs.sp;
                let lo = self.read(bus, sp);
                let hi = self.read(bus, sp.wrapping_add(1));
                self.idle(1);
                let [old_lo, old_hi] = self.index_value(idx).to_le_bytes();
                self.write(bus, sp.wrapping_add(1), old_hi);
                self.write(bus, sp, old_lo);
                self.idle(2);
                let v = u16::from_le_bytes([lo, hi]);
                self.set_index_value(idx, v);
                self.regs.wz = v;
            }
            ExDeHl => {
                // Never affected by a DD/FD prefix.
                let (de, hl) = (self.regs.de(), self.regs.hl());
                self.regs.set_de(hl);
                self.regs.set_hl(de);
            }
            Di => {
                self.regs.iff1 = false;
                self.regs.iff2 = false;
            }
            Ei => {
                self.regs.iff1 = true;
                self.regs.iff2 = true;
                self.regs.ei = true;
            }
            CallCc(cc) => {
                let nn = self.fetch_word(bus);
                self.regs.wz = nn;
                if self.condition(cc) {
                    self.idle(1);
                    self.push(bus, self.regs.pc);
                    self.regs.pc = nn;
                }
            }
            Push(p) => {
                self.idle(1);
                let v = self.rp2(p, idx);
                self.push(bus, v);
            }
            Call => {
                let nn = self.fetch_word(bus);
                self.regs.wz = nn;
                self.idle(1);
                self.push(bus, self.regs.pc);
                self.regs.pc = nn;
            }
            Rst(y) => {
                self.idle(1);
                self.push(bus, self.regs.pc);
                self.regs.pc = (y as u16) * 8;
                self.regs.wz = self.regs.pc;
            }
            PrefixCb => self.execute_cb(bus),
            PrefixEd => self.execute_ed(bus),
            PrefixDd => self.execute_indexed(bus, Index::Ix),
            PrefixFd => self.execute_indexed(bus, Index::Iy),
            _ => unreachable!("{instr:?} is not an unprefixed instruction"),
        }
    }

    /// After a DD/FD prefix: HL, H, L and (HL) mean IX/IY, IXH/IXL and
    /// (IX+d). DD/FD CB is the indexed CB page. A further DD/FD/ED prefix
    /// replaces this one (the earlier prefix only cost its fetch); those
    /// chains are not covered by SingleStepTests.
    fn execute_indexed<B: Bus>(&mut self, bus: &mut B, idx: Index) {
        // The prefix behaves as an instruction that leaves the flags alone:
        // Q is 0 for what follows (visible through SCF/CCF's X/Y).
        self.regs.q = 0;
        let op = self.fetch_opcode(bus);
        match decode_base(op) {
            Instruction::PrefixCb => self.execute_indexed_cb(bus, idx),
            Instruction::PrefixDd => self.execute_indexed(bus, Index::Ix),
            Instruction::PrefixFd => self.execute_indexed(bus, Index::Iy),
            Instruction::PrefixEd => self.execute_ed(bus),
            instr => self.execute(bus, instr, idx),
        }
    }

    fn alu_a(&mut self, op: u8, v: u8) {
        let (a, f) = alu8(op, self.regs.a, v, self.regs.f);
        self.regs.a = a;
        self.set_f(f);
    }

    /// DAA ("The Undocumented Z80 Documented", §4.7).
    fn daa(&mut self) {
        let (a, f) = (self.regs.a, self.regs.f);
        let lo = a & 0x0F;
        let mut diff = 0;
        let mut carry = 0;
        if f & flag::C != 0 || a > 0x99 {
            diff |= 0x60;
            carry = flag::C;
        }
        if f & flag::H != 0 || lo > 9 {
            diff |= 0x06;
        }
        let subtract = f & flag::N != 0;
        let r = if subtract {
            a.wrapping_sub(diff)
        } else {
            a.wrapping_add(diff)
        };
        let half = if subtract {
            f & flag::H != 0 && lo < 6
        } else {
            lo > 9
        };
        self.regs.a = r;
        let nf =
            sz_xy(r) | super::parity(r) | (f & flag::N) | carry | if half { flag::H } else { 0 };
        self.set_f(nf);
    }
}

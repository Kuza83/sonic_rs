//! CB page: rotates/shifts, BIT, RES, SET (and the DDCB/FDCB forms).
//!
//! BIT flags ("The Undocumented Z80 Documented" §4.1): Z = P/V = tested bit
//! clear, S = bit 7 tested and set, H = 1, N = 0, C kept; X/Y come from the
//! register for BIT b,r, from the high byte of WZ for BIT b,(HL), and from
//! the high byte of the address for BIT b,(IX+d).

use super::rot8;
use crate::bus::Bus;
use crate::cpu::{Cpu, Index};
use crate::decode::decode_cb;
use crate::instruction::Instruction;
use crate::registers::flag;

const REG_MEM: u8 = 6;
/// DDCB/FDCB: internal cycles after fetching the final opcode byte.
const INDEXED_CB_INTERNAL: u32 = 2;

impl Cpu {
    fn bit_flags(&mut self, bit: u8, value: u8, xy_source: u8) {
        let tested = value & (1 << bit);
        let mut f = (self.regs.f & flag::C) | flag::H | (xy_source & (flag::X | flag::Y));
        if tested == 0 {
            f |= flag::Z | flag::PV;
        }
        if bit == 7 && tested != 0 {
            f |= flag::S;
        }
        self.set_f(f);
    }

    /// Applies a rotate/RES/SET operation; BIT is handled by the callers.
    fn cb_modify(&mut self, instr: Instruction, v: u8) -> u8 {
        match instr {
            Instruction::Rot(op, _) => {
                let (r, f) = rot8(op, v, self.regs.f);
                self.set_f(f);
                r
            }
            Instruction::Res(b, _) => v & !(1 << b),
            Instruction::Set(b, _) => v | (1 << b),
            _ => unreachable!("not a modifying CB instruction"),
        }
    }

    /// CB xx after a plain CB prefix.
    pub(crate) fn execute_cb<B: Bus>(&mut self, bus: &mut B) {
        let op = self.fetch_opcode(bus);
        let instr = decode_cb(op);
        let r = op & 7;
        match instr {
            Instruction::Bit(b, _) if r == REG_MEM => {
                let v = self.read(bus, self.regs.hl());
                self.idle(1);
                let wz_hi = (self.regs.wz >> 8) as u8;
                self.bit_flags(b, v, wz_hi);
            }
            Instruction::Bit(b, _) => {
                let v = self.reg8(r, Index::Hl);
                self.bit_flags(b, v, v);
            }
            _ if r == REG_MEM => {
                let addr = self.regs.hl();
                let v = self.read(bus, addr);
                self.idle(1);
                let v = self.cb_modify(instr, v);
                self.write(bus, addr, v);
            }
            _ => {
                let v = self.cb_modify(instr, self.reg8(r, Index::Hl));
                self.set_reg8(r, Index::Hl, v);
            }
        }
    }

    /// DD CB d xx / FD CB d xx: the operand is always (IX+d); the result of
    /// a rotate/RES/SET is also copied to register z when z ≠ 6
    /// (undocumented). The final opcode byte is read as data, not as an M1
    /// cycle (R is not incremented for it).
    pub(crate) fn execute_indexed_cb<B: Bus>(&mut self, bus: &mut B, idx: Index) {
        let d = self.fetch_byte(bus) as i8;
        let op = self.fetch_byte(bus);
        self.idle(INDEXED_CB_INTERNAL);
        let addr = self.index_value(idx).wrapping_add(d as i16 as u16);
        self.regs.wz = addr;
        let instr = decode_cb(op);
        let v = self.read(bus, addr);
        self.idle(1);
        if let Instruction::Bit(b, _) = instr {
            self.bit_flags(b, v, (addr >> 8) as u8);
            return;
        }
        let v = self.cb_modify(instr, v);
        self.write(bus, addr, v);
        let r = op & 7;
        if r != REG_MEM {
            self.set_reg8(r, Index::Hl, v);
        }
    }
}

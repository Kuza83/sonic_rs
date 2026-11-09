//! Bit manipulation (BTST/BCHG/BCLR/BSET), Scc and TAS.

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::Operand;
use crate::instruction::{BitNumber, BitOp, Ea, Size};
use crate::registers::sr;

/// Bit numbers wrap modulo 32 on data registers and modulo 8 on memory bytes.
const REGISTER_BIT_MASK: u32 = 31;
const MEMORY_BIT_MASK: u32 = 7;
/// Bit operations on Dn: internal cycles (BTST, BCHG/BSET low/high word, BCLR low/high word).
const BTST_REG: u32 = 2;
const BCHG_BSET_REG_LOW: u32 = 2;
const BCHG_BSET_REG_HIGH: u32 = 4;
const BCLR_REG_LOW: u32 = 4;
const BCLR_REG_HIGH: u32 = 6;
/// BTST Dn,#imm: internal cycles after the prefetch.
const BTST_IMMEDIATE: u32 = 2;
/// Scc Dn: extra cycles when the condition is true.
const SCC_REG_TRUE: u32 = 2;
/// TAS: idle cycles inside the read-modify-write bus cycle.
const TAS_RMW_INTERNAL: u32 = 2;

impl Cpu {
    /// Evaluates a condition code (M68000 PRM Table 3-19).
    pub(crate) fn condition(&self, cond: u8) -> bool {
        let s = self.regs.sr();
        let (n, z, v, c) = (
            s & sr::N != 0,
            s & sr::Z != 0,
            s & sr::V != 0,
            s & sr::C != 0,
        );
        match cond & 0xF {
            0x0 => true,
            0x1 => false,
            0x2 => !c && !z,
            0x3 => c || z,
            0x4 => !c,
            0x5 => c,
            0x6 => !z,
            0x7 => z,
            0x8 => !v,
            0x9 => v,
            0xA => !n,
            0xB => n,
            0xC => n == v,
            0xD => n != v,
            0xE => !z && n == v,
            _ => z || n != v,
        }
    }

    fn set_z_from_bit(&mut self, value: u32, bit: u32) {
        let s = self.regs.sr() & !sr::Z;
        let z = if value & (1 << bit) == 0 { sr::Z } else { 0 };
        self.regs.set_sr(s | z);
    }

    fn apply_bit(op: BitOp, value: u32, bit: u32) -> u32 {
        match op {
            BitOp::Btst => value,
            BitOp::Bchg => value ^ (1 << bit),
            BitOp::Bclr => value & !(1 << bit),
            BitOp::Bset => value | (1 << bit),
        }
    }

    pub(super) fn op_bit<B: Bus>(
        &mut self,
        bus: &mut B,
        op: BitOp,
        bit: BitNumber,
        dst: Ea,
    ) -> Res {
        let number = match bit {
            BitNumber::Immediate => self.fetch(bus)? as u32,
            BitNumber::Register(r) => self.regs.d[r as usize],
        };
        match self.resolve(bus, dst, Size::Byte)? {
            Operand::DataReg(r) => {
                let bit = number & REGISTER_BIT_MASK;
                let value = self.regs.d[r as usize];
                self.set_z_from_bit(value, bit);
                self.regs.d[r as usize] = Self::apply_bit(op, value, bit);
                self.fetch(bus)?;
                let high = bit >= 16;
                self.idle(match op {
                    BitOp::Btst => BTST_REG,
                    BitOp::Bchg | BitOp::Bset if high => BCHG_BSET_REG_HIGH,
                    BitOp::Bchg | BitOp::Bset => BCHG_BSET_REG_LOW,
                    BitOp::Bclr if high => BCLR_REG_HIGH,
                    BitOp::Bclr => BCLR_REG_LOW,
                });
                Ok(())
            }
            Operand::Immediate(value) => {
                // BTST Dn,#imm
                self.set_z_from_bit(value, number & MEMORY_BIT_MASK);
                self.fetch(bus)?;
                self.idle(BTST_IMMEDIATE);
                Ok(())
            }
            Operand::Memory(addr) => {
                let bit = number & MEMORY_BIT_MASK;
                let value = self.read(bus, addr, Size::Byte)?;
                self.set_z_from_bit(value, bit);
                self.fetch(bus)?;
                if op != BitOp::Btst {
                    self.write(bus, addr, Size::Byte, Self::apply_bit(op, value, bit))?;
                }
                Ok(())
            }
            Operand::AddrReg(_) => unreachable!("bit operations never target An"),
        }
    }

    /// Scc: memory destinations are read before being written, whatever the condition.
    pub(super) fn op_scc<B: Bus>(&mut self, bus: &mut B, cond: u8, dst: Ea) -> Res {
        let taken = self.condition(cond);
        let value = if taken { 0xFF } else { 0x00 };
        match self.resolve(bus, dst, Size::Byte)? {
            Operand::DataReg(r) => {
                self.write_data_reg(r, Size::Byte, value);
                self.fetch(bus)?;
                if taken {
                    self.idle(SCC_REG_TRUE);
                }
                Ok(())
            }
            Operand::Memory(addr) => {
                self.read(bus, addr, Size::Byte)?;
                self.fetch(bus)?;
                self.write(bus, addr, Size::Byte, value)
            }
            _ => unreachable!("Scc destination is data alterable"),
        }
    }

    /// TAS: indivisible read-modify-write; tests the byte then sets bit 7.
    pub(super) fn op_tas<B: Bus>(&mut self, bus: &mut B, dst: Ea) -> Res {
        match self.resolve(bus, dst, Size::Byte)? {
            Operand::DataReg(r) => {
                let value = self.regs.d[r as usize] & 0xFF;
                self.set_logic_flags(value, Size::Byte);
                self.write_data_reg(r, Size::Byte, value | 0x80);
                self.fetch(bus).map(drop)
            }
            Operand::Memory(addr) => {
                let value = self.read(bus, addr, Size::Byte)?;
                self.set_logic_flags(value, Size::Byte);
                self.idle(TAS_RMW_INTERNAL);
                self.write(bus, addr, Size::Byte, value | 0x80)?;
                self.fetch(bus).map(drop)
            }
            _ => unreachable!("TAS destination is data alterable"),
        }
    }
}

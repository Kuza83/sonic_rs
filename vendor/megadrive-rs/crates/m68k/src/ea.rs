//! Effective address resolution and operand access.
//!
//! The bus order (extension fetches, idle cycles) follows the 68000 microcode
//! as observed in SingleStepTests/680x0 transactions (Yacht table agrees).

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::instruction::{Ea, Size};

/// Internal cycles spent computing an indexed address before its extension word.
const INDEX_CALC: u32 = 2;
/// Internal cycles spent decrementing An for a predecrement source.
const PREDEC_CALC: u32 = 2;

/// A resolved operand location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operand {
    DataReg(u8),
    AddrReg(u8),
    Memory(u32),
    Immediate(u32),
}

pub(crate) fn sign_extend(value: u32, size: Size) -> u32 {
    match size {
        Size::Byte => value as u8 as i8 as i32 as u32,
        Size::Word => value as u16 as i16 as i32 as u32,
        Size::Long => value,
    }
}

impl Cpu {
    /// (An)+ / -(An) step: byte accesses through A7 keep the stack word aligned.
    fn step_size(reg: u8, size: Size) -> u32 {
        if reg == 7 && size == Size::Byte {
            2
        } else {
            size.bytes()
        }
    }

    /// Index register value from a brief extension word (M68000 PRM §2.2.8):
    /// bit 15 D/A, bits 14..12 register, bit 11 W/L.
    fn index_value(&self, ext: u16) -> u32 {
        let reg = ((ext >> 12) & 7) as usize;
        let value = if ext & 0x8000 != 0 {
            self.regs.a[reg]
        } else {
            self.regs.d[reg]
        };
        if ext & 0x0800 != 0 {
            value
        } else {
            sign_extend(value, Size::Word)
        }
    }

    pub(crate) fn brief_ext_address(&self, base: u32, ext: u16) -> u32 {
        base.wrapping_add(self.index_value(ext))
            .wrapping_add(sign_extend(ext as u32, Size::Byte))
    }

    /// Resolves `ea`, fetching extension words and applying (An)+ / -(An).
    pub(crate) fn resolve<B: Bus>(&mut self, bus: &mut B, ea: Ea, size: Size) -> Res<Operand> {
        Ok(match ea {
            Ea::DataReg(r) => Operand::DataReg(r),
            Ea::AddrReg(r) => Operand::AddrReg(r),
            Ea::Indirect(r) => Operand::Memory(self.regs.a[r as usize]),
            Ea::PostInc(r) => {
                let addr = self.regs.a[r as usize];
                self.regs.a[r as usize] = addr.wrapping_add(Self::step_size(r, size));
                Operand::Memory(addr)
            }
            Ea::PreDec(r) => {
                self.idle(PREDEC_CALC);
                let addr = self.regs.a[r as usize].wrapping_sub(Self::step_size(r, size));
                self.regs.a[r as usize] = addr;
                Operand::Memory(addr)
            }
            Ea::Immediate => Operand::Immediate(match size {
                Size::Byte => self.fetch(bus)? as u32 & 0xFF,
                Size::Word => self.fetch(bus)? as u32,
                Size::Long => {
                    let hi = self.fetch(bus)? as u32;
                    (hi << 16) | self.fetch(bus)? as u32
                }
            }),
            _ => Operand::Memory(self.control_address(bus, ea)?),
        })
    }

    /// Address of a memory EA that needs no register side effect
    /// (Indirect, displacement, index, absolute, PC-relative).
    pub(crate) fn control_address<B: Bus>(&mut self, bus: &mut B, ea: Ea) -> Res<u32> {
        Ok(match ea {
            Ea::Indirect(r) => self.regs.a[r as usize],
            Ea::Disp16(r) => {
                let base = self.regs.a[r as usize];
                base.wrapping_add(sign_extend(self.fetch(bus)? as u32, Size::Word))
            }
            Ea::Index8(r) => {
                self.idle(INDEX_CALC);
                let base = self.regs.a[r as usize];
                let ext = self.fetch(bus)?;
                self.brief_ext_address(base, ext)
            }
            Ea::AbsShort => sign_extend(self.fetch(bus)? as u32, Size::Word),
            Ea::AbsLong => {
                let hi = self.fetch(bus)? as u32;
                (hi << 16) | self.fetch(bus)? as u32
            }
            Ea::PcDisp16 => {
                // Base is the address of the extension word itself.
                let base = self.regs.pc.wrapping_add(2);
                base.wrapping_add(sign_extend(self.fetch(bus)? as u32, Size::Word))
            }
            Ea::PcIndex8 => {
                self.idle(INDEX_CALC);
                let base = self.regs.pc.wrapping_add(2);
                let ext = self.fetch(bus)?;
                self.brief_ext_address(base, ext)
            }
            Ea::DataReg(_) | Ea::AddrReg(_) | Ea::PostInc(_) | Ea::PreDec(_) | Ea::Immediate => {
                unreachable!("{ea:?} is not a control address")
            }
        })
    }

    pub(crate) fn read_operand<B: Bus>(
        &mut self,
        bus: &mut B,
        op: Operand,
        size: Size,
    ) -> Res<u32> {
        match op {
            Operand::DataReg(r) => Ok(self.regs.d[r as usize] & size.mask()),
            Operand::AddrReg(r) => Ok(self.regs.a[r as usize] & size.mask()),
            Operand::Memory(addr) => self.read(bus, addr, size),
            Operand::Immediate(value) => Ok(value),
        }
    }

    /// Writes the low `size` bits of a data register, preserving the rest.
    pub(crate) fn write_data_reg(&mut self, reg: u8, size: Size, value: u32) {
        let d = &mut self.regs.d[reg as usize];
        *d = (*d & !size.mask()) | (value & size.mask());
    }

    pub(crate) fn resolve_read<B: Bus>(
        &mut self,
        bus: &mut B,
        ea: Ea,
        size: Size,
    ) -> Res<(Operand, u32)> {
        let op = self.resolve(bus, ea, size)?;
        let value = self.read_operand(bus, op, size)?;
        Ok((op, value))
    }
}

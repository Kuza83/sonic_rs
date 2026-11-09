//! MOVEM, MOVEP and the status/USP moves.

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::{Operand, sign_extend};
use crate::instruction::{Ea, Size};
use crate::registers::sr;

/// MOVE to SR/CCR: internal cycles before the prefetch queue reload.
const MOVE_TO_STATUS_INTERNAL: u32 = 4;
/// MOVE from SR to Dn: internal cycles after the prefetch.
const MOVE_FROM_SR_REG: u32 = 2;

impl Cpu {
    /// Register `index` of a MOVEM mask: 0..=7 are D0-D7, 8..=15 are A0-A7.
    fn movem_reg(&self, index: usize) -> u32 {
        if index < 8 {
            self.regs.d[index]
        } else {
            self.regs.a[index - 8]
        }
    }

    fn set_movem_reg(&mut self, index: usize, value: u32) {
        if index < 8 {
            self.regs.d[index] = value;
        } else {
            self.regs.a[index - 8] = value;
        }
    }

    pub(super) fn op_movem<B: Bus>(
        &mut self,
        bus: &mut B,
        size: Size,
        to_memory: bool,
        ea: Ea,
    ) -> Res {
        let mask = self.fetch(bus)?;
        if to_memory {
            self.movem_to_memory(bus, size, ea, mask)?;
        } else {
            self.movem_to_registers(bus, size, ea, mask)?;
        }
        self.fetch(bus).map(drop)
    }

    fn movem_to_memory<B: Bus>(&mut self, bus: &mut B, size: Size, ea: Ea, mask: u16) -> Res {
        if let Ea::PreDec(an) = ea {
            // Predecrement: the mask is reversed (bit 0 = A7) and registers are
            // stored downwards, each long low word first. The initial An is
            // what gets stored if An is in the list.
            let mut addr = self.regs.a[an as usize];
            for bit in 0..16 {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                let value = self.movem_reg(15 - bit);
                addr = addr.wrapping_sub(size.bytes());
                self.write_low_first(bus, addr, size, value)?;
            }
            self.regs.a[an as usize] = addr;
            return Ok(());
        }
        let mut addr = self.control_address(bus, ea)?;
        for index in 0..16 {
            if mask & (1 << index) != 0 {
                let value = self.movem_reg(index);
                self.write(bus, addr, size, value)?;
                addr = addr.wrapping_add(size.bytes());
            }
        }
        Ok(())
    }

    /// Memory to registers: word values are sign-extended to 32 bits, and the
    /// 68000 performs one extra word read past the last register.
    fn movem_to_registers<B: Bus>(&mut self, bus: &mut B, size: Size, ea: Ea, mask: u16) -> Res {
        let mut addr = match ea {
            Ea::PostInc(an) => self.regs.a[an as usize],
            _ => self.control_address(bus, ea)?,
        };
        for index in 0..16 {
            if mask & (1 << index) != 0 {
                let value = match self.read(bus, addr, size) {
                    Ok(value) => value,
                    Err(fault) => {
                        // An odd (An)+ faults on the first word, by which time
                        // An already points one word further.
                        if let Ea::PostInc(an) = ea {
                            self.regs.a[an as usize] = addr.wrapping_add(2);
                        }
                        return Err(fault);
                    }
                };
                self.set_movem_reg(index, sign_extend(value, size));
                addr = addr.wrapping_add(size.bytes());
            }
        }
        self.read_word(bus, addr)?;
        if let Ea::PostInc(an) = ea {
            self.regs.a[an as usize] = addr;
        }
        Ok(())
    }

    /// MOVEP: bytes at every other address, most significant first.
    pub(super) fn op_movep<B: Bus>(
        &mut self,
        bus: &mut B,
        size: Size,
        to_memory: bool,
        dreg: u8,
        areg: u8,
    ) -> Res {
        let disp = sign_extend(self.fetch(bus)? as u32, Size::Word);
        let base = self.regs.a[areg as usize].wrapping_add(disp);
        let count = size.bytes();
        if to_memory {
            let value = self.regs.d[dreg as usize];
            for i in 0..count {
                let shift = 8 * (count - 1 - i);
                self.write(bus, base.wrapping_add(2 * i), Size::Byte, value >> shift)?;
            }
        } else {
            let mut value = 0;
            for i in 0..count {
                value = (value << 8) | self.read(bus, base.wrapping_add(2 * i), Size::Byte)?;
            }
            self.write_data_reg(dreg, size, value);
        }
        self.fetch(bus).map(drop)
    }

    /// MOVE from SR is not privileged on the 68000. Memory destinations are
    /// read before being written.
    pub(super) fn op_move_from_sr<B: Bus>(&mut self, bus: &mut B, dst: Ea) -> Res {
        let value = self.regs.sr() as u32;
        match self.resolve(bus, dst, Size::Word)? {
            Operand::DataReg(r) => {
                self.write_data_reg(r, Size::Word, value);
                self.fetch(bus)?;
                self.idle(MOVE_FROM_SR_REG);
                Ok(())
            }
            Operand::Memory(addr) => {
                self.read(bus, addr, Size::Word)?;
                self.fetch(bus)?;
                self.write(bus, addr, Size::Word, value)
            }
            _ => unreachable!("MOVE from SR destination is data alterable"),
        }
    }

    pub(super) fn op_move_to_ccr<B: Bus>(&mut self, bus: &mut B, src: Ea) -> Res {
        let (_, value) = self.resolve_read(bus, src, Size::Word)?;
        let sr = self.regs.sr();
        self.regs.set_sr((sr & !sr::CCR) | (value as u16 & sr::CCR));
        self.finish_status_write(bus, MOVE_TO_STATUS_INTERNAL)
    }

    pub(super) fn op_move_to_sr<B: Bus>(&mut self, bus: &mut B, src: Ea) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        let (_, value) = self.resolve_read(bus, src, Size::Word)?;
        self.regs.set_sr(value as u16);
        self.finish_status_write(bus, MOVE_TO_STATUS_INTERNAL)
    }

    pub(super) fn op_move_usp<B: Bus>(&mut self, bus: &mut B, to_usp: bool, reg: u8) -> Res {
        if !self.regs.supervisor() {
            return self.privilege_violation(bus);
        }
        if to_usp {
            let value = self.regs.a[reg as usize];
            self.regs.set_usp(value);
        } else {
            self.regs.a[reg as usize] = self.regs.usp();
        }
        self.fetch(bus).map(drop)
    }
}

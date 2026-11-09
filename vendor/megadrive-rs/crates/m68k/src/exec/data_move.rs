//! Data movement: MOVE, MOVEA, MOVEQ, LEA, PEA, CLR, TST, EXG, SWAP, EXT.

use crate::bus::Bus;
use crate::cpu::{Cpu, Res};
use crate::ea::{Operand, sign_extend};
use crate::instruction::{Ea, ExgMode, Size};

/// LEA/PEA spend two extra internal cycles after an indexed address calculation.
const CONTROL_INDEX_EXTRA: u32 = 2;
/// Internal cycles of register-to-register operations on 32 bits.
const LONG_REG_EXTRA: u32 = 2;

impl Cpu {
    pub(super) fn op_move<B: Bus>(&mut self, bus: &mut B, size: Size, src: Ea, dst: Ea) -> Res {
        let (src_op, value) = self.resolve_read(bus, src, size)?;
        let src_in_memory = matches!(src_op, Operand::Memory(_));
        self.set_logic_flags(value, size);

        match dst {
            Ea::DataReg(r) => {
                self.write_data_reg(r, size, value);
                self.fetch(bus)?;
            }
            Ea::PreDec(r) => {
                // The next opcode is prefetched before the write; long data
                // goes low word first.
                self.fetch(bus)?;
                let r = r as usize;
                if size == Size::Long {
                    // An is decremented one word at a time: a fault on the low
                    // word leaves An - 2 in the register and the frame.
                    let lo_addr = self.regs.a[r].wrapping_sub(2);
                    self.regs.a[r] = lo_addr;
                    self.write_word(bus, lo_addr, value as u16)?;
                    let hi_addr = lo_addr.wrapping_sub(2);
                    self.regs.a[r] = hi_addr;
                    self.write_word(bus, hi_addr, (value >> 16) as u16)?;
                } else {
                    let step = if r == 7 && size == Size::Byte {
                        2
                    } else {
                        size.bytes()
                    };
                    let addr = self.regs.a[r].wrapping_sub(step);
                    self.regs.a[r] = addr;
                    self.write(bus, addr, size, value)?;
                }
            }
            Ea::PostInc(r) => {
                // An is only incremented once the write has completed.
                let addr = self.regs.a[r as usize];
                self.write(bus, addr, size, value)?;
                let step = if r == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.regs.a[r as usize] = addr.wrapping_add(step);
                self.fetch(bus)?;
            }
            Ea::AbsLong if src_in_memory => {
                // With a memory source, the low address word is used straight
                // from IRC and only refilled after the write.
                let hi = self.fetch(bus)? as u32;
                let addr = (hi << 16) | self.irc() as u32;
                self.write(bus, addr, size, value)?;
                self.fetch(bus)?;
                self.fetch(bus)?;
            }
            _ => {
                let Operand::Memory(addr) = self.resolve(bus, dst, size)? else {
                    unreachable!("MOVE destination is data alterable memory");
                };
                self.write(bus, addr, size, value)?;
                self.fetch(bus)?;
            }
        }
        Ok(())
    }

    pub(super) fn op_movea<B: Bus>(&mut self, bus: &mut B, size: Size, src: Ea, dst: u8) -> Res {
        let (_, value) = self.resolve_read(bus, src, size)?;
        self.regs.a[dst as usize] = sign_extend(value, size);
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_moveq<B: Bus>(&mut self, bus: &mut B, value: i8, dst: u8) -> Res {
        let value = value as i32 as u32;
        self.regs.d[dst as usize] = value;
        self.set_logic_flags(value, Size::Long);
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_lea<B: Bus>(&mut self, bus: &mut B, src: Ea, dst: u8) -> Res {
        let addr = self.control_address(bus, src)?;
        if matches!(src, Ea::Index8(_) | Ea::PcIndex8) {
            self.idle(CONTROL_INDEX_EXTRA);
        }
        self.regs.a[dst as usize] = addr;
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_pea<B: Bus>(&mut self, bus: &mut B, src: Ea) -> Res {
        let addr = self.control_address(bus, src)?;
        if matches!(src, Ea::Index8(_) | Ea::PcIndex8) {
            self.idle(CONTROL_INDEX_EXTRA);
        }
        // Absolute modes push before prefetching; the others prefetch first.
        let absolute = matches!(src, Ea::AbsShort | Ea::AbsLong);
        if !absolute {
            self.fetch(bus)?;
        }
        let sp = self.regs.a[7].wrapping_sub(4);
        self.regs.a[7] = sp;
        self.write(bus, sp, Size::Long, addr)?;
        if absolute {
            self.fetch(bus)?;
        }
        Ok(())
    }

    /// CLR reads its destination before writing it (M68000 UM, CLR timing note).
    pub(super) fn op_clr<B: Bus>(&mut self, bus: &mut B, size: Size, dst: Ea) -> Res {
        let op = self.resolve(bus, dst, size)?;
        match op {
            Operand::DataReg(r) => {
                self.set_logic_flags(0, size);
                self.write_data_reg(r, size, 0);
                self.fetch(bus)?;
                if size == Size::Long {
                    self.idle(LONG_REG_EXTRA);
                }
            }
            Operand::Memory(addr) => {
                self.read(bus, addr, size)?;
                self.set_logic_flags(0, size);
                self.fetch(bus)?;
                self.write_low_first(bus, addr, size, 0)?;
            }
            _ => unreachable!("CLR destination is data alterable"),
        }
        Ok(())
    }

    pub(super) fn op_tst<B: Bus>(&mut self, bus: &mut B, size: Size, dst: Ea) -> Res {
        let (_, value) = self.resolve_read(bus, dst, size)?;
        self.set_logic_flags(value, size);
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_exg<B: Bus>(&mut self, bus: &mut B, mode: ExgMode, rx: u8, ry: u8) -> Res {
        let (rx, ry) = (rx as usize, ry as usize);
        match mode {
            ExgMode::DataData => self.regs.d.swap(rx, ry),
            ExgMode::AddrAddr => self.regs.a.swap(rx, ry),
            ExgMode::DataAddr => std::mem::swap(&mut self.regs.d[rx], &mut self.regs.a[ry]),
        }
        self.fetch(bus)?;
        self.idle(LONG_REG_EXTRA);
        Ok(())
    }

    pub(super) fn op_swap<B: Bus>(&mut self, bus: &mut B, reg: u8) -> Res {
        let value = self.regs.d[reg as usize].rotate_left(16);
        self.regs.d[reg as usize] = value;
        self.set_logic_flags(value, Size::Long);
        self.fetch(bus).map(drop)
    }

    pub(super) fn op_ext<B: Bus>(&mut self, bus: &mut B, size: Size, reg: u8) -> Res {
        let d = self.regs.d[reg as usize];
        let value = match size {
            Size::Word => sign_extend(d, Size::Byte),
            _ => sign_extend(d, Size::Word),
        };
        self.write_data_reg(reg, size, value);
        self.set_logic_flags(value, size);
        self.fetch(bus).map(drop)
    }
}

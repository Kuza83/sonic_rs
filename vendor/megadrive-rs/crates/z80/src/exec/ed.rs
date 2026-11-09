//! ED page. Flags of IN/OUT block instructions and of interrupted block
//! repeats follow "The Undocumented Z80 Documented" and the 2022 block
//! instruction research by David Banks (hoglet), as encoded in
//! SingleStepTests/z80.

use super::{parity, sub8, sz_xy};
use crate::bus::Bus;
use crate::cpu::{Cpu, Index};
use crate::decode::decode_ed;
use crate::instruction::Instruction;
use crate::registers::flag;

const REG_MEM: u8 = 6;
/// Internal cycles of the 16-bit ADC/SBC.
const ALU16_INTERNAL: u32 = 7;
/// Extra cycles of a repeating block instruction.
const BLOCK_REPEAT: u32 = 5;
/// Block kinds.
const BLOCK_LD: u8 = 0;
const BLOCK_CP: u8 = 1;
const BLOCK_IN: u8 = 2;

impl Cpu {
    pub(crate) fn execute_ed<B: Bus>(&mut self, bus: &mut B) {
        let op = self.fetch_opcode(bus);
        use Instruction::*;
        match decode_ed(op) {
            InRC(r) => {
                let bc = self.regs.bc();
                let v = self.port_in(bus, bc);
                self.regs.wz = bc.wrapping_add(1);
                if r != REG_MEM {
                    self.set_reg8(r, Index::Hl, v);
                }
                let f = (self.regs.f & flag::C) | sz_xy(v) | parity(v);
                self.set_f(f);
            }
            OutCR(r) => {
                let bc = self.regs.bc();
                let v = if r == REG_MEM {
                    0
                } else {
                    self.reg8(r, Index::Hl)
                };
                self.port_out(bus, bc, v);
                self.regs.wz = bc.wrapping_add(1);
            }
            SbcHlRp(p) | AdcHlRp(p) => {
                self.idle(ALU16_INTERNAL);
                let hl = self.regs.hl();
                let v = self.rp(p, Index::Hl);
                let c = (self.regs.f & flag::C) as u32;
                let subtract = matches!(decode_ed(op), SbcHlRp(_));
                let wide = if subtract {
                    (hl as u32).wrapping_sub(v as u32).wrapping_sub(c)
                } else {
                    hl as u32 + v as u32 + c
                };
                let r = wide as u16;
                let mut f = ((r >> 8) as u8) & (flag::S | flag::X | flag::Y);
                if r == 0 {
                    f |= flag::Z;
                }
                if (hl ^ v ^ r) & 0x1000 != 0 {
                    f |= flag::H;
                }
                let overflow = if subtract {
                    (hl ^ v) & (hl ^ r) & 0x8000
                } else {
                    (hl ^ r) & (v ^ r) & 0x8000
                };
                if overflow != 0 {
                    f |= flag::PV;
                }
                if wide > 0xFFFF {
                    f |= flag::C;
                }
                if subtract {
                    f |= flag::N;
                }
                self.regs.wz = hl.wrapping_add(1);
                self.regs.set_hl(r);
                self.set_f(f);
            }
            LdNnRp(p) => {
                let nn = self.fetch_word(bus);
                self.write_word(bus, nn, self.rp(p, Index::Hl));
                self.regs.wz = nn.wrapping_add(1);
            }
            LdRpNnInd(p) => {
                let nn = self.fetch_word(bus);
                let v = self.read_word(bus, nn);
                self.set_rp(p, Index::Hl, v);
                self.regs.wz = nn.wrapping_add(1);
            }
            Neg => {
                let (a, f) = sub8(0, self.regs.a, false);
                self.regs.a = a;
                self.set_f(f);
            }
            Retn | Reti => {
                self.regs.pc = self.pop(bus);
                self.regs.wz = self.regs.pc;
                self.regs.iff1 = self.regs.iff2;
            }
            Im(mode) => self.regs.im = mode,
            LdIA | LdRA => {
                self.idle(1);
                if matches!(decode_ed(op), LdIA) {
                    self.regs.i = self.regs.a;
                } else {
                    self.regs.r = self.regs.a;
                }
            }
            LdAI | LdAR => {
                self.idle(1);
                let v = if matches!(decode_ed(op), LdAI) {
                    self.regs.i
                } else {
                    self.regs.r
                };
                self.regs.a = v;
                let pv = if self.regs.iff2 { flag::PV } else { 0 };
                let f = (self.regs.f & flag::C) | sz_xy(v) | pv;
                self.set_f(f);
                self.regs.p = true;
            }
            Rrd | Rld => {
                let hl = self.regs.hl();
                let m = self.read(bus, hl);
                self.idle(4);
                let a = self.regs.a;
                let (new_a, new_m) = if matches!(decode_ed(op), Rrd) {
                    ((a & 0xF0) | (m & 0x0F), (a << 4) | (m >> 4))
                } else {
                    ((a & 0xF0) | (m >> 4), (m << 4) | (a & 0x0F))
                };
                self.write(bus, hl, new_m);
                self.regs.a = new_a;
                self.regs.wz = hl.wrapping_add(1);
                let f = (self.regs.f & flag::C) | sz_xy(new_a) | parity(new_a);
                self.set_f(f);
            }
            Block {
                kind,
                decrement,
                repeat,
            } => self.block(bus, kind, decrement, repeat),
            EdNop => {}
            _ => unreachable!("ED page decodes to ED instructions"),
        }
    }

    fn step_hl(&mut self, decrement: bool) {
        let hl = self.regs.hl();
        self.regs.set_hl(if decrement {
            hl.wrapping_sub(1)
        } else {
            hl.wrapping_add(1)
        });
    }

    /// Repeating block instruction: PC back on the instruction, 5 more
    /// T-states, WZ = PC + 1, X/Y from bits 11/13 of PC.
    fn block_repeat(&mut self, f: u8) -> u8 {
        self.regs.pc = self.regs.pc.wrapping_sub(2);
        self.idle(BLOCK_REPEAT);
        self.regs.wz = self.regs.pc.wrapping_add(1);
        let pch = (self.regs.pc >> 8) as u8;
        (f & !(flag::X | flag::Y)) | (pch & (flag::X | flag::Y))
    }

    fn block<B: Bus>(&mut self, bus: &mut B, kind: u8, decrement: bool, repeat: bool) {
        match kind {
            BLOCK_LD => {
                let v = self.read(bus, self.regs.hl());
                self.write(bus, self.regs.de(), v);
                self.idle(2);
                self.step_hl(decrement);
                let de = self.regs.de();
                self.regs.set_de(if decrement {
                    de.wrapping_sub(1)
                } else {
                    de.wrapping_add(1)
                });
                let bc = self.regs.bc().wrapping_sub(1);
                self.regs.set_bc(bc);
                let n = v.wrapping_add(self.regs.a);
                let mut f = self.regs.f & (flag::S | flag::Z | flag::C);
                f |= (n & flag::X) | ((n << 4) & flag::Y);
                if bc != 0 {
                    f |= flag::PV;
                }
                if repeat && bc != 0 {
                    f = self.block_repeat(f);
                }
                self.set_f(f);
            }
            BLOCK_CP => {
                let v = self.read(bus, self.regs.hl());
                self.idle(5);
                self.step_hl(decrement);
                let bc = self.regs.bc().wrapping_sub(1);
                self.regs.set_bc(bc);
                self.regs.wz = if decrement {
                    self.regs.wz.wrapping_sub(1)
                } else {
                    self.regs.wz.wrapping_add(1)
                };
                let a = self.regs.a;
                let r = a.wrapping_sub(v);
                let h = (a ^ v ^ r) & flag::H;
                let n = r.wrapping_sub((h != 0) as u8);
                let mut f = (self.regs.f & flag::C) | flag::N | h | (r & flag::S);
                if r == 0 {
                    f |= flag::Z;
                }
                f |= (n & flag::X) | ((n << 4) & flag::Y);
                if bc != 0 {
                    f |= flag::PV;
                }
                if repeat && bc != 0 && r != 0 {
                    f = self.block_repeat(f);
                }
                self.set_f(f);
            }
            _ => self.block_io(bus, kind == BLOCK_IN, decrement, repeat),
        }
    }

    /// INI/IND/OUTI/OUTD and their repeating forms.
    fn block_io<B: Bus>(&mut self, bus: &mut B, input: bool, decrement: bool, repeat: bool) {
        self.idle(1);
        let (value, k) = if input {
            let bc = self.regs.bc();
            let v = self.port_in(bus, bc);
            self.write(bus, self.regs.hl(), v);
            self.regs.wz = if decrement {
                bc.wrapping_sub(1)
            } else {
                bc.wrapping_add(1)
            };
            self.regs.b = self.regs.b.wrapping_sub(1);
            let c = if decrement {
                self.regs.c.wrapping_sub(1)
            } else {
                self.regs.c.wrapping_add(1)
            };
            (v, v as u16 + c as u16)
        } else {
            let v = self.read(bus, self.regs.hl());
            self.regs.b = self.regs.b.wrapping_sub(1);
            let bc = self.regs.bc();
            self.port_out(bus, bc, v);
            self.regs.wz = if decrement {
                bc.wrapping_sub(1)
            } else {
                bc.wrapping_add(1)
            };
            self.step_hl(decrement);
            (v, v as u16 + self.regs.l as u16)
        };
        if input {
            self.step_hl(decrement);
        }
        let b = self.regs.b;
        let mut f = sz_xy(b);
        if value & 0x80 != 0 {
            f |= flag::N;
        }
        if k > 0xFF {
            f |= flag::H | flag::C;
        }
        f |= parity((k as u8 & 7) ^ b);
        if repeat && b != 0 {
            f = self.block_repeat(f);
            // Interrupted I/O block: H and P/V are recomputed from the next B
            // (hoglet, 2022).
            f &= !(flag::H | flag::PV);
            let pv_base = parity((k as u8 & 7) ^ b);
            if k > 0xFF {
                if value & 0x80 != 0 {
                    f |= pv_base ^ parity(b.wrapping_sub(1) & 7) ^ flag::PV;
                    if b & 0x0F == 0x00 {
                        f |= flag::H;
                    }
                } else {
                    f |= pv_base ^ parity(b.wrapping_add(1) & 7) ^ flag::PV;
                    if b & 0x0F == 0x0F {
                        f |= flag::H;
                    }
                }
            } else {
                f |= pv_base ^ parity(b & 7) ^ flag::PV;
            }
        }
        self.set_f(f);
    }
}

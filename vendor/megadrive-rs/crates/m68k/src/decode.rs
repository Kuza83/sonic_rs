//! Opcode decoding through a 64K-entry table built once on first use.
//! Encodings: M68000 Programmer's Reference Manual, chapter 4 and §8.

use std::sync::OnceLock;

use crate::instruction::{
    AluOp, BcdOp, BitNumber, BitOp, Ea, ExgMode, Instruction, MulDivOp, ShiftCount, ShiftKind,
    Size, UnaryOp,
};

const OPCODE_COUNT: usize = 0x1_0000;

const OP_NOP: u16 = 0x4E71;
const OP_RTE: u16 = 0x4E73;
const OP_RTS: u16 = 0x4E75;
const OP_RTR: u16 = 0x4E77;
const OP_RESET: u16 = 0x4E70;
const OP_STOP: u16 = 0x4E72;
const OP_TRAPV: u16 = 0x4E76;

static TABLE: OnceLock<Box<[Instruction]>> = OnceLock::new();

pub fn decode(opcode: u16) -> Instruction {
    TABLE.get_or_init(build_table)[opcode as usize]
}

fn build_table() -> Box<[Instruction]> {
    (0..OPCODE_COUNT).map(|op| decode_slow(op as u16)).collect()
}

/// Field extraction helpers.
fn bits(op: u16, shift: u16, width: u16) -> u16 {
    (op >> shift) & ((1 << width) - 1)
}

/// Source/destination EA in bits 5..0 (mode 5..3, register 2..0).
fn ea_low(op: u16) -> Option<Ea> {
    Ea::decode(bits(op, 3, 3), bits(op, 0, 3))
}

fn reg_high(op: u16) -> u8 {
    bits(op, 9, 3) as u8
}

/// Size field in bits 7..6 with the common 00/01/10 encoding.
fn size_76(op: u16) -> Option<Size> {
    match bits(op, 6, 2) {
        0 => Some(Size::Byte),
        1 => Some(Size::Word),
        2 => Some(Size::Long),
        _ => None,
    }
}

fn decode_slow(op: u16) -> Instruction {
    let decoded = match bits(op, 12, 4) {
        0x0 => decode_immediate(op),
        0x1..=0x3 => decode_move(op),
        0x4 => decode_misc(op),
        0x5 => decode_quick(op),
        0x6 => Some(decode_branch(op)),
        0x7 => decode_moveq(op),
        0x8 => decode_alu_line(op, AluOp::Or),
        0x9 => decode_add_sub(op, AluOp::Sub),
        0xB => decode_cmp_eor(op),
        0xC => decode_exg(op).or_else(|| decode_alu_line(op, AluOp::And)),
        0xD => decode_add_sub(op, AluOp::Add),
        0xA => Some(Instruction::LineA),
        0xE => decode_shift(op),
        0xF => Some(Instruction::LineF),
        _ => None,
    };
    decoded.unwrap_or(Instruction::Illegal)
}

/// MOVE / MOVEA: 00ss DDDd ddmm mrrr (destination register/mode swapped).
fn decode_move(op: u16) -> Option<Instruction> {
    let size = match bits(op, 12, 2) {
        1 => Size::Byte,
        3 => Size::Word,
        2 => Size::Long,
        _ => return None,
    };
    let src = ea_low(op)?;
    if size == Size::Byte && matches!(src, Ea::AddrReg(_)) {
        return None;
    }
    let dst = Ea::decode(bits(op, 6, 3), bits(op, 9, 3))?;
    match dst {
        Ea::AddrReg(r) if size != Size::Byte => Some(Instruction::Movea { size, src, dst: r }),
        _ if dst.is_data_alterable() => Some(Instruction::Move { size, src, dst }),
        _ => None,
    }
}

/// Line 6: BRA (condition 0, "true"), BSR (condition 1), Bcc.
fn decode_branch(op: u16) -> Instruction {
    let cond = bits(op, 8, 4) as u8;
    let disp = op as u8 as i8;
    if cond == 1 {
        Instruction::Bsr { disp }
    } else {
        Instruction::Bcc { cond, disp }
    }
}

fn decode_moveq(op: u16) -> Option<Instruction> {
    if op & 0x0100 != 0 {
        return None;
    }
    Some(Instruction::Moveq {
        value: op as u8 as i8,
        dst: reg_high(op),
    })
}

/// Line 4: miscellaneous single-operand and control instructions.
fn decode_misc(op: u16) -> Option<Instruction> {
    match op {
        OP_NOP => return Some(Instruction::Nop),
        OP_RTE => return Some(Instruction::Rte),
        OP_RTS => return Some(Instruction::Rts),
        OP_RTR => return Some(Instruction::Rtr),
        OP_RESET => return Some(Instruction::Reset),
        OP_STOP => return Some(Instruction::Stop),
        OP_TRAPV => return Some(Instruction::Trapv),
        _ => {}
    }
    if op & 0xFFF0 == 0x4E40 {
        return Some(Instruction::Trap {
            vector: bits(op, 0, 4) as u8,
        });
    }
    // CHK: 0100 rrr1 10mm mrrr (word size on the 68000).
    if op & 0xF1C0 == 0x4180 {
        let src = ea_low(op)?;
        return src.is_data().then_some(Instruction::Chk {
            src,
            dst: reg_high(op),
        });
    }
    let low_reg = bits(op, 0, 3) as u8;
    match op & 0xFFF8 {
        0x4E50 => return Some(Instruction::Link { reg: low_reg }),
        0x4E58 => return Some(Instruction::Unlk { reg: low_reg }),
        0x4E60 => {
            return Some(Instruction::MoveUsp {
                to_usp: true,
                reg: low_reg,
            });
        }
        0x4E68 => {
            return Some(Instruction::MoveUsp {
                to_usp: false,
                reg: low_reg,
            });
        }
        _ => {}
    }
    match op & 0xFFC0 {
        0x4E80 | 0x4EC0 => {
            let dst = ea_low(op)?;
            if !dst.is_control() {
                return None;
            }
            return Some(if op & 0x0040 != 0 {
                Instruction::Jmp { dst }
            } else {
                Instruction::Jsr { dst }
            });
        }
        0x40C0 => {
            let dst = ea_low(op)?;
            return dst
                .is_data_alterable()
                .then_some(Instruction::MoveFromSr { dst });
        }
        0x44C0 | 0x46C0 => {
            let src = ea_low(op)?;
            if !src.is_data() {
                return None;
            }
            return Some(if op & 0x0200 != 0 {
                Instruction::MoveToSr { src }
            } else {
                Instruction::MoveToCcr { src }
            });
        }
        _ => {}
    }
    // LEA: 0100 rrr1 11mm mrrr
    if op & 0xF1C0 == 0x41C0 {
        let src = ea_low(op)?;
        return src.is_control().then_some(Instruction::Lea {
            src,
            dst: reg_high(op),
        });
    }
    match op & 0xFFC0 {
        // SWAP Dn shares PEA's encoding with mode 0.
        0x4840 => {
            return match ea_low(op)? {
                Ea::DataReg(reg) => Some(Instruction::Swap { reg }),
                src if src.is_control() => Some(Instruction::Pea { src }),
                _ => None,
            };
        }
        // EXT Dn shares MOVEM's register-to-memory encoding with mode 0.
        0x4880 | 0x48C0 | 0x4C80 | 0x4CC0 => {
            let size = if op & 0x0040 != 0 {
                Size::Long
            } else {
                Size::Word
            };
            let to_memory = op & 0x0400 == 0;
            let ea = ea_low(op)?;
            let movem = Instruction::Movem {
                size,
                to_memory,
                ea,
            };
            return match ea {
                Ea::DataReg(reg) if to_memory => Some(Instruction::Ext { size, reg }),
                Ea::PreDec(_) if to_memory => Some(movem),
                Ea::PostInc(_) if !to_memory => Some(movem),
                _ if ea.is_control() && (!to_memory || ea.is_alterable()) => Some(movem),
                _ => None,
            };
        }
        _ => {}
    }
    match op & 0xFF00 {
        0x4200 => {
            let size = size_76(op)?;
            let dst = ea_low(op)?;
            dst.is_data_alterable()
                .then_some(Instruction::Clr { size, dst })
        }
        0x4000 | 0x4400 | 0x4600 => {
            let op_kind = match op & 0xFF00 {
                0x4000 => UnaryOp::Negx,
                0x4400 => UnaryOp::Neg,
                _ => UnaryOp::Not,
            };
            let size = size_76(op)?;
            let dst = ea_low(op)?;
            dst.is_data_alterable().then_some(Instruction::Unary {
                op: op_kind,
                size,
                dst,
            })
        }
        0x4800 if bits(op, 6, 2) == 0 => {
            let dst = ea_low(op)?;
            dst.is_data_alterable().then_some(Instruction::Nbcd { dst })
        }
        0x4A00 if bits(op, 6, 2) == 3 => {
            let dst = ea_low(op)?;
            dst.is_data_alterable().then_some(Instruction::Tas { dst })
        }
        0x4A00 => {
            let size = size_76(op)?;
            let dst = ea_low(op)?;
            // The 68000 only accepts data alterable operands for TST (no An, PC-relative, #).
            dst.is_data_alterable()
                .then_some(Instruction::Tst { size, dst })
        }
        _ => None,
    }
}

/// Line 0 immediates: ORI/ANDI/SUBI/ADDI/EORI/CMPI, and their CCR/SR forms.
fn decode_immediate(op: u16) -> Option<Instruction> {
    if op & 0x0100 != 0 {
        // Dynamic bit operations; the An mode encodes MOVEP.
        let dst = ea_low(op)?;
        if let Ea::AddrReg(areg) = dst {
            return Some(Instruction::Movep {
                size: if op & 0x0040 != 0 {
                    Size::Long
                } else {
                    Size::Word
                },
                to_memory: op & 0x0080 != 0,
                dreg: reg_high(op),
                areg,
            });
        }
        return decode_bit(op, BitNumber::Register(reg_high(op)), dst);
    }
    if op & 0xFF00 == 0x0800 {
        let dst = ea_low(op)?;
        if dst == Ea::Immediate {
            return None;
        }
        return decode_bit(op, BitNumber::Immediate, dst);
    }
    let alu = match bits(op, 9, 3) {
        0 => AluOp::Or,
        1 => AluOp::And,
        2 => AluOp::Sub,
        3 => AluOp::Add,
        5 => AluOp::Eor,
        6 => AluOp::Cmp,
        _ => return None,
    };
    let logic = matches!(alu, AluOp::Or | AluOp::And | AluOp::Eor);
    match op & 0x00FF {
        0x3C if logic => return Some(Instruction::CcrImm { op: alu }),
        0x7C if logic => return Some(Instruction::SrImm { op: alu }),
        _ => {}
    }
    let size = size_76(op)?;
    let dst = ea_low(op)?;
    dst.is_data_alterable()
        .then_some(Instruction::AluImm { op: alu, size, dst })
}

/// BTST accepts any data operand (even #imm with a register bit number);
/// BCHG/BCLR/BSET need data alterable ones.
fn decode_bit(op: u16, bit: BitNumber, dst: Ea) -> Option<Instruction> {
    let bit_op = match bits(op, 6, 2) {
        0 => BitOp::Btst,
        1 => BitOp::Bchg,
        2 => BitOp::Bclr,
        _ => BitOp::Bset,
    };
    let valid = if bit_op == BitOp::Btst {
        dst.is_data()
    } else {
        dst.is_data_alterable()
    };
    valid.then_some(Instruction::Bit {
        op: bit_op,
        bit,
        dst,
    })
}

/// ADDQ/SUBQ: 0101 ddds ssmm mrrr; size 11 is Scc (DBcc for An mode, later milestone).
fn decode_quick(op: u16) -> Option<Instruction> {
    if bits(op, 6, 2) == 3 {
        let dst = ea_low(op)?;
        if let Ea::AddrReg(reg) = dst {
            return Some(Instruction::Dbcc {
                cond: bits(op, 8, 4) as u8,
                reg,
            });
        }
        return dst.is_data_alterable().then_some(Instruction::Scc {
            cond: bits(op, 8, 4) as u8,
            dst,
        });
    }
    let size = size_76(op)?;
    let dst = ea_low(op)?;
    if !dst.is_alterable() || (size == Size::Byte && matches!(dst, Ea::AddrReg(_))) {
        return None;
    }
    let data = match bits(op, 9, 3) {
        0 => 8,
        n => n as u8,
    };
    let alu = if op & 0x0100 != 0 {
        AluOp::Sub
    } else {
        AluOp::Add
    };
    Some(Instruction::AluQuick {
        op: alu,
        size,
        data,
        dst,
    })
}

/// Opmode in bits 8..6: 0-2 <ea>,Dn ; 3 word-address form ; 4-6 Dn,<ea> ; 7 long-address form.
fn opmode(op: u16) -> u16 {
    bits(op, 6, 3)
}

fn opmode_size(op: u16) -> Size {
    match opmode(op) & 3 {
        0 => Size::Byte,
        1 => Size::Word,
        _ => Size::Long,
    }
}

/// <ea>,Dn form shared by ADD/SUB/CMP/AND/OR. An sources are byte-invalid,
/// and AND/OR only accept data sources.
fn decode_to_reg(op: u16, alu: AluOp) -> Option<Instruction> {
    let size = opmode_size(op);
    let src = ea_low(op)?;
    let an_ok = matches!(alu, AluOp::Add | AluOp::Sub | AluOp::Cmp) && size != Size::Byte;
    if matches!(src, Ea::AddrReg(_)) && !an_ok {
        return None;
    }
    Some(Instruction::AluToReg {
        op: alu,
        size,
        src,
        dst: reg_high(op),
    })
}

/// Dn,<ea> form: memory alterable destinations (EOR also accepts Dn).
fn decode_to_ea(op: u16, alu: AluOp) -> Option<Instruction> {
    let dst = ea_low(op)?;
    let valid = match dst {
        Ea::DataReg(_) => alu == AluOp::Eor,
        _ => dst.is_data_alterable(),
    };
    valid.then_some(Instruction::AluToEa {
        op: alu,
        size: opmode_size(op),
        src: reg_high(op),
        dst,
    })
}

/// Lines 8 (OR) and C (AND). Opmodes 3/7 are DIVx/MULx and register forms of
/// opmode 4 are SBCD/ABCD (later milestones).
fn decode_alu_line(op: u16, alu: AluOp) -> Option<Instruction> {
    let or_line = alu == AluOp::Or;
    match opmode(op) {
        0..=2 => decode_to_reg(op, alu),
        3 | 7 => {
            let src = ea_low(op)?;
            if !src.is_data() {
                return None;
            }
            let signed = opmode(op) == 7;
            let md = match (or_line, signed) {
                (true, false) => MulDivOp::Divu,
                (true, true) => MulDivOp::Divs,
                (false, false) => MulDivOp::Mulu,
                (false, true) => MulDivOp::Muls,
            };
            Some(Instruction::MulDiv {
                op: md,
                src,
                dst: reg_high(op),
            })
        }
        4 if bits(op, 4, 2) == 0 => Some(Instruction::Bcd {
            op: if or_line { BcdOp::Sbcd } else { BcdOp::Abcd },
            memory: op & 0x0008 != 0,
            rx: reg_high(op),
            ry: bits(op, 0, 3) as u8,
        }),
        4..=6 if bits(op, 4, 2) != 0 => decode_to_ea(op, alu),
        _ => None,
    }
}

/// Lines 9 (SUB) and D (ADD): also ADDA/SUBA and ADDX/SUBX.
fn decode_add_sub(op: u16, alu: AluOp) -> Option<Instruction> {
    match opmode(op) {
        0..=2 => decode_to_reg(op, alu),
        3 | 7 => Some(Instruction::AluAddr {
            op: alu,
            size: if opmode(op) == 3 {
                Size::Word
            } else {
                Size::Long
            },
            src: ea_low(op)?,
            dst: reg_high(op),
        }),
        _ if bits(op, 4, 2) == 0 => Some(Instruction::AluX {
            op: alu,
            size: opmode_size(op),
            memory: op & 0x0008 != 0,
            rx: reg_high(op),
            ry: bits(op, 0, 3) as u8,
        }),
        _ => decode_to_ea(op, alu),
    }
}

/// Line B: CMP, CMPA, CMPM, EOR.
fn decode_cmp_eor(op: u16) -> Option<Instruction> {
    match opmode(op) {
        0..=2 => decode_to_reg(op, AluOp::Cmp),
        3 | 7 => Some(Instruction::AluAddr {
            op: AluOp::Cmp,
            size: if opmode(op) == 3 {
                Size::Word
            } else {
                Size::Long
            },
            src: ea_low(op)?,
            dst: reg_high(op),
        }),
        _ if bits(op, 3, 3) == 1 => Some(Instruction::Cmpm {
            size: opmode_size(op),
            ax: reg_high(op),
            ay: bits(op, 0, 3) as u8,
        }),
        _ => decode_to_ea(op, AluOp::Eor),
    }
}

fn shift_kind(field: u16) -> ShiftKind {
    match field & 3 {
        0 => ShiftKind::Arithmetic,
        1 => ShiftKind::Logical,
        2 => ShiftKind::RotateExtend,
        _ => ShiftKind::Rotate,
    }
}

/// Line E: register form 1110 cccd ssit trrr, memory form 1110 0ttd 11mm mrrr.
fn decode_shift(op: u16) -> Option<Instruction> {
    let left = op & 0x0100 != 0;
    if bits(op, 6, 2) == 3 {
        if op & 0x0800 != 0 {
            return None; // bit-field instructions are 68020+
        }
        let dst = ea_low(op)?;
        let memory_alterable =
            dst.is_alterable() && !matches!(dst, Ea::DataReg(_) | Ea::AddrReg(_));
        return memory_alterable.then_some(Instruction::ShiftMem {
            kind: shift_kind(bits(op, 9, 2)),
            left,
            dst,
        });
    }
    let field = reg_high(op);
    let count = if op & 0x0020 != 0 {
        ShiftCount::Register(field)
    } else {
        ShiftCount::Immediate(if field == 0 { 8 } else { field })
    };
    Some(Instruction::Shift {
        kind: shift_kind(bits(op, 3, 2)),
        left,
        size: size_76(op)?,
        count,
        reg: bits(op, 0, 3) as u8,
    })
}

/// EXG: 1100 xxx1 oooo oyyy with opmode 01000, 01001 or 10001.
fn decode_exg(op: u16) -> Option<Instruction> {
    if op & 0x0100 == 0 {
        return None;
    }
    let mode = match bits(op, 3, 5) {
        0b01000 => ExgMode::DataData,
        0b01001 => ExgMode::AddrAddr,
        0b10001 => ExgMode::DataAddr,
        _ => return None,
    };
    Some(Instruction::Exg {
        mode,
        rx: reg_high(op),
        ry: bits(op, 0, 3) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_nop() {
        assert_eq!(decode(0x4E71), Instruction::Nop);
    }

    #[test]
    fn decodes_move_and_movea() {
        // move.l (a3)+,-(a2)
        assert_eq!(
            decode(0x251B),
            Instruction::Move {
                size: Size::Long,
                src: Ea::PostInc(3),
                dst: Ea::PreDec(2)
            }
        );
        // movea.w d0,a1
        assert_eq!(
            decode(0x3240),
            Instruction::Movea {
                size: Size::Word,
                src: Ea::DataReg(0),
                dst: 1
            }
        );
        // move.b a0,d0 is invalid
        assert_eq!(decode(0x1008), Instruction::Illegal);
        // move.w d0,(d16,pc) is invalid
        assert_eq!(decode(0x35C0), Instruction::Illegal);
    }

    #[test]
    fn swap_pea_ext_disambiguation() {
        assert_eq!(decode(0x4843), Instruction::Swap { reg: 3 });
        assert_eq!(
            decode(0x4853),
            Instruction::Pea {
                src: Ea::Indirect(3)
            }
        );
        assert_eq!(decode(0x485B), Instruction::Illegal); // pea (a3)+
        assert_eq!(
            decode(0x48C2),
            Instruction::Ext {
                size: Size::Long,
                reg: 2
            }
        );
    }

    #[test]
    fn decodes_alu_families() {
        // addq.l #8,a0
        assert_eq!(
            decode(0x5088),
            Instruction::AluQuick {
                op: AluOp::Add,
                size: Size::Long,
                data: 8,
                dst: Ea::AddrReg(0)
            }
        );
        // addq.b #1,a0 is invalid
        assert_eq!(decode(0x5208), Instruction::Illegal);
        // addx.l -(a6),-(a4)
        assert_eq!(
            decode(0xD98E),
            Instruction::AluX {
                op: AluOp::Add,
                size: Size::Long,
                memory: true,
                rx: 4,
                ry: 6
            }
        );
        // cmpm.b (a1)+,(a3)+
        assert_eq!(
            decode(0xB709),
            Instruction::Cmpm {
                size: Size::Byte,
                ax: 3,
                ay: 1
            }
        );
        // eor.l d0,d1 / exg d0,d1 share line C/B patterns
        assert_eq!(
            decode(0xB181),
            Instruction::AluToEa {
                op: AluOp::Eor,
                size: Size::Long,
                src: 0,
                dst: Ea::DataReg(1)
            }
        );
        assert_eq!(decode(0x003C), Instruction::CcrImm { op: AluOp::Or });
        assert_eq!(decode(0x0A7C), Instruction::SrImm { op: AluOp::Eor });
        assert_eq!(decode(0x0C3C), Instruction::Illegal); // cmpi #,ccr does not exist
    }

    #[test]
    fn decodes_control_flow() {
        assert_eq!(decode(0x6000), Instruction::Bcc { cond: 0, disp: 0 });
        assert_eq!(decode(0x61FE), Instruction::Bsr { disp: -2 });
        assert_eq!(decode(0x51C8), Instruction::Dbcc { cond: 1, reg: 0 });
        assert_eq!(
            decode(0x4E90),
            Instruction::Jsr {
                dst: Ea::Indirect(0)
            }
        );
        assert_eq!(decode(0x4ED8), Instruction::Illegal); // jmp (a0)+
        assert_eq!(
            decode(0x48E7),
            Instruction::Movem {
                size: Size::Long,
                to_memory: true,
                ea: Ea::PreDec(7)
            }
        );
        assert_eq!(decode(0x4CE7), Instruction::Illegal); // movem -(a7) to registers
        assert_eq!(
            decode(0x0188),
            Instruction::Movep {
                size: Size::Word,
                to_memory: true,
                dreg: 0,
                areg: 0
            }
        );
        assert_eq!(decode(0x46FC), Instruction::MoveToSr { src: Ea::Immediate });
        assert_eq!(
            decode(0x4E6E),
            Instruction::MoveUsp {
                to_usp: false,
                reg: 6
            }
        );
    }

    #[test]
    fn decodes_muldiv_bcd() {
        assert_eq!(
            decode(0x80EF),
            Instruction::MulDiv {
                op: MulDivOp::Divu,
                src: Ea::Disp16(7),
                dst: 0
            }
        );
        assert_eq!(
            decode(0xC1C1),
            Instruction::MulDiv {
                op: MulDivOp::Muls,
                src: Ea::DataReg(1),
                dst: 0
            }
        );
        assert_eq!(
            decode(0xC30F),
            Instruction::Bcd {
                op: BcdOp::Abcd,
                memory: true,
                rx: 1,
                ry: 7
            }
        );
        assert_eq!(decode(0x4838), Instruction::Nbcd { dst: Ea::AbsShort });
        assert_eq!(decode(0x80C8), Instruction::Illegal); // divu a0,d0
    }

    #[test]
    fn decodes_exceptions() {
        assert_eq!(decode(0x4E4F), Instruction::Trap { vector: 15 });
        assert_eq!(decode(0x4E76), Instruction::Trapv);
        assert_eq!(
            decode(0x43AD),
            Instruction::Chk {
                src: Ea::Disp16(5),
                dst: 1
            }
        );
        assert_eq!(decode(0x4AFC), Instruction::Illegal);
        assert_eq!(decode(0xA123), Instruction::LineA);
        assert_eq!(decode(0xF000), Instruction::LineF);
        assert_eq!(decode(0x4E72), Instruction::Stop);
    }

    /// Regression guard: the number of opcodes decoding to `Illegal` (line A
    /// and line F decode to their own variants). Any decoder change that
    /// moves this number must be explained.
    #[test]
    fn illegal_opcode_count_is_stable() {
        let illegal = (0..=0xFFFFu16)
            .filter(|&op| decode(op) == Instruction::Illegal)
            .count();
        assert_eq!(illegal, 11529);
    }

    #[test]
    fn tst_rejects_address_register() {
        assert_eq!(decode(0x4A48), Instruction::Illegal);
    }
}

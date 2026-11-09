//! 68000 disassembler (Motorola syntax), driven by the shared decoder.

use std::fmt::Write;

use crate::decode::decode;
use crate::ea::sign_extend;
use crate::instruction::{
    AluOp, BitNumber, CONDITION_NAMES, Ea, ExgMode, Instruction, ShiftCount, ShiftKind, Size,
};

/// The officially reserved illegal opcode.
const ILLEGAL_OPCODE: u16 = 0x4AFC;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disassembled {
    pub text: String,
    /// Instruction length in bytes, opcode and extension words included.
    pub len: u32,
}

/// Sequential reader over the instruction stream.
struct Stream<F> {
    next: u32,
    read_word: F,
}

impl<F: FnMut(u32) -> u16> Stream<F> {
    fn word(&mut self) -> u16 {
        let w = (self.read_word)(self.next);
        self.next = self.next.wrapping_add(2);
        w
    }

    fn long(&mut self) -> u32 {
        let hi = self.word() as u32;
        (hi << 16) | self.word() as u32
    }
}

/// Disassembles the instruction at `pc`. `read_word` must be side-effect free
/// (never route it through device registers).
pub fn disassemble(pc: u32, read_word: impl FnMut(u32) -> u16) -> Disassembled {
    let mut s = Stream {
        next: pc,
        read_word,
    };
    let opcode = s.word();
    let text = render(decode(opcode), opcode, &mut s);
    Disassembled {
        text,
        len: s.next.wrapping_sub(pc),
    }
}

fn signed_hex(value: i32) -> String {
    if value < 0 {
        format!("-${:X}", (value as i64).unsigned_abs())
    } else {
        format!("${value:X}")
    }
}

fn index_reg(ext: u16) -> String {
    let kind = if ext & 0x8000 != 0 { 'a' } else { 'd' };
    let size = if ext & 0x0800 != 0 { 'l' } else { 'w' };
    format!("{kind}{}.{size}", (ext >> 12) & 7)
}

fn fmt_ea<F: FnMut(u32) -> u16>(ea: Ea, size: Size, s: &mut Stream<F>) -> String {
    match ea {
        Ea::DataReg(r) => format!("d{r}"),
        Ea::AddrReg(r) => format!("a{r}"),
        Ea::Indirect(r) => format!("(a{r})"),
        Ea::PostInc(r) => format!("(a{r})+"),
        Ea::PreDec(r) => format!("-(a{r})"),
        Ea::Disp16(r) => format!("({},a{r})", signed_hex(s.word() as i16 as i32)),
        Ea::Index8(r) => {
            let ext = s.word();
            format!("({},a{r},{})", signed_hex(ext as i8 as i32), index_reg(ext))
        }
        Ea::AbsShort => format!("(${:04X}).w", s.word()),
        Ea::AbsLong => format!("(${:08X}).l", s.long()),
        Ea::PcDisp16 => {
            let base = s.next;
            let target = base.wrapping_add(sign_extend(s.word() as u32, Size::Word));
            format!("(${target:06X},pc)")
        }
        Ea::PcIndex8 => {
            let base = s.next;
            let ext = s.word();
            let target = base.wrapping_add(sign_extend(ext as u32, Size::Byte));
            format!("(${target:06X},pc,{})", index_reg(ext))
        }
        Ea::Immediate => match size {
            Size::Byte => format!("#${:02X}", s.word() & 0xFF),
            Size::Word => format!("#${:04X}", s.word()),
            Size::Long => format!("#${:08X}", s.long()),
        },
    }
}

fn render<F: FnMut(u32) -> u16>(instr: Instruction, opcode: u16, s: &mut Stream<F>) -> String {
    use Instruction::*;
    let mut out = String::new();
    match instr {
        Illegal if opcode == ILLEGAL_OPCODE => write!(out, "illegal"),
        Illegal => write!(out, "dc.w ${opcode:04X}"),
        Nop => write!(out, "nop"),
        Move { size, src, dst } => {
            let src = fmt_ea(src, size, s);
            let dst = fmt_ea(dst, size, s);
            write!(out, "move.{} {src},{dst}", size.suffix())
        }
        Movea { size, src, dst } => {
            let src = fmt_ea(src, size, s);
            write!(out, "movea.{} {src},a{dst}", size.suffix())
        }
        Moveq { value, dst } => write!(out, "moveq #{},d{dst}", signed_hex(value as i32)),
        Lea { src, dst } => write!(out, "lea {},a{dst}", fmt_ea(src, Size::Long, s)),
        Pea { src } => write!(out, "pea {}", fmt_ea(src, Size::Long, s)),
        Clr { size, dst } => write!(out, "clr.{} {}", size.suffix(), fmt_ea(dst, size, s)),
        Tst { size, dst } => write!(out, "tst.{} {}", size.suffix(), fmt_ea(dst, size, s)),
        Exg { mode, rx, ry } => match mode {
            ExgMode::DataData => write!(out, "exg d{rx},d{ry}"),
            ExgMode::AddrAddr => write!(out, "exg a{rx},a{ry}"),
            ExgMode::DataAddr => write!(out, "exg d{rx},a{ry}"),
        },
        Swap { reg } => write!(out, "swap d{reg}"),
        Ext { size, reg } => write!(out, "ext.{} d{reg}", size.suffix()),
        AluToReg { op, size, src, dst } => {
            let src = fmt_ea(src, size, s);
            write!(out, "{}.{} {src},d{dst}", lower(op), size.suffix())
        }
        AluToEa { op, size, src, dst } => {
            let dst = fmt_ea(dst, size, s);
            write!(out, "{}.{} d{src},{dst}", lower(op), size.suffix())
        }
        AluImm { op, size, dst } => {
            let imm = fmt_ea(Ea::Immediate, size, s);
            let dst = fmt_ea(dst, size, s);
            write!(out, "{}i.{} {imm},{dst}", lower(op), size.suffix())
        }
        AluQuick {
            op,
            size,
            data,
            dst,
        } => {
            let dst = fmt_ea(dst, size, s);
            write!(out, "{}q.{} #{data},{dst}", lower(op), size.suffix())
        }
        AluAddr { op, size, src, dst } => {
            let src = fmt_ea(src, size, s);
            write!(out, "{}a.{} {src},a{dst}", lower(op), size.suffix())
        }
        AluX {
            op,
            size,
            memory,
            rx,
            ry,
        } => {
            let (op, sz) = (lower(op), size.suffix());
            if memory {
                write!(out, "{op}x.{sz} -(a{ry}),-(a{rx})")
            } else {
                write!(out, "{op}x.{sz} d{ry},d{rx}")
            }
        }
        Cmpm { size, ax, ay } => write!(out, "cmpm.{} (a{ay})+,(a{ax})+", size.suffix()),
        Unary { op, size, dst } => {
            let dst = fmt_ea(dst, size, s);
            write!(
                out,
                "{}.{} {dst}",
                op.mnemonic().to_lowercase(),
                size.suffix()
            )
        }
        CcrImm { op } => write!(out, "{}i #${:02X},ccr", lower(op), s.word() & 0xFF),
        SrImm { op } => write!(out, "{}i #${:04X},sr", lower(op), s.word()),
        Shift {
            kind,
            left,
            size,
            count,
            reg,
        } => {
            let count = match count {
                ShiftCount::Immediate(n) => format!("#{n}"),
                ShiftCount::Register(r) => format!("d{r}"),
            };
            write!(
                out,
                "{}.{} {count},d{reg}",
                shift_name(kind, left).to_lowercase(),
                size.suffix()
            )
        }
        ShiftMem { kind, left, dst } => {
            let dst = fmt_ea(dst, Size::Word, s);
            write!(out, "{}.w {dst}", shift_name(kind, left).to_lowercase())
        }
        Bit { op, bit, dst } => {
            let bit = match bit {
                BitNumber::Immediate => format!("#{}", s.word() & 0xFF),
                BitNumber::Register(r) => format!("d{r}"),
            };
            let dst = fmt_ea(dst, Size::Byte, s);
            write!(out, "{} {bit},{dst}", op.mnemonic().to_lowercase())
        }
        Scc { cond, dst } => {
            let dst = fmt_ea(dst, Size::Byte, s);
            write!(out, "s{} {dst}", CONDITION_NAMES[cond as usize])
        }
        Tas { dst } => write!(out, "tas {}", fmt_ea(dst, Size::Byte, s)),
        Bcc { cond, disp } => {
            let name = if cond == 0 {
                "ra"
            } else {
                CONDITION_NAMES[cond as usize]
            };
            let (suffix, target) = branch(disp, s);
            write!(out, "b{name}.{suffix} ${target:06X}")
        }
        Bsr { disp } => {
            let (suffix, target) = branch(disp, s);
            write!(out, "bsr.{suffix} ${target:06X}")
        }
        Dbcc { cond, reg } => {
            let (_, target) = branch(0, s);
            write!(
                out,
                "db{} d{reg},${target:06X}",
                CONDITION_NAMES[cond as usize]
            )
        }
        Jmp { dst } => write!(out, "jmp {}", fmt_ea(dst, Size::Long, s)),
        Jsr { dst } => write!(out, "jsr {}", fmt_ea(dst, Size::Long, s)),
        Rts => write!(out, "rts"),
        Rtr => write!(out, "rtr"),
        Rte => write!(out, "rte"),
        Link { reg } => write!(out, "link a{reg},#{}", signed_hex(s.word() as i16 as i32)),
        Unlk { reg } => write!(out, "unlk a{reg}"),
        Movem {
            size,
            to_memory,
            ea,
        } => {
            let mask = s.word();
            let list = register_list(mask, matches!(ea, Ea::PreDec(_)));
            let ea = fmt_ea(ea, size, s);
            let sz = size.suffix();
            if to_memory {
                write!(out, "movem.{sz} {list},{ea}")
            } else {
                write!(out, "movem.{sz} {ea},{list}")
            }
        }
        Movep {
            size,
            to_memory,
            dreg,
            areg,
        } => {
            let mem = format!("({},a{areg})", signed_hex(s.word() as i16 as i32));
            let sz = size.suffix();
            if to_memory {
                write!(out, "movep.{sz} d{dreg},{mem}")
            } else {
                write!(out, "movep.{sz} {mem},d{dreg}")
            }
        }
        MoveFromSr { dst } => write!(out, "move sr,{}", fmt_ea(dst, Size::Word, s)),
        MoveToCcr { src } => write!(out, "move {},ccr", fmt_ea(src, Size::Word, s)),
        MoveToSr { src } => write!(out, "move {},sr", fmt_ea(src, Size::Word, s)),
        MoveUsp { to_usp, reg } => {
            if to_usp {
                write!(out, "move a{reg},usp")
            } else {
                write!(out, "move usp,a{reg}")
            }
        }
        MulDiv { op, src, dst } => {
            let src = fmt_ea(src, Size::Word, s);
            write!(out, "{} {src},d{dst}", op.mnemonic().to_lowercase())
        }
        Bcd { op, memory, rx, ry } => {
            let name = op.mnemonic().to_lowercase();
            if memory {
                write!(out, "{name} -(a{ry}),-(a{rx})")
            } else {
                write!(out, "{name} d{ry},d{rx}")
            }
        }
        Nbcd { dst } => write!(out, "nbcd {}", fmt_ea(dst, Size::Byte, s)),
        Trap { vector } => write!(out, "trap #{vector}"),
        Trapv => write!(out, "trapv"),
        Chk { src, dst } => write!(out, "chk {},d{dst}", fmt_ea(src, Size::Word, s)),
        Reset => write!(out, "reset"),
        Stop => write!(out, "stop #${:04X}", s.word()),
        LineA | LineF => write!(out, "dc.w ${opcode:04X}"),
    }
    .expect("writing to a String cannot fail");
    out
}

/// Branch size suffix and absolute target. The displacement is relative to
/// the address following the opcode word.
fn branch<F: FnMut(u32) -> u16>(disp: i8, s: &mut Stream<F>) -> (&'static str, u32) {
    let base = s.next;
    if disp == 0 {
        let offset = sign_extend(s.word() as u32, Size::Word);
        ("w", base.wrapping_add(offset))
    } else {
        ("s", base.wrapping_add(disp as i32 as u32))
    }
}

/// MOVEM register list (`d0-d3/a0/a6`). Predecrement masks are bit-reversed.
fn register_list(mask: u16, predec: bool) -> String {
    let mask = if predec { mask.reverse_bits() } else { mask };
    let mut groups = Vec::new();
    for (kind, offset) in [('d', 0), ('a', 8)] {
        let mut i = 0;
        while i < 8 {
            if mask & (1 << (offset + i)) == 0 {
                i += 1;
                continue;
            }
            let start = i;
            while i + 1 < 8 && mask & (1 << (offset + i + 1)) != 0 {
                i += 1;
            }
            groups.push(if start == i {
                format!("{kind}{start}")
            } else {
                format!("{kind}{start}-{kind}{i}")
            });
            i += 1;
        }
    }
    if groups.is_empty() {
        "#0".into()
    } else {
        groups.join("/")
    }
}

fn shift_name(kind: ShiftKind, left: bool) -> String {
    format!("{}{}", kind.mnemonic(), if left { 'L' } else { 'R' })
}

fn lower(op: AluOp) -> String {
    op.mnemonic().to_lowercase()
}

/// Operand shape as written in SingleStepTests names.
fn template_ea(ea: Ea) -> String {
    match ea {
        Ea::DataReg(r) => format!("D{r}"),
        Ea::AddrReg(r) => format!("A{r}"),
        Ea::Indirect(r) => format!("(A{r})"),
        Ea::PostInc(r) => format!("(A{r})+"),
        Ea::PreDec(r) => format!("-(A{r})"),
        Ea::Disp16(r) => format!("(d16, A{r})"),
        Ea::Index8(r) => format!("(d8, A{r}, Xn)"),
        Ea::AbsShort => "(xxx).w".into(),
        Ea::AbsLong => "(xxx).l".into(),
        Ea::PcDisp16 => "(d16, PC)".into(),
        Ea::PcIndex8 => "(d8, PC, Xn)".into(),
        Ea::Immediate => "#".into(),
    }
}

/// Renders the operand-shape template used by SingleStepTests test names
/// (e.g. `ADD.b Q, (d8, A1, Xn)`), to cross-check decoding against them.
/// Returns `None` for opcodes that have no template.
pub fn disassemble_template(opcode: u16) -> Option<String> {
    use Instruction::*;
    let t = template_ea;
    Some(match decode(opcode) {
        Illegal => return None,
        Nop => "NOP".into(),
        Move { size, src, dst } => format!("MOVE.{} {}, {}", size.suffix(), t(src), t(dst)),
        Movea { size, src, dst } => format!("MOVEA.{} {}, A{dst}", size.suffix(), t(src)),
        Moveq { dst, .. } => format!("MOVE.q Q, D{dst}"),
        Lea { src, dst } => format!("LEA {}, A{dst}", t(src)),
        Pea { src } => format!("PEA {}", t(src)),
        Clr { size, dst } => format!("CLR.{} {}", size.suffix(), t(dst)),
        Tst { size, dst } => format!("TST.{} {}", size.suffix(), t(dst)),
        Exg { mode, rx, ry } => match mode {
            ExgMode::DataData => format!("EXG D{rx}, D{ry}"),
            ExgMode::AddrAddr => format!("EXG A{rx}, A{ry}"),
            ExgMode::DataAddr => format!("EXG D{rx}, A{ry}"),
        },
        Swap { reg } => format!("SWAP D{reg}"),
        Ext { size, reg } => format!("EXT.{} D{reg}", size.suffix()),
        AluToReg { op, size, src, dst } => {
            format!("{}.{} {}, D{dst}", op.mnemonic(), size.suffix(), t(src))
        }
        AluToEa { op, size, src, dst } => {
            format!("{}.{} D{src}, {}", op.mnemonic(), size.suffix(), t(dst))
        }
        AluImm { op, size, dst } => format!("{}.{} #, {}", op.mnemonic(), size.suffix(), t(dst)),
        AluQuick { op, size, dst, .. } => {
            format!("{}.{} Q, {}", op.mnemonic(), size.suffix(), t(dst))
        }
        AluAddr { op, size, src, dst } => {
            format!("{}A.{} {}, A{dst}", op.mnemonic(), size.suffix(), t(src))
        }
        AluX {
            op,
            size,
            memory,
            rx,
            ry,
        } => {
            let (op, sz) = (op.mnemonic(), size.suffix());
            if memory {
                format!("{op}X.{sz} -(A{ry}), -(A{rx})")
            } else {
                format!("{op}X.{sz} D{ry}, D{rx}")
            }
        }
        Cmpm { size, ax, ay } => format!("CMP.{} (A{ay})+, (A{ax})+", size.suffix()),
        Unary { op, size, dst } => format!("{}.{} {}", op.mnemonic(), size.suffix(), t(dst)),
        CcrImm { op } => format!("{}ItoCCR #", op.mnemonic()),
        SrImm { op } => format!("{}ItoSR #", op.mnemonic()),
        Shift {
            kind,
            left,
            size,
            count,
            reg,
        } => {
            let count = match count {
                ShiftCount::Immediate(_) => "Q".to_string(),
                ShiftCount::Register(r) => format!("D{r}"),
            };
            format!(
                "{}.{} {count}, D{reg}",
                shift_name(kind, left),
                size.suffix()
            )
        }
        ShiftMem { kind, left, dst } => format!("{}.w {}", shift_name(kind, left), t(dst)),
        Bit { op, bit, dst } => {
            let bit = match bit {
                BitNumber::Immediate => "#".to_string(),
                BitNumber::Register(r) => format!("D{r}"),
            };
            format!("{} {bit}, {}", op.mnemonic(), t(dst))
        }
        Scc { dst, .. } => format!("Scc {}", t(dst)),
        Tas { dst } => format!("TAS {}", t(dst)),
        Bcc { disp, .. } => format!("Bcc {}", if disp == 0 { "#" } else { "Q" }),
        Bsr { disp } => format!("BSR {}", if disp == 0 { "#" } else { "Q" }),
        Dbcc { reg, .. } => format!("DBcc D{reg}, #"),
        Jmp { dst } => format!("JMP {}", t(dst)),
        Jsr { dst } => format!("JSR {}", t(dst)),
        Rts => "RTS".into(),
        Rtr => "RTR".into(),
        Rte => "RTE".into(),
        Link { reg } => format!("LINK A{reg}, #"),
        Unlk { reg } => format!("UNLINK A{reg}"),
        Movem {
            size,
            to_memory,
            ea,
        } => {
            if to_memory {
                format!("MOVEM.{} #, {}", size.suffix(), t(ea))
            } else {
                format!("MOVEM.{} {}, #", size.suffix(), t(ea))
            }
        }
        Movep {
            size,
            to_memory,
            dreg,
            areg,
        } => {
            if to_memory {
                format!("MOVEP.{} D{dreg}, (d16, A{areg})", size.suffix())
            } else {
                format!("MOVEP.{} (d16, A{areg}), D{dreg}", size.suffix())
            }
        }
        MoveFromSr { dst } => format!("MOVEfromSR {}", t(dst)),
        MoveToCcr { src } => format!("MOVEtoCCR {}", t(src)),
        MoveToSr { src } => format!("MOVEtoSR {}", t(src)),
        MoveUsp { to_usp, reg } => {
            if to_usp {
                format!("MOVEtoUSP A{reg}")
            } else {
                format!("MOVEfromUSP A{reg}")
            }
        }
        MulDiv { op, src, dst } => format!("{} {}, D{dst}", op.mnemonic(), t(src)),
        Bcd { op, memory, rx, ry } => {
            if memory {
                format!("{} -(A{ry}), -(A{rx})", op.mnemonic())
            } else {
                format!("{} D{ry}, D{rx}", op.mnemonic())
            }
        }
        Nbcd { dst } => format!("NBCD {}", t(dst)),
        Trap { .. } => "TRAP Q".into(),
        Trapv => "TRAPV".into(),
        Chk { src, dst } => format!("CHK {}, D{dst}", t(src)),
        Reset => "RESET".into(),
        Stop => "STOP".into(),
        LineA | LineF => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(words: &[u16]) -> Disassembled {
        disassemble(0x1000, |addr| {
            words
                .get(((addr - 0x1000) / 2) as usize)
                .copied()
                .unwrap_or(0)
        })
    }

    #[test]
    fn nop() {
        assert_eq!(
            dis(&[0x4E71]),
            Disassembled {
                text: "nop".into(),
                len: 2
            }
        );
        assert_eq!(disassemble_template(0x4E71).as_deref(), Some("NOP"));
    }

    #[test]
    fn unknown_opcode_is_data() {
        assert_eq!(dis(&[0xFFFF]).text, "dc.w $FFFF");
    }

    #[test]
    fn move_with_extension_words() {
        // move.l (8,a3,d1.w),-(a2)
        let d = dis(&[0x2533, 0x1008]);
        assert_eq!(d.text, "move.l ($8,a3,d1.w),-(a2)");
        assert_eq!(d.len, 4);
        // move.w #$1234,($00FF0000).l
        let d = dis(&[0x33FC, 0x1234, 0x00FF, 0x0000]);
        assert_eq!(d.text, "move.w #$1234,($00FF0000).l");
        assert_eq!(d.len, 8);
    }

    #[test]
    fn pc_relative_shows_target() {
        // lea (-2,pc),a0 : base is the extension word address $1002
        assert_eq!(dis(&[0x41FA, 0xFFFE]).text, "lea ($001000,pc),a0");
    }

    #[test]
    fn alu_forms() {
        assert_eq!(dis(&[0xD98E]).text, "addx.l -(a6),-(a4)");
        assert_eq!(dis(&[0xB709]).text, "cmpm.b (a1)+,(a3)+");
        assert_eq!(dis(&[0x5088]).text, "addq.l #8,a0");
        assert_eq!(dis(&[0x0C41, 0x1234]).text, "cmpi.w #$1234,d1");
        assert_eq!(dis(&[0x027C, 0xF8FF]).text, "andi #$F8FF,sr");
        assert_eq!(dis(&[0x4480]).text, "neg.l d0");
        assert_eq!(dis(&[0xD0C9]).text, "adda.w a1,a0");
    }

    #[test]
    fn shift_bit_scc_tas() {
        assert_eq!(dis(&[0xE302]).text, "asl.b #1,d2");
        assert_eq!(dis(&[0xE967]).text, "asl.w d4,d7");
        assert_eq!(dis(&[0xE1E0]).text, "asl.w -(a0)");
        assert_eq!(dis(&[0xE5D5]).text, "roxl.w (a5)");
        assert_eq!(dis(&[0x082D, 0x0003, 0x0010]).text, "btst #3,($10,a5)");
        assert_eq!(dis(&[0x0D02]).text, "btst d6,d2");
        assert_eq!(dis(&[0x57C0]).text, "seq d0");
        assert_eq!(dis(&[0x4AD2]).text, "tas (a2)");
        assert_eq!(disassemble_template(0x013C).as_deref(), Some("BTST D0, #"));
    }

    #[test]
    fn control_flow() {
        assert_eq!(dis(&[0x6000, 0x0010]).text, "bra.w $001012");
        assert_eq!(dis(&[0x67FE]).text, "beq.s $001000");
        assert_eq!(dis(&[0x51C8, 0xFFFE]).text, "dbf d0,$001000");
        assert_eq!(dis(&[0x4EB9, 0x0000, 0x0200]).text, "jsr ($00000200).l");
        assert_eq!(dis(&[0x4E56, 0xFFF0]).text, "link a6,#-$10");
        assert_eq!(dis(&[0x4E75]).text, "rts");
    }

    #[test]
    fn movem_register_lists() {
        // movem.l d0-d3/a0,-(a7): predecrement mask is reversed
        assert_eq!(dis(&[0x48E7, 0xF080]).text, "movem.l d0-d3/a0,-(a7)");
        assert_eq!(dis(&[0x4CDF, 0x0101]).text, "movem.l (a7)+,d0/a0");
        assert_eq!(dis(&[0x0188, 0x0010]).text, "movep.w d0,($10,a0)");
        assert_eq!(dis(&[0x46FC, 0x2700]).text, "move #$2700,sr");
    }

    #[test]
    fn moveq_is_signed() {
        assert_eq!(dis(&[0x70FF]).text, "moveq #-$1,d0");
        assert_eq!(
            disassemble_template(0x7CB5).as_deref(),
            Some("MOVE.q Q, D6")
        );
    }
}

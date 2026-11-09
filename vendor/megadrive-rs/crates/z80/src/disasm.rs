//! Z80 disassembler (Zilog syntax), driven by the shared decoder.

use crate::decode::{decode_base, decode_cb, decode_ed};
use crate::instruction::Instruction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disassembled {
    pub text: String,
    /// Instruction length in bytes.
    pub len: u16,
}

const R: [&str; 8] = ["b", "c", "d", "e", "h", "l", "(hl)", "a"];
const RP: [&str; 4] = ["bc", "de", "hl", "sp"];
const RP2: [&str; 4] = ["bc", "de", "hl", "af"];
const CC: [&str; 8] = ["nz", "z", "nc", "c", "po", "pe", "p", "m"];
const ROT: [&str; 8] = ["rlc", "rrc", "rl", "rr", "sla", "sra", "sll", "srl"];
const ALU: [&str; 8] = [
    "add a,", "adc a,", "sub ", "sbc a,", "and ", "xor ", "or ", "cp ",
];

struct Stream<F> {
    pc: u16,
    read: F,
}

impl<F: FnMut(u16) -> u8> Stream<F> {
    fn byte(&mut self) -> u8 {
        let b = (self.read)(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    fn word(&mut self) -> u16 {
        let lo = self.byte();
        u16::from_le_bytes([lo, self.byte()])
    }

    /// Target of a relative jump whose displacement is the next byte.
    fn relative(&mut self) -> u16 {
        let d = self.byte() as i8;
        self.pc.wrapping_add(d as i16 as u16)
    }
}

/// Disassembles the instruction at `pc`; `read` must be side-effect free.
pub fn disassemble(pc: u16, read: impl FnMut(u16) -> u8) -> Disassembled {
    let mut s = Stream { pc, read };
    let op = s.byte();
    let text = match decode_base(op) {
        Instruction::PrefixDd => indexed_text(&mut s, "ix"),
        Instruction::PrefixFd => indexed_text(&mut s, "iy"),
        instr => base_text(instr, op, &mut s, None),
    };
    Disassembled {
        text,
        len: s.pc.wrapping_sub(pc),
    }
}

/// `(ix+$05)` / `(iy-$03)` from the next byte.
fn displacement<F: FnMut(u16) -> u8>(s: &mut Stream<F>, ix: &str) -> String {
    let d = s.byte() as i8;
    let sign = if d < 0 { '-' } else { '+' };
    format!("({ix}{sign}${:02X})", d.unsigned_abs())
}

fn indexed_text<F: FnMut(u16) -> u8>(s: &mut Stream<F>, ix: &'static str) -> String {
    let op = s.byte();
    match decode_base(op) {
        Instruction::PrefixCb => {
            let mem = displacement(s, ix);
            let op2 = s.byte();
            let copy = match op2 & 7 {
                6 => String::new(),
                r => format!(",{}", R[r as usize]),
            };
            match decode_cb(op2) {
                Instruction::Rot(o, _) => format!("{} {mem}{copy}", ROT[o as usize]),
                Instruction::Bit(b, _) => format!("bit {b},{mem}"),
                Instruction::Res(b, _) => format!("res {b},{mem}{copy}"),
                Instruction::Set(b, _) => format!("set {b},{mem}{copy}"),
                _ => unreachable!("CB page decodes to CB instructions"),
            }
        }
        Instruction::PrefixDd | Instruction::PrefixFd | Instruction::PrefixEd => {
            format!("db ${:02X}", if ix == "ix" { 0xDD } else { 0xFD })
        }
        instr => base_text(instr, op, s, Some(ix)),
    }
}

/// Unprefixed instruction text; `idx` substitutes HL/H/L/(HL) for a DD/FD form.
fn base_text<F: FnMut(u16) -> u8>(
    instr: Instruction,
    op: u8,
    s: &mut Stream<F>,
    idx: Option<&'static str>,
) -> String {
    use Instruction::*;
    let hl = idx.unwrap_or("hl");
    // `real`: H/L stay H/L (the other operand is (IX+d)).
    let reg = |s: &mut Stream<F>, r: u8, real: bool| -> String {
        match (r, idx) {
            (6, Some(ix)) => displacement(s, ix),
            (4, Some(ix)) if !real => format!("{ix}h"),
            (5, Some(ix)) if !real => format!("{ix}l"),
            _ => R[r as usize].to_string(),
        }
    };
    let rp = |p: u8| if p == 2 { hl } else { RP[p as usize] };
    let rp2 = |p: u8| if p == 2 { hl } else { RP2[p as usize] };
    match instr {
        PrefixCb => {
            let op2 = s.byte();
            match decode_cb(op2) {
                Rot(o, r) => format!("{} {}", ROT[o as usize], R[r as usize]),
                Bit(b, r) => format!("bit {b},{}", R[r as usize]),
                Res(b, r) => format!("res {b},{}", R[r as usize]),
                Set(b, r) => format!("set {b},{}", R[r as usize]),
                _ => unreachable!("CB page decodes to CB instructions"),
            }
        }
        PrefixEd => {
            let op2 = s.byte();
            ed_text(decode_ed(op2), op2, s)
        }
        Unimplemented | PrefixDd | PrefixFd => format!("db ${op:02X}"),
        Nop => "nop".into(),
        ExAfAf => "ex af,af'".into(),
        Djnz => format!("djnz ${:04X}", s.relative()),
        Jr => format!("jr ${:04X}", s.relative()),
        JrCc(cc) => format!("jr {},${:04X}", CC[cc as usize], s.relative()),
        LdRpNn(p) => format!("ld {},${:04X}", rp(p), s.word()),
        AddHlRp(p) => format!("add {hl},{}", rp(p)),
        LdIndirectA(p) => format!("ld ({}),a", RP[p as usize]),
        LdAIndirect(p) => format!("ld a,({})", RP[p as usize]),
        LdNnHl => format!("ld (${:04X}),{hl}", s.word()),
        LdHlNn => format!("ld {hl},(${:04X})", s.word()),
        LdNnA => format!("ld (${:04X}),a", s.word()),
        LdANn => format!("ld a,(${:04X})", s.word()),
        IncRp(p) => format!("inc {}", rp(p)),
        DecRp(p) => format!("dec {}", rp(p)),
        IncR(r) => format!("inc {}", reg(s, r, false)),
        DecR(r) => format!("dec {}", reg(s, r, false)),
        LdRN(r) => {
            let dst = reg(s, r, false);
            format!("ld {dst},${:02X}", s.byte())
        }
        Rlca => "rlca".into(),
        Rrca => "rrca".into(),
        Rla => "rla".into(),
        Rra => "rra".into(),
        Daa => "daa".into(),
        Cpl => "cpl".into(),
        Scf => "scf".into(),
        Ccf => "ccf".into(),
        Halt => "halt".into(),
        LdRR(d, r) => {
            let mem = d == 6 || r == 6;
            let dst = reg(s, d, mem);
            let src = reg(s, r, mem);
            format!("ld {dst},{src}")
        }
        Alu(o, r) => format!("{}{}", ALU[o as usize], reg(s, r, false)),
        AluN(o) => format!("{}${:02X}", ALU[o as usize], s.byte()),
        RetCc(cc) => format!("ret {}", CC[cc as usize]),
        Pop(p) => format!("pop {}", rp2(p)),
        Ret => "ret".into(),
        Exx => "exx".into(),
        JpHl => format!("jp ({hl})"),
        LdSpHl => format!("ld sp,{hl}"),
        JpCc(cc) => format!("jp {},${:04X}", CC[cc as usize], s.word()),
        Jp => format!("jp ${:04X}", s.word()),
        OutNA => format!("out (${:02X}),a", s.byte()),
        InAN => format!("in a,(${:02X})", s.byte()),
        ExSpHl => format!("ex (sp),{hl}"),
        ExDeHl => "ex de,hl".into(),
        Di => "di".into(),
        Ei => "ei".into(),
        CallCc(cc) => format!("call {},${:04X}", CC[cc as usize], s.word()),
        Push(p) => format!("push {}", rp2(p)),
        Call => format!("call ${:04X}", s.word()),
        Rst(y) => format!("rst ${:02X}", y * 8),
        _ => unreachable!("not an unprefixed instruction"),
    }
}

const BLOCK: [[&str; 4]; 4] = [
    ["ldi", "cpi", "ini", "outi"],
    ["ldd", "cpd", "ind", "outd"],
    ["ldir", "cpir", "inir", "otir"],
    ["lddr", "cpdr", "indr", "otdr"],
];

fn ed_text<F: FnMut(u16) -> u8>(instr: Instruction, op: u8, s: &mut Stream<F>) -> String {
    use Instruction::*;
    match instr {
        InRC(6) => "in (c)".into(),
        InRC(r) => format!("in {},(c)", R[r as usize]),
        OutCR(6) => "out (c),0".into(),
        OutCR(r) => format!("out (c),{}", R[r as usize]),
        SbcHlRp(p) => format!("sbc hl,{}", RP[p as usize]),
        AdcHlRp(p) => format!("adc hl,{}", RP[p as usize]),
        LdNnRp(p) => format!("ld (${:04X}),{}", s.word(), RP[p as usize]),
        LdRpNnInd(p) => format!("ld {},(${:04X})", RP[p as usize], s.word()),
        Neg => "neg".into(),
        Retn => "retn".into(),
        Reti => "reti".into(),
        Im(m) => format!("im {m}"),
        LdIA => "ld i,a".into(),
        LdRA => "ld r,a".into(),
        LdAI => "ld a,i".into(),
        LdAR => "ld a,r".into(),
        Rrd => "rrd".into(),
        Rld => "rld".into(),
        Block {
            kind,
            decrement,
            repeat,
        } => BLOCK[(repeat as usize) * 2 + decrement as usize][kind as usize].into(),
        _ => format!("db $ED,${op:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(bytes: &[u8]) -> Disassembled {
        disassemble(0x100, |a| {
            bytes.get((a - 0x100) as usize).copied().unwrap_or(0)
        })
    }

    #[test]
    fn base_forms() {
        assert_eq!(dis(&[0x00]).text, "nop");
        assert_eq!(dis(&[0x21, 0x34, 0x12]).text, "ld hl,$1234");
        assert_eq!(dis(&[0x21, 0x34, 0x12]).len, 3);
        assert_eq!(dis(&[0x18, 0xFE]).text, "jr $0100");
        assert_eq!(dis(&[0x86]).text, "add a,(hl)");
        assert_eq!(dis(&[0xFE, 0x10]).text, "cp $10");
        assert_eq!(dis(&[0xCD, 0x00, 0x80]).text, "call $8000");
        assert_eq!(dis(&[0xFF]).text, "rst $38");
        assert_eq!(dis(&[0xCB, 0x7E]).text, "bit 7,(hl)");
        assert_eq!(dis(&[0xCB, 0x30]).text, "sll b");
        assert_eq!(dis(&[0xCB, 0x30]).len, 2);
        assert_eq!(dis(&[0xED, 0xB0]).text, "ldir");
        assert_eq!(dis(&[0xED, 0x43, 0x00, 0x40]).text, "ld ($4000),bc");
        assert_eq!(dis(&[0xED, 0x70]).text, "in (c)");
        assert_eq!(dis(&[0xED, 0x00]).text, "db $ED,$00");
        assert_eq!(dis(&[0xDD, 0x7E, 0x05]).text, "ld a,(ix+$05)");
        assert_eq!(dis(&[0xFD, 0x66, 0xFD]).text, "ld h,(iy-$03)");
        assert_eq!(dis(&[0xDD, 0x36, 0x02, 0x99]).text, "ld (ix+$02),$99");
        assert_eq!(dis(&[0xDD, 0x36, 0x02, 0x99]).len, 4);
        assert_eq!(dis(&[0xDD, 0x24]).text, "inc ixh");
        assert_eq!(dis(&[0xDD, 0xE9]).text, "jp (ix)");
        assert_eq!(dis(&[0xFD, 0xCB, 0x01, 0x46]).text, "bit 0,(iy+$01)");
        assert_eq!(dis(&[0xDD, 0xCB, 0x01, 0x00]).text, "rlc (ix+$01),b");
    }
}

//! Opcode decoding: one 256-entry table per prefix, built once.
//! Field naming after "Decoding Z80 opcodes" (C. Dinu): x = bits 7-6,
//! y = bits 5-3, z = bits 2-0, p = y >> 1, q = y & 1.

use std::sync::OnceLock;

use crate::instruction::Instruction;

static BASE: OnceLock<[Instruction; 256]> = OnceLock::new();
static CB: OnceLock<[Instruction; 256]> = OnceLock::new();

static ED: OnceLock<[Instruction; 256]> = OnceLock::new();

pub fn decode_ed(opcode: u8) -> Instruction {
    ED.get_or_init(|| std::array::from_fn(|op| decode_ed_slow(op as u8)))[opcode as usize]
}

/// IM operand for y = 0..7 (undocumented IM 0/1 duplicates: y = 1, 5 → 0).
const IM_MODES: [u8; 8] = [0, 0, 1, 2, 0, 0, 1, 2];

fn decode_ed_slow(op: u8) -> Instruction {
    use Instruction::*;
    let (x, y, z, p, q) = fields(op);
    match (x, z) {
        (1, 0) => InRC(y),
        (1, 1) => OutCR(y),
        (1, 2) => {
            if q == 0 {
                SbcHlRp(p)
            } else {
                AdcHlRp(p)
            }
        }
        (1, 3) => {
            if q == 0 {
                LdNnRp(p)
            } else {
                LdRpNnInd(p)
            }
        }
        (1, 4) => Neg,
        (1, 5) => {
            if y == 1 {
                Reti
            } else {
                Retn
            }
        }
        (1, 6) => Im(IM_MODES[y as usize]),
        (1, 7) => [LdIA, LdRA, LdAI, LdAR, Rrd, Rld, EdNop, EdNop][y as usize],
        (2, 0..=3) if y >= 4 => Block {
            kind: z,
            decrement: y & 1 != 0,
            repeat: y >= 6,
        },
        _ => EdNop,
    }
}

/// CB page: x = 0 rotate/shift, 1 BIT, 2 RES, 3 SET; y = operation/bit; z = register.
pub fn decode_cb(opcode: u8) -> Instruction {
    CB.get_or_init(|| {
        std::array::from_fn(|op| {
            let (x, y, z, _, _) = fields(op as u8);
            match x {
                0 => Instruction::Rot(y, z),
                1 => Instruction::Bit(y, z),
                2 => Instruction::Res(y, z),
                _ => Instruction::Set(y, z),
            }
        })
    })[opcode as usize]
}

pub fn decode_base(opcode: u8) -> Instruction {
    BASE.get_or_init(|| std::array::from_fn(|op| decode_base_slow(op as u8)))[opcode as usize]
}

fn fields(op: u8) -> (u8, u8, u8, u8, u8) {
    let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
    (x, y, z, y >> 1, y & 1)
}

fn decode_base_slow(op: u8) -> Instruction {
    use Instruction::*;
    let (x, y, z, p, q) = fields(op);
    match (x, z) {
        (0, 0) => match y {
            0 => Nop,
            1 => ExAfAf,
            2 => Djnz,
            3 => Jr,
            _ => JrCc(y - 4),
        },
        (0, 1) => {
            if q == 0 {
                LdRpNn(p)
            } else {
                AddHlRp(p)
            }
        }
        (0, 2) => match (q, p) {
            (0, 0 | 1) => LdIndirectA(p),
            (0, 2) => LdNnHl,
            (0, _) => LdNnA,
            (_, 0 | 1) => LdAIndirect(p),
            (_, 2) => LdHlNn,
            _ => LdANn,
        },
        (0, 3) => {
            if q == 0 {
                IncRp(p)
            } else {
                DecRp(p)
            }
        }
        (0, 4) => IncR(y),
        (0, 5) => DecR(y),
        (0, 6) => LdRN(y),
        (0, _) => [Rlca, Rrca, Rla, Rra, Daa, Cpl, Scf, Ccf][y as usize],
        (1, _) if y == 6 && z == 6 => Halt,
        (1, _) => LdRR(y, z),
        (2, _) => Alu(y, z),
        (_, 0) => RetCc(y),
        (_, 1) => match (q, p) {
            (0, _) => Pop(p),
            (_, 0) => Ret,
            (_, 1) => Exx,
            (_, 2) => JpHl,
            _ => LdSpHl,
        },
        (_, 2) => JpCc(y),
        (_, 3) => [Jp, PrefixCb, OutNA, InAN, ExSpHl, ExDeHl, Di, Ei][y as usize],
        (_, 4) => CallCc(y),
        (_, 5) => match (q, p) {
            (0, _) => Push(p),
            (_, 0) => Call,
            (_, 1) => PrefixDd,
            (_, 2) => PrefixEd,
            _ => PrefixFd,
        },
        (_, 6) => AluN(y),
        _ => Rst(y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_checks() {
        assert_eq!(decode_base(0x00), Instruction::Nop);
        assert_eq!(decode_base(0x76), Instruction::Halt);
        assert_eq!(decode_base(0x41), Instruction::LdRR(0, 1)); // ld b,c
        assert_eq!(decode_base(0x86), Instruction::Alu(0, 6)); // add a,(hl)
        assert_eq!(decode_base(0xCB), Instruction::PrefixCb);
        assert_eq!(decode_base(0xFF), Instruction::Rst(7));
        assert_eq!(decode_base(0x2A), Instruction::LdHlNn);
        assert_eq!(decode_base(0xF9), Instruction::LdSpHl);
    }
}

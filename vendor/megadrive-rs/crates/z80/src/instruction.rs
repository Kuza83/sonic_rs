//! Decoded instruction representation, shared by the executor and the
//! disassembler. Register fields keep the Z80 encodings:
//! - `r`: 0 B, 1 C, 2 D, 3 E, 4 H, 5 L, 6 (HL), 7 A
//! - `rp`: 0 BC, 1 DE, 2 HL, 3 SP; `rp2` (PUSH/POP): 0 BC, 1 DE, 2 HL, 3 AF
//! - `cc`: 0 NZ, 1 Z, 2 NC, 3 C, 4 PO, 5 PE, 6 P, 7 M
//! - `alu`: 0 ADD, 1 ADC, 2 SUB, 3 SBC, 4 AND, 5 XOR, 6 OR, 7 CP
//! - `rot`: 0 RLC, 1 RRC, 2 RL, 3 RR, 4 SLA, 5 SRA, 6 SLL, 7 SRL
//!
//! HL, H, L and (HL) become IX/IY, IXH/IXL and (IX+d)/(IY+d) under a DD/FD
//! prefix; that substitution is done at execution time.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    /// Not implemented yet (temporary during J8 development).
    Unimplemented,
    Nop,
    ExAfAf,
    Djnz,
    Jr,
    /// JR cc,d for cc 0-3.
    JrCc(u8),
    LdRpNn(u8),
    AddHlRp(u8),
    /// LD (BC),A / LD (DE),A (rp 0/1).
    LdIndirectA(u8),
    /// LD A,(BC) / LD A,(DE).
    LdAIndirect(u8),
    LdNnHl,
    LdHlNn,
    LdNnA,
    LdANn,
    IncRp(u8),
    DecRp(u8),
    IncR(u8),
    DecR(u8),
    LdRN(u8),
    Rlca,
    Rrca,
    Rla,
    Rra,
    Daa,
    Cpl,
    Scf,
    Ccf,
    Halt,
    LdRR(u8, u8),
    Alu(u8, u8),
    AluN(u8),
    RetCc(u8),
    Pop(u8),
    Ret,
    Exx,
    JpHl,
    LdSpHl,
    JpCc(u8),
    Jp,
    OutNA,
    InAN,
    ExSpHl,
    ExDeHl,
    Di,
    Ei,
    CallCc(u8),
    Push(u8),
    Call,
    Rst(u8),
    PrefixCb,
    PrefixDd,
    PrefixEd,
    PrefixFd,
    // --- CB page ---
    /// Rotate/shift `rot` of register `r`.
    Rot(u8, u8),
    /// BIT b,r
    Bit(u8, u8),
    /// RES b,r
    Res(u8, u8),
    /// SET b,r
    Set(u8, u8),
    // --- ED page ---
    /// IN r,(C); r = 6 is IN (C) (flags only).
    InRC(u8),
    /// OUT (C),r; r = 6 outputs 0.
    OutCR(u8),
    SbcHlRp(u8),
    AdcHlRp(u8),
    LdNnRp(u8),
    LdRpNnInd(u8),
    Neg,
    Retn,
    Reti,
    /// IM 0/1/2.
    Im(u8),
    LdIA,
    LdRA,
    LdAI,
    LdAR,
    Rrd,
    Rld,
    /// Block instruction: `kind` 0 LD, 1 CP, 2 IN, 3 OUT; `decrement`; `repeat`.
    Block {
        kind: u8,
        decrement: bool,
        repeat: bool,
    },
    /// Undefined ED opcode: an 8 T-state NOP.
    EdNop,
}

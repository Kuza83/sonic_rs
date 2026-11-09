//! Decoded instruction representation, shared by the executor and the disassembler.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Byte,
    Word,
    Long,
}

impl Size {
    pub const fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    pub const fn mask(self) -> u32 {
        match self {
            Size::Byte => 0xFF,
            Size::Word => 0xFFFF,
            Size::Long => 0xFFFF_FFFF,
        }
    }

    pub const fn msb(self) -> u32 {
        match self {
            Size::Byte => 0x80,
            Size::Word => 0x8000,
            Size::Long => 0x8000_0000,
        }
    }

    pub const fn suffix(self) -> &'static str {
        match self {
            Size::Byte => "b",
            Size::Word => "w",
            Size::Long => "l",
        }
    }
}

/// Effective address, as encoded by the mode/register fields (extension
/// words not yet fetched).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ea {
    DataReg(u8),
    AddrReg(u8),
    Indirect(u8),
    PostInc(u8),
    PreDec(u8),
    Disp16(u8),
    Index8(u8),
    AbsShort,
    AbsLong,
    PcDisp16,
    PcIndex8,
    Immediate,
}

impl Ea {
    /// Decodes the 3-bit mode and register fields (M68000 PRM §2.2).
    pub fn decode(mode: u16, reg: u16) -> Option<Ea> {
        let r = reg as u8;
        Some(match mode {
            0 => Ea::DataReg(r),
            1 => Ea::AddrReg(r),
            2 => Ea::Indirect(r),
            3 => Ea::PostInc(r),
            4 => Ea::PreDec(r),
            5 => Ea::Disp16(r),
            6 => Ea::Index8(r),
            7 => match reg {
                0 => Ea::AbsShort,
                1 => Ea::AbsLong,
                2 => Ea::PcDisp16,
                3 => Ea::PcIndex8,
                4 => Ea::Immediate,
                _ => return None,
            },
            _ => return None,
        })
    }

    // Addressing categories, M68000 PRM Table 2-4.

    pub fn is_data(self) -> bool {
        !matches!(self, Ea::AddrReg(_))
    }

    pub fn is_alterable(self) -> bool {
        !matches!(self, Ea::PcDisp16 | Ea::PcIndex8 | Ea::Immediate)
    }

    pub fn is_data_alterable(self) -> bool {
        self.is_data() && self.is_alterable()
    }

    pub fn is_control(self) -> bool {
        matches!(
            self,
            Ea::Indirect(_)
                | Ea::Disp16(_)
                | Ea::Index8(_)
                | Ea::AbsShort
                | Ea::AbsLong
                | Ea::PcDisp16
                | Ea::PcIndex8
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExgMode {
    DataData,
    AddrAddr,
    DataAddr,
}

/// Two-operand ALU operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AluOp {
    Add,
    Sub,
    Cmp,
    And,
    Or,
    Eor,
}

impl AluOp {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            AluOp::Add => "ADD",
            AluOp::Sub => "SUB",
            AluOp::Cmp => "CMP",
            AluOp::And => "AND",
            AluOp::Or => "OR",
            AluOp::Eor => "EOR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Negx,
    Not,
}

impl UnaryOp {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            UnaryOp::Neg => "NEG",
            UnaryOp::Negx => "NEGX",
            UnaryOp::Not => "NOT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftKind {
    /// ASL/ASR
    Arithmetic,
    /// LSL/LSR
    Logical,
    /// ROXL/ROXR (through X)
    RotateExtend,
    /// ROL/ROR
    Rotate,
}

impl ShiftKind {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            ShiftKind::Arithmetic => "AS",
            ShiftKind::Logical => "LS",
            ShiftKind::RotateExtend => "ROX",
            ShiftKind::Rotate => "RO",
        }
    }
}

/// Shift count of the register forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftCount {
    /// 1..=8 encoded in the opcode.
    Immediate(u8),
    /// Count in Dn, modulo 64.
    Register(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOp {
    Btst,
    Bchg,
    Bclr,
    Bset,
}

impl BitOp {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            BitOp::Btst => "BTST",
            BitOp::Bchg => "BCHG",
            BitOp::Bclr => "BCLR",
            BitOp::Bset => "BSET",
        }
    }
}

/// Bit number source: immediate extension word or data register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitNumber {
    Immediate,
    Register(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulDivOp {
    Mulu,
    Muls,
    Divu,
    Divs,
}

impl MulDivOp {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            MulDivOp::Mulu => "MULU",
            MulDivOp::Muls => "MULS",
            MulDivOp::Divu => "DIVU",
            MulDivOp::Divs => "DIVS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BcdOp {
    Abcd,
    Sbcd,
}

impl BcdOp {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            BcdOp::Abcd => "ABCD",
            BcdOp::Sbcd => "SBCD",
        }
    }
}

/// Condition code mnemonics, indexed by the 4-bit condition field (M68000 PRM Table 3-19).
pub const CONDITION_NAMES: [&str; 16] = [
    "t", "f", "hi", "ls", "cc", "cs", "ne", "eq", "vc", "vs", "pl", "mi", "ge", "lt", "gt", "le",
];

/// One decoded opcode word. Extension words are fetched at execution or
/// disassembly time; only what the opcode word encodes is stored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    /// Opcode not (yet) recognized: raises the illegal-instruction exception.
    Illegal,
    Nop,
    Move {
        size: Size,
        src: Ea,
        dst: Ea,
    },
    Movea {
        size: Size,
        src: Ea,
        dst: u8,
    },
    Moveq {
        value: i8,
        dst: u8,
    },
    Lea {
        src: Ea,
        dst: u8,
    },
    Pea {
        src: Ea,
    },
    Clr {
        size: Size,
        dst: Ea,
    },
    Tst {
        size: Size,
        dst: Ea,
    },
    Exg {
        mode: ExgMode,
        rx: u8,
        ry: u8,
    },
    Swap {
        reg: u8,
    },
    Ext {
        size: Size,
        reg: u8,
    },
    /// ADD/SUB/CMP/AND/OR <ea>,Dn
    AluToReg {
        op: AluOp,
        size: Size,
        src: Ea,
        dst: u8,
    },
    /// ADD/SUB/AND/OR/EOR Dn,<ea>
    AluToEa {
        op: AluOp,
        size: Size,
        src: u8,
        dst: Ea,
    },
    /// ADDI/SUBI/CMPI/ANDI/ORI/EORI #,<ea>
    AluImm {
        op: AluOp,
        size: Size,
        dst: Ea,
    },
    /// ADDQ/SUBQ #data,<ea> (data 1..=8)
    AluQuick {
        op: AluOp,
        size: Size,
        data: u8,
        dst: Ea,
    },
    /// ADDA/SUBA/CMPA <ea>,An
    AluAddr {
        op: AluOp,
        size: Size,
        src: Ea,
        dst: u8,
    },
    /// ADDX/SUBX Dy,Dx or -(Ay),-(Ax)
    AluX {
        op: AluOp,
        size: Size,
        memory: bool,
        rx: u8,
        ry: u8,
    },
    /// CMPM (Ay)+,(Ax)+
    Cmpm {
        size: Size,
        ax: u8,
        ay: u8,
    },
    Unary {
        op: UnaryOp,
        size: Size,
        dst: Ea,
    },
    /// ANDI/ORI/EORI #,CCR
    CcrImm {
        op: AluOp,
    },
    /// ANDI/ORI/EORI #,SR (privileged)
    SrImm {
        op: AluOp,
    },
    /// Register shifts/rotates.
    Shift {
        kind: ShiftKind,
        left: bool,
        size: Size,
        count: ShiftCount,
        reg: u8,
    },
    /// Memory shifts/rotates: word, by one bit.
    ShiftMem {
        kind: ShiftKind,
        left: bool,
        dst: Ea,
    },
    Bit {
        op: BitOp,
        bit: BitNumber,
        dst: Ea,
    },
    Scc {
        cond: u8,
        dst: Ea,
    },
    Tas {
        dst: Ea,
    },
    /// Bcc/BRA; `disp == 0` means a 16-bit displacement in the extension word.
    Bcc {
        cond: u8,
        disp: i8,
    },
    Bsr {
        disp: i8,
    },
    Dbcc {
        cond: u8,
        reg: u8,
    },
    Jmp {
        dst: Ea,
    },
    Jsr {
        dst: Ea,
    },
    Rts,
    Rtr,
    Rte,
    Link {
        reg: u8,
    },
    Unlk {
        reg: u8,
    },
    Movem {
        size: Size,
        to_memory: bool,
        ea: Ea,
    },
    Movep {
        size: Size,
        to_memory: bool,
        dreg: u8,
        areg: u8,
    },
    MoveFromSr {
        dst: Ea,
    },
    MoveToCcr {
        src: Ea,
    },
    MoveToSr {
        src: Ea,
    },
    MoveUsp {
        to_usp: bool,
        reg: u8,
    },
    /// MULU/MULS/DIVU/DIVS <ea>,Dn (word source).
    MulDiv {
        op: MulDivOp,
        src: Ea,
        dst: u8,
    },
    /// ABCD/SBCD Dy,Dx or -(Ay),-(Ax)
    Bcd {
        op: BcdOp,
        memory: bool,
        rx: u8,
        ry: u8,
    },
    Nbcd {
        dst: Ea,
    },
    /// TRAP #vector (0..=15)
    Trap {
        vector: u8,
    },
    Trapv,
    /// CHK <ea>,Dn (word)
    Chk {
        src: Ea,
        dst: u8,
    },
    Reset,
    Stop,
    /// $Axxx opcodes (line 1010 emulator exception)
    LineA,
    /// $Fxxx opcodes (line 1111 emulator exception)
    LineF,
}

//! Motorola 68000 CPU core, generic over a [`Bus`].

mod bus;
mod cpu;
mod decode;
mod disasm;
mod ea;
mod exceptions;
mod exec;
mod instruction;
mod registers;

pub use bus::{ADDRESS_MASK, Bus, FlatBus};
pub use cpu::{Cpu, CpuState};
pub use decode::decode;
pub use disasm::{Disassembled, disassemble, disassemble_template};
pub use instruction::{AluOp, BitNumber, BitOp, BcdOp, Ea, ExgMode, Instruction, MulDivOp, ShiftCount, ShiftKind, Size, UnaryOp};
pub use registers::{Registers, sr};

/// Data-dependent instruction timings (register-source totals, in cycles).
pub mod timing {
    pub use crate::exec::muldiv::{divs_cycles, divu_cycles};
}

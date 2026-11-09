//! 68000 execution core: state, bus access helpers and the step loop.
//!
//! Prefetch model: the 68000 keeps a two-word queue (IR/IRC). At an instruction
//! boundary `regs.pc` is the address of the next opcode, `prefetch[0]` holds
//! that opcode and `prefetch[1]` the word that follows it. This matches the
//! state exposed by SingleStepTests/680x0 ("pc is the formal program counter",
//! "prefetch is the current contents of the prefetch queue").
//!
//! Every word fetch shifts the queue and advances `regs.pc` by 2, so during
//! execution `regs.pc + 2` is the address of the word in `prefetch[1]`. The
//! value of `regs.pc` at the time of a fault is what the 68000 stacks.

use crate::bus::Bus;
use crate::decode::decode;
use crate::instruction::Size;
use crate::registers::{Registers, sr};

/// Cycles taken by one bus read or write with immediate DTACK.
pub(crate) const BUS_CYCLE: u32 = 4;

/// Function codes (FC2..FC0) posted on the bus (M68000 UM §3.6).
pub(crate) mod fc {
    pub const USER_DATA: u8 = 1;
    pub const USER_PROGRAM: u8 = 2;
    pub const SUPERVISOR_DATA: u8 = 5;
    pub const SUPERVISOR_PROGRAM: u8 = 6;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CpuState {
    #[default]
    Running,
    /// After STOP: idle until an interrupt above the mask arrives.
    Stopped,
    /// Double bus fault: the CPU no longer executes until reset.
    Halted,
}

/// Status register value after reset: supervisor, interrupts masked (level 7).
const RESET_SR: u16 = 0x2700;
/// Reset vectors: initial SSP at $000000, initial PC at $000004.
const RESET_SSP_VECTOR: u32 = 0x0;
const RESET_PC_VECTOR: u32 = 0x4;
/// Level 7 is non-maskable and edge-triggered.
const NMI_LEVEL: u8 = 7;

/// A bus cycle that aborted the current instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Word or long access at an odd address. `not_instruction` is the SSW
    /// I/N bit.
    Address {
        addr: u32,
        write: bool,
        fc: u8,
        not_instruction: bool,
    },
}

pub(crate) type Res<T = ()> = Result<T, Fault>;

#[derive(Debug, Clone, Default)]
pub struct Cpu {
    pub regs: Registers,
    pub prefetch: [u16; 2],
    /// Opcode of the instruction being executed.
    pub(crate) ir: u16,
    pub(crate) state: CpuState,
    /// Cycles consumed by the instruction being executed.
    cycles: u32,
    /// Interrupt priority level currently driven on IPL2..IPL0.
    irq_level: u8,
    /// Level 7 is edge-triggered: set on a transition to 7, cleared when taken.
    nmi_pending: bool,
    /// Set when the current step entered a group 1/2 exception (no trace then).
    pub(crate) exception_taken: bool,
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> CpuState {
        self.state
    }

    pub(crate) fn halt(&mut self) {
        self.state = CpuState::Halted;
    }

    /// Power-on / RESET line: supervisor mode, interrupts masked, SSP and PC
    /// loaded from the first two vectors (M68000 UM §6.3.1).
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.state = CpuState::Running;
        self.regs.set_sr(RESET_SR);
        self.regs.a[7] = bus.read32(RESET_SSP_VECTOR);
        let pc = bus.read32(RESET_PC_VECTOR);
        self.regs.pc = pc;
        self.prefetch = [bus.read16(pc), bus.read16(pc.wrapping_add(2))];
    }

    /// Drives the interrupt priority level (0 = none, 1..=7).
    pub fn set_irq_level(&mut self, level: u8) {
        let level = level & 7;
        if level == NMI_LEVEL && self.irq_level != NMI_LEVEL {
            self.nmi_pending = true;
        }
        self.irq_level = level;
    }

    /// Interrupt to take before the next instruction, if any.
    fn pending_interrupt(&self) -> Option<u8> {
        if self.nmi_pending {
            return Some(NMI_LEVEL);
        }
        let mask = ((self.regs.sr() & sr::INT_MASK) >> sr::INT_SHIFT) as u8;
        (self.irq_level > mask && self.irq_level != NMI_LEVEL).then_some(self.irq_level)
    }

    /// Executes one instruction (or takes a pending interrupt) and returns the
    /// number of cycles it took. A stopped or halted CPU idles for 4 cycles.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.cycles = 0;
        self.exception_taken = false;
        if self.state == CpuState::Halted {
            return BUS_CYCLE;
        }
        if let Some(level) = self.pending_interrupt() {
            if level == NMI_LEVEL {
                self.nmi_pending = false;
            }
            self.ir = self.prefetch[0];
            if let Err(fault) = self.interrupt_exception(bus, level) {
                self.group0_exception(bus, fault);
            }
            return self.cycles;
        }
        if self.state == CpuState::Stopped {
            return BUS_CYCLE;
        }
        self.ir = self.prefetch[0];
        let tracing = self.regs.sr() & sr::T != 0;
        let mut result = self.execute(bus, decode(self.ir));
        if result.is_ok() && tracing && !self.exception_taken && self.state == CpuState::Running {
            result = self.trace_exception(bus);
        }
        if let Err(fault) = result {
            self.group0_exception(bus, fault);
        }
        self.cycles
    }

    // --- timing -----------------------------------------------------------

    pub(crate) fn idle(&mut self, cycles: u32) {
        self.cycles += cycles;
    }

    // --- bus access -------------------------------------------------------

    fn data_fc(&self) -> u8 {
        if self.regs.supervisor() {
            fc::SUPERVISOR_DATA
        } else {
            fc::USER_DATA
        }
    }

    fn program_fc(&self) -> u8 {
        if self.regs.supervisor() {
            fc::SUPERVISOR_PROGRAM
        } else {
            fc::USER_PROGRAM
        }
    }

    /// Word/long accesses at odd addresses abort with an address error before
    /// any bus cycle starts (M68000 UM §6.3.4).
    fn check_aligned(&self, addr: u32, write: bool, fc: u8) -> Res {
        if addr & 1 != 0 {
            Err(Fault::Address {
                addr,
                write,
                fc,
                not_instruction: false,
            })
        } else {
            Ok(())
        }
    }

    pub(crate) fn read<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Res<u32> {
        match size {
            Size::Byte => {
                self.cycles += BUS_CYCLE;
                Ok(bus.read8(addr) as u32)
            }
            Size::Word => self.read_word(bus, addr).map(u32::from),
            Size::Long => {
                let hi = self.read_word(bus, addr)? as u32;
                let lo = self.read_word(bus, addr.wrapping_add(2))? as u32;
                Ok((hi << 16) | lo)
            }
        }
    }

    pub(crate) fn read_word<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Res<u16> {
        self.check_aligned(addr, false, self.data_fc())?;
        self.cycles += BUS_CYCLE;
        Ok(bus.read16(addr))
    }

    /// Long writes go high word first, the usual order.
    pub(crate) fn write<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size, value: u32) -> Res {
        match size {
            Size::Byte => {
                self.cycles += BUS_CYCLE;
                bus.write8(addr, value as u8);
                Ok(())
            }
            Size::Word => self.write_word(bus, addr, value as u16),
            Size::Long => {
                self.write_word(bus, addr, (value >> 16) as u16)?;
                self.write_word(bus, addr.wrapping_add(2), value as u16)
            }
        }
    }

    /// Long write low word first (at `addr + 2`), then high word: used by
    /// predecrement destinations and read-modify-write instructions.
    pub(crate) fn write_low_first<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
    ) -> Res {
        if size != Size::Long {
            return self.write(bus, addr, size, value);
        }
        // The low word is the first bus cycle, so it is the faulting address.
        self.write_word(bus, addr.wrapping_add(2), value as u16)?;
        self.write_word(bus, addr, (value >> 16) as u16)
    }

    pub(crate) fn write_word<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u16) -> Res {
        self.check_aligned(addr, true, self.data_fc())?;
        self.cycles += BUS_CYCLE;
        bus.write16(addr, value);
        Ok(())
    }

    /// Shifts the prefetch queue and refills IRC from the next program word.
    /// Returns the word that moved out of IRC (an extension word, or the next
    /// opcode at the end of an instruction).
    pub(crate) fn fetch<B: Bus>(&mut self, bus: &mut B) -> Res<u16> {
        let addr = self.regs.pc.wrapping_add(4);
        self.check_aligned(addr, false, self.program_fc())?;
        let consumed = self.prefetch[1];
        self.prefetch[0] = consumed;
        self.cycles += BUS_CYCLE;
        self.prefetch[1] = bus.read16(addr);
        self.regs.pc = self.regs.pc.wrapping_add(2);
        Ok(consumed)
    }

    /// Program-space word read (no queue shift).
    pub(crate) fn read_program<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Res<u16> {
        self.check_aligned(addr, false, self.program_fc())?;
        self.cycles += BUS_CYCLE;
        Ok(bus.read16(addr))
    }

    /// Word currently in IRC, without consuming it.
    pub(crate) fn irc(&self) -> u16 {
        self.prefetch[1]
    }

    /// Branch/jump/return: reloads both prefetch words from `target`,
    /// back to back.
    pub(crate) fn branch_to<B: Bus>(&mut self, bus: &mut B, target: u32) -> Res {
        self.check_branch_target(target)?;
        self.regs.pc = target;
        self.prefetch[0] = self.read_program(bus, target)?;
        self.prefetch[1] = self.read_program(bus, target.wrapping_add(2))?;
        Ok(())
    }

    /// An odd branch target faults on the first prefetch: the frame holds the
    /// target as access address, PC = target - 4 and I/N set
    /// (SingleStepTests/680x0 Bcc/JMP/RTS frames).
    pub(crate) fn check_branch_target(&mut self, target: u32) -> Res {
        if target & 1 == 0 {
            return Ok(());
        }
        self.regs.pc = target.wrapping_sub(4);
        Err(Fault::Address {
            addr: target,
            write: false,
            fc: self.program_fc(),
            not_instruction: true,
        })
    }

    /// Exception entry: reloads both prefetch words from `target` with two
    /// idle cycles between the reads.
    pub(crate) fn jump<B: Bus>(&mut self, bus: &mut B, target: u32) -> Res {
        let fc = self.program_fc();
        self.check_aligned(target, false, fc)?;
        self.regs.pc = target;
        self.cycles += BUS_CYCLE;
        self.prefetch[0] = bus.read16(target);
        self.idle(2);
        self.cycles += BUS_CYCLE;
        self.prefetch[1] = bus.read16(target.wrapping_add(2));
        Ok(())
    }
}

impl savestate::State for Cpu {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"M68K", |v| {
            v.state(&mut self.regs)?;
            v.field(&mut self.prefetch)?;
            v.field(&mut self.ir)?;
            savestate::enum_field(
                v,
                &mut self.state,
                "68000 state",
                |s| s as u8,
                |c| match c {
                    0 => Some(CpuState::Running),
                    1 => Some(CpuState::Stopped),
                    2 => Some(CpuState::Halted),
                    _ => None,
                },
            )?;
            v.field(&mut self.cycles)?;
            v.field(&mut self.irq_level)?;
            v.field(&mut self.nmi_pending)?;
            v.field(&mut self.exception_taken)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::FlatBus;

    #[test]
    fn nop_advances_pc_and_prefetch() {
        let mut bus = FlatBus::new();
        bus.write16(0x1004, 0xABCD);
        let mut cpu = Cpu::new();
        cpu.regs.pc = 0x1000;
        cpu.prefetch = [0x4E71, 0x4E71];
        assert_eq!(cpu.step(&mut bus), 4);
        assert_eq!(cpu.regs.pc, 0x1002);
        assert_eq!(cpu.prefetch, [0x4E71, 0xABCD]);
    }
}

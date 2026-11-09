//! Exception processing (M68000 UM §6.3).
//!
//! - Group 0 (address error): 7-word frame, write order and timing observed in
//!   SingleStepTests/680x0.
//! - Groups 1 and 2 (trace, interrupts, illegal, privilege, TRAP, CHK, zero
//!   divide...): 3-word frame written PC low, SR, PC high (SingleStepTests
//!   TRAP/TRAPV/CHK transactions).

use crate::bus::Bus;
use crate::cpu::{Cpu, CpuState, Fault, Res};
use crate::instruction::Size;
use crate::registers::sr;

/// Exception vector numbers (M68000 UM Table 6-2).
pub(crate) mod vector {
    pub const ADDRESS_ERROR: u32 = 3;
    pub const ILLEGAL_INSTRUCTION: u32 = 4;
    pub const ZERO_DIVIDE: u32 = 5;
    pub const CHK: u32 = 6;
    pub const TRAPV: u32 = 7;
    pub const PRIVILEGE_VIOLATION: u32 = 8;
    pub const TRACE: u32 = 9;
    pub const LINE_A: u32 = 10;
    pub const LINE_F: u32 = 11;
    /// Autovectors: 24 + interrupt level (25..=31).
    pub const SPURIOUS_INTERRUPT: u32 = 24;
    /// TRAP #n uses 32 + n.
    pub const TRAP_BASE: u32 = 32;
}

/// Special status word bits of the group 0 frame (M68000 UM §6.3.9.1).
mod ssw {
    pub const READ: u16 = 1 << 4;
    pub const NOT_INSTRUCTION: u16 = 1 << 3;
    /// The undocumented upper bits mirror the opcode in IR (observed in
    /// SingleStepTests/680x0 frames: SSW = IR & 0xFFE0 | R/W | I/N | FC).
    pub const IR_BITS: u16 = 0xFFE0;
}

/// Idle time between the faulting cycle and the first frame write.
const GROUP0_LEAD_IN: u32 = 4;
/// Illegal, line A/F, privilege violation and trace: 34 cycles in total
/// (M68000 UM Table 8-14), i.e. 4 internal cycles before the frame.
const GROUP1_LEAD_IN: u32 = 4;
/// Interrupt: 44 cycles in total (M68000 UM Table 8-14). The split around
/// the acknowledge cycle follows Yacht; not covered by SingleStepTests.
const INTERRUPT_BEFORE_ACK: u32 = 6;
const INTERRUPT_ACK: u32 = 4;
const INTERRUPT_AFTER_ACK: u32 = 4;

impl Cpu {
    /// Address error (group 0). Frame layout from low to high address: SSW,
    /// access address (long), IR, SR, PC (long) — 7 words.
    pub(crate) fn group0_exception<B: Bus>(&mut self, bus: &mut B, fault: Fault) {
        let Fault::Address {
            addr,
            write,
            fc,
            not_instruction,
        } = fault;
        self.idle(GROUP0_LEAD_IN);

        let old_sr = self.regs.sr();
        self.regs.set_sr((old_sr | sr::S) & !sr::T);

        let status = (self.ir & ssw::IR_BITS)
            | if write { 0 } else { ssw::READ }
            | if not_instruction {
                ssw::NOT_INSTRUCTION
            } else {
                0
            }
            | fc as u16;
        let pc = self.regs.pc;
        let sp = self.regs.a[7].wrapping_sub(14);
        self.regs.a[7] = sp;

        let frame = [
            (12, pc as u16),
            (8, old_sr),
            (10, (pc >> 16) as u16),
            (6, self.ir),
            (4, addr as u16),
            (0, status),
            (2, (addr >> 16) as u16),
        ];
        for (offset, value) in frame {
            if self
                .write_word(bus, sp.wrapping_add(offset), value)
                .is_err()
            {
                // Fault while stacking a group 0 frame: double bus fault.
                self.halt();
                return;
            }
        }
        // A fault while entering an address error handler would loop forever
        // on the real chip; halting is the observable outcome we keep.
        if self.take_vector(bus, vector::ADDRESS_ERROR).is_err() {
            self.halt();
        }
    }

    /// Group 1/2 exception: 3-word frame (SR, PC). `return_pc` is the PC to
    /// stack; any instruction-specific idle time is spent by the caller.
    pub(crate) fn group2_exception<B: Bus>(
        &mut self,
        bus: &mut B,
        vector: u32,
        return_pc: u32,
    ) -> Res {
        self.exception_taken = true;
        let old_sr = self.regs.sr();
        self.regs.set_sr((old_sr | sr::S) & !sr::T);
        self.push_short_frame(bus, old_sr, return_pc)?;
        self.take_vector(bus, vector)
    }

    /// Illegal instruction, line A/F, privilege violation: the stacked PC is
    /// the address of the offending instruction.
    pub(crate) fn instruction_exception<B: Bus>(&mut self, bus: &mut B, vector: u32) -> Res {
        self.idle(GROUP1_LEAD_IN);
        let pc = self.regs.pc;
        self.group2_exception(bus, vector, pc)
    }

    pub(crate) fn privilege_violation<B: Bus>(&mut self, bus: &mut B) -> Res {
        self.instruction_exception(bus, vector::PRIVILEGE_VIOLATION)
    }

    /// Trace: taken after an instruction executed with T set; stacks the
    /// address of the next instruction.
    pub(crate) fn trace_exception<B: Bus>(&mut self, bus: &mut B) -> Res {
        self.idle(GROUP1_LEAD_IN);
        let pc = self.regs.pc;
        self.group2_exception(bus, vector::TRACE, pc)
    }

    /// Interrupt at `level`: the mask is raised to `level`. The vector comes
    /// from the acknowledge cycle (autovector when the bus returns `None`).
    pub(crate) fn interrupt_exception<B: Bus>(&mut self, bus: &mut B, level: u8) -> Res {
        self.state = CpuState::Running;
        self.idle(INTERRUPT_BEFORE_ACK);
        let old_sr = self.regs.sr();
        let new_sr = (old_sr | sr::S) & !(sr::T | sr::INT_MASK) | ((level as u16) << sr::INT_SHIFT);
        self.regs.set_sr(new_sr);
        let pc = self.regs.pc;
        let sp = self.regs.a[7].wrapping_sub(6);
        self.regs.a[7] = sp;
        // Frame: PC low first, then the acknowledge cycle, then SR and PC high.
        self.write_word(bus, sp.wrapping_add(4), pc as u16)?;
        self.idle(INTERRUPT_ACK);
        let vector = match bus.interrupt_ack(level) {
            Some(v) => v as u32,
            None => vector::SPURIOUS_INTERRUPT + level as u32,
        };
        self.idle(INTERRUPT_AFTER_ACK);
        self.write_word(bus, sp, old_sr)?;
        self.write_word(bus, sp.wrapping_add(2), (pc >> 16) as u16)?;
        self.take_vector(bus, vector)
    }

    /// Pushes SR and PC in the 68000 order: PC low, SR, PC high.
    fn push_short_frame<B: Bus>(&mut self, bus: &mut B, status: u16, pc: u32) -> Res {
        let sp = self.regs.a[7].wrapping_sub(6);
        self.regs.a[7] = sp;
        self.write_word(bus, sp.wrapping_add(4), pc as u16)?;
        self.write_word(bus, sp, status)?;
        self.write_word(bus, sp.wrapping_add(2), (pc >> 16) as u16)
    }

    /// Loads the handler address from the vector table and jumps to it.
    fn take_vector<B: Bus>(&mut self, bus: &mut B, vector: u32) -> Res {
        let target = self.read(bus, vector * 4, Size::Long)?;
        self.jump(bus, target)
    }
}

#[cfg(test)]
mod tests {
    use super::vector;
    use crate::bus::{Bus, FlatBus};
    use crate::cpu::{Cpu, CpuState};
    use crate::registers::sr;

    const CODE: u32 = 0x1000;
    const STACK: u32 = 0x8000;
    const HANDLER: u32 = 0x4000;
    const NOP: u16 = 0x4E71;

    /// CPU in supervisor mode at CODE with every vector pointing to HANDLER.
    fn setup(program: &[u16]) -> (Cpu, FlatBus) {
        let mut bus = FlatBus::new();
        for v in 2..64 {
            bus.write32(v * 4, HANDLER);
        }
        bus.write32(0, STACK);
        bus.write32(4, CODE);
        for (i, w) in program.iter().enumerate() {
            bus.write16(CODE + 2 * i as u32, *w);
        }
        for i in 0..8 {
            bus.write16(CODE + 2 * (program.len() + i) as u32, NOP);
            bus.write16(HANDLER + 2 * i as u32, NOP);
        }
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        (cpu, bus)
    }

    /// Checks the 3-word frame at the top of the supervisor stack.
    fn assert_frame(cpu: &Cpu, bus: &mut FlatBus, sr_value: u16, pc: u32) {
        let sp = cpu.regs.a[7];
        assert_eq!(sp, STACK - 6);
        assert_eq!(bus.read16(sp), sr_value);
        assert_eq!(bus.read32(sp + 2), pc);
        assert_eq!(cpu.regs.pc, HANDLER);
    }

    #[test]
    fn reset_loads_vectors() {
        let (cpu, _) = setup(&[NOP]);
        assert_eq!(cpu.regs.a[7], STACK);
        assert_eq!(cpu.regs.pc, CODE);
        assert_eq!(cpu.regs.sr(), 0x2700);
        assert_eq!(cpu.prefetch[0], NOP);
    }

    /// ILLEGAL, line A, line F: 34 cycles, stacked PC = the instruction itself.
    #[test]
    fn illegal_and_line_emulators() {
        for (opcode, vec) in [
            (0x4AFC, vector::ILLEGAL_INSTRUCTION),
            (0xA000, vector::LINE_A),
            (0xF123, vector::LINE_F),
        ] {
            let (mut cpu, mut bus) = setup(&[opcode]);
            bus.write32(vec * 4, HANDLER);
            assert_eq!(cpu.step(&mut bus), 34, "{opcode:04X}");
            assert_frame(&cpu, &mut bus, 0x2700, CODE);
        }
    }

    #[test]
    fn privileged_instruction_in_user_mode() {
        // move #$2700,sr from user mode
        let (mut cpu, mut bus) = setup(&[0x46FC, 0x2700]);
        cpu.regs.set_usp(0x6000);
        cpu.regs.set_sr(0x0000);
        assert_eq!(cpu.step(&mut bus), 34);
        assert!(cpu.regs.supervisor());
        assert_frame(&cpu, &mut bus, 0x0000, CODE);
        assert_eq!(cpu.regs.usp(), 0x6000);
    }

    /// Autovectored interrupt: 44 cycles, mask raised to the level, PC of
    /// the interrupted instruction stacked.
    #[test]
    fn autovectored_interrupt() {
        let (mut cpu, mut bus) = setup(&[NOP, NOP]);
        cpu.regs.set_sr(0x2300);
        cpu.set_irq_level(3);
        assert_eq!(cpu.step(&mut bus), 4, "level 3 is masked by 3");
        cpu.set_irq_level(6);
        assert_eq!(cpu.step(&mut bus), 44);
        assert_frame(&cpu, &mut bus, 0x2300, CODE + 2);
        assert_eq!(cpu.regs.sr() & sr::INT_MASK, 0x0600);
        // Masked now: the handler runs.
        assert_eq!(cpu.step(&mut bus), 4);
        assert_eq!(cpu.regs.pc, HANDLER + 2);
    }

    #[test]
    fn nmi_is_edge_triggered() {
        let (mut cpu, mut bus) = setup(&[NOP]);
        cpu.set_irq_level(7);
        assert_eq!(cpu.step(&mut bus), 44);
        assert_eq!(cpu.regs.pc, HANDLER);
        // Still asserted, already taken: no second interrupt.
        assert_eq!(cpu.step(&mut bus), 4);
    }

    #[test]
    fn stop_waits_for_interrupt() {
        // stop #$2000 (mask 0)
        let (mut cpu, mut bus) = setup(&[0x4E72, 0x2000]);
        cpu.step(&mut bus);
        assert_eq!(cpu.state(), CpuState::Stopped);
        let pc = cpu.regs.pc;
        assert_eq!(cpu.step(&mut bus), 4);
        assert_eq!(cpu.regs.pc, pc);
        cpu.set_irq_level(4);
        assert_eq!(cpu.step(&mut bus), 44);
        assert_eq!(cpu.state(), CpuState::Running);
        assert_frame(&cpu, &mut bus, 0x2000, CODE + 4);
    }

    /// Trace after an instruction run with T set: stacked PC = next instruction.
    #[test]
    fn trace_after_instruction() {
        let (mut cpu, mut bus) = setup(&[NOP, NOP]);
        cpu.regs.set_sr(0xA700);
        assert_eq!(cpu.step(&mut bus), 4 + 34);
        assert_frame(&cpu, &mut bus, 0xA700, CODE + 2);
        assert_eq!(cpu.regs.sr() & sr::T, 0);
    }

    #[test]
    fn interrupt_ack_vector_from_bus() {
        struct VectoredBus(FlatBus);
        impl Bus for VectoredBus {
            fn read8(&mut self, a: u32) -> u8 {
                self.0.read8(a)
            }
            fn read16(&mut self, a: u32) -> u16 {
                self.0.read16(a)
            }
            fn write8(&mut self, a: u32, v: u8) {
                self.0.write8(a, v)
            }
            fn write16(&mut self, a: u32, v: u16) {
                self.0.write16(a, v)
            }
            fn interrupt_ack(&mut self, _level: u8) -> Option<u8> {
                Some(64)
            }
        }
        let (mut cpu, bus) = setup(&[NOP]);
        let mut bus = VectoredBus(bus);
        bus.write32(64 * 4, 0x5000);
        bus.write16(0x5000, NOP);
        cpu.set_irq_level(2);
        cpu.regs.set_sr(0x2000);
        cpu.step(&mut bus);
        assert_eq!(cpu.regs.pc, 0x5000);
    }
}

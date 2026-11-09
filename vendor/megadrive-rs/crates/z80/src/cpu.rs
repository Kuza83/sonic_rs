//! Z80 execution core: state, bus access helpers and the step loop.
//!
//! T-states are counted per bus cycle (opcode fetch 4, memory read/write 3,
//! I/O 4) plus explicit internal cycles per instruction, so the total matches
//! the `cycles` list of SingleStepTests/z80.

use crate::bus::Bus;
use crate::decode::decode_base;
use crate::registers::Registers;

/// T-states of an opcode fetch (M1 cycle with refresh).
pub(crate) const M1_TSTATES: u32 = 4;
/// T-states of a memory read or write cycle.
pub(crate) const MEM_TSTATES: u32 = 3;
/// T-states of an I/O read or write cycle.
pub(crate) const IO_TSTATES: u32 = 4;

/// Register used where an instruction names HL: plain HL, or IX/IY after a
/// DD/FD prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Index {
    Hl,
    Ix,
    Iy,
}

#[derive(Debug, Clone, Default)]
pub struct Cpu {
    pub regs: Registers,
    /// T-states of the instruction being executed.
    tstates: u32,
    /// Executing HALT: the CPU repeats NOPs until an interrupt.
    pub halted: bool,
    /// Set when the current instruction writes F (drives the Q register).
    pub(crate) flags_written: bool,
    /// Level of the /INT line.
    int_line: bool,
    /// Edge on /NMI, serviced before the next instruction.
    nmi_pending: bool,
}

/// Interrupt entry costs (Zilog UM0080, interrupt response timing).
/// NMI: 5-T-state M1 then two stack writes.
const NMI_ACK_TSTATES: u32 = 5;
/// Maskable interrupt acknowledge: M1 of 4 + 2 wait states, plus 1.
const INT_ACK_TSTATES: u32 = 7;
const NMI_VECTOR: u16 = 0x0066;
const IM1_VECTOR: u16 = 0x0038;
/// RST opcodes have the form 11yyy111.
const RST_MASK: u8 = 0xC7;
const RST_BASE: u8 = 0xC7;

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drives the level-sensitive /INT line.
    pub fn set_int(&mut self, asserted: bool) {
        self.int_line = asserted;
    }

    /// Falling edge on /NMI.
    pub fn trigger_nmi(&mut self) {
        self.nmi_pending = true;
    }

    /// Power-on/reset state (Zilog UM0080: PC = 0, I = R = 0, IM 0,
    /// interrupts disabled).
    pub fn reset(&mut self) {
        self.regs.pc = 0;
        self.regs.i = 0;
        self.regs.r = 0;
        self.regs.im = 0;
        self.regs.iff1 = false;
        self.regs.iff2 = false;
        self.regs.ei = false;
        self.halted = false;
    }

    /// Executes one instruction (or accepts a pending interrupt) and returns
    /// its length in T-states.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.tstates = 0;
        if self.nmi_pending {
            self.nmi_pending = false;
            self.accept_nmi(bus);
            return self.tstates;
        }
        // EI delays acceptance by one instruction.
        if self.int_line && self.regs.iff1 && !self.regs.ei {
            self.accept_int(bus);
            return self.tstates;
        }
        if self.halted {
            // HALT executes NOPs (M1 cycles with refresh) in place.
            self.regs.bump_r();
            self.tstates += M1_TSTATES;
            self.regs.ei = false;
            return self.tstates;
        }
        self.flags_written = false;
        self.regs.ei = false;
        self.regs.p = false;
        let opcode = self.fetch_opcode(bus);
        self.execute(bus, decode_base(opcode), Index::Hl);
        self.regs.q = if self.flags_written { self.regs.f } else { 0 };
        self.tstates
    }

    /// Common start of an interrupt: leave HALT, count the acknowledge M1.
    /// An interrupt right after LD A,I / LD A,R clears P/V (the P quirk).
    fn begin_interrupt(&mut self) {
        self.halted = false;
        self.regs.bump_r();
        if self.regs.p {
            self.regs.f &= !crate::registers::flag::PV;
        }
        self.regs.p = false;
        self.regs.ei = false;
        self.regs.q = 0;
    }

    fn accept_nmi<B: Bus>(&mut self, bus: &mut B) {
        self.begin_interrupt();
        self.regs.iff1 = false;
        self.tstates += NMI_ACK_TSTATES;
        self.push(bus, self.regs.pc);
        self.regs.pc = NMI_VECTOR;
        self.regs.wz = NMI_VECTOR;
    }

    fn accept_int<B: Bus>(&mut self, bus: &mut B) {
        self.begin_interrupt();
        self.regs.iff1 = false;
        self.regs.iff2 = false;
        self.tstates += INT_ACK_TSTATES;
        let data = bus.interrupt_data();
        let target = match self.regs.im {
            2 => {
                let vector = ((self.regs.i as u16) << 8) | data as u16;
                self.push(bus, self.regs.pc);
                self.read_word(bus, vector)
            }
            1 => {
                self.push(bus, self.regs.pc);
                IM1_VECTOR
            }
            _ => {
                // IM 0 executes the byte on the bus; only RST is supported
                // (what interrupting devices put there in practice).
                self.push(bus, self.regs.pc);
                if data & RST_MASK == RST_BASE {
                    (data & !RST_MASK) as u16
                } else {
                    IM1_VECTOR
                }
            }
        };
        self.regs.pc = target;
        self.regs.wz = target;
    }

    // --- timing and flags --------------------------------------------------

    pub(crate) fn idle(&mut self, tstates: u32) {
        self.tstates += tstates;
    }

    pub(crate) fn set_f(&mut self, value: u8) {
        self.regs.f = value;
        self.flags_written = true;
    }

    // --- bus access --------------------------------------------------------

    /// M1 cycle: reads the opcode at PC, increments PC and R.
    pub(crate) fn fetch_opcode<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let op = bus.read(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        self.regs.bump_r();
        self.tstates += M1_TSTATES;
        op
    }

    /// Operand byte at PC.
    pub(crate) fn fetch_byte<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = self.read(bus, self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        v
    }

    /// Little-endian operand word at PC.
    pub(crate) fn fetch_word<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch_byte(bus);
        let hi = self.fetch_byte(bus);
        u16::from_le_bytes([lo, hi])
    }

    pub(crate) fn read<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.tstates += MEM_TSTATES;
        bus.read(addr)
    }

    pub(crate) fn write<B: Bus>(&mut self, bus: &mut B, addr: u16, value: u8) {
        self.tstates += MEM_TSTATES;
        bus.write(addr, value);
    }

    pub(crate) fn read_word<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u16 {
        let lo = self.read(bus, addr);
        let hi = self.read(bus, addr.wrapping_add(1));
        u16::from_le_bytes([lo, hi])
    }

    pub(crate) fn write_word<B: Bus>(&mut self, bus: &mut B, addr: u16, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.write(bus, addr, lo);
        self.write(bus, addr.wrapping_add(1), hi);
    }

    pub(crate) fn port_in<B: Bus>(&mut self, bus: &mut B, port: u16) -> u8 {
        self.tstates += IO_TSTATES;
        bus.port_in(port)
    }

    pub(crate) fn port_out<B: Bus>(&mut self, bus: &mut B, port: u16, value: u8) {
        self.tstates += IO_TSTATES;
        bus.port_out(port, value);
    }

    /// PUSH: high byte first, at SP-1 then SP-2.
    pub(crate) fn push<B: Bus>(&mut self, bus: &mut B, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.write(bus, self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.write(bus, self.regs.sp, lo);
    }

    pub(crate) fn pop<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let value = self.read_word(bus, self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(2);
        value
    }
}

impl savestate::State for Cpu {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"Z80 ", |v| {
            let r = &mut self.regs;
            for byte in [
                &mut r.a, &mut r.f, &mut r.b, &mut r.c, &mut r.d, &mut r.e, &mut r.h, &mut r.l,
            ] {
                v.field(byte)?;
            }
            for word in [
                &mut r.af_alt,
                &mut r.bc_alt,
                &mut r.de_alt,
                &mut r.hl_alt,
                &mut r.ix,
                &mut r.iy,
                &mut r.sp,
                &mut r.pc,
                &mut r.wz,
            ] {
                v.field(word)?;
            }
            for byte in [&mut r.i, &mut r.r, &mut r.im, &mut r.q] {
                v.field(byte)?;
            }
            for flag in [&mut r.iff1, &mut r.iff2, &mut r.p, &mut r.ei] {
                v.field(flag)?;
            }
            v.field(&mut self.tstates)?;
            v.field(&mut self.halted)?;
            v.field(&mut self.flags_written)?;
            v.field(&mut self.int_line)?;
            v.field(&mut self.nmi_pending)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::FlatBus;

    fn cpu_at(program: &[u8]) -> (Cpu, FlatBus) {
        let mut bus = FlatBus::new();
        bus.mem[..program.len()].copy_from_slice(program);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.regs.sp = 0x8000;
        (cpu, bus)
    }

    #[test]
    fn im1_interrupt() {
        // ei ; nop
        let (mut cpu, mut bus) = cpu_at(&[0xFB, 0x00, 0x00]);
        cpu.regs.im = 1;
        cpu.set_int(true);
        cpu.step(&mut bus); // EI
        cpu.step(&mut bus); // NOP: still delayed by EI
        assert_eq!(cpu.regs.pc, 2);
        assert_eq!(cpu.step(&mut bus), 13);
        assert_eq!(cpu.regs.pc, 0x38);
        assert!(!cpu.regs.iff1 && !cpu.regs.iff2);
        assert_eq!(cpu.regs.sp, 0x7FFE);
        assert_eq!(bus.mem[0x7FFE], 0x02);
        assert_eq!(bus.mem[0x7FFF], 0x00);
    }

    #[test]
    fn im2_interrupt_reads_vector_table() {
        let (mut cpu, mut bus) = cpu_at(&[0x00]);
        cpu.regs.im = 2;
        cpu.regs.i = 0x40;
        cpu.regs.iff1 = true;
        bus.mem[0x40FF] = 0x34; // vector at I:$FF (undriven bus reads $FF)
        bus.mem[0x4100] = 0x12;
        cpu.set_int(true);
        assert_eq!(cpu.step(&mut bus), 19);
        assert_eq!(cpu.regs.pc, 0x1234);
    }

    #[test]
    fn disabled_interrupts_are_ignored() {
        let (mut cpu, mut bus) = cpu_at(&[0x00]);
        cpu.regs.im = 1;
        cpu.set_int(true);
        cpu.step(&mut bus);
        assert_eq!(cpu.regs.pc, 1);
    }

    #[test]
    fn halt_waits_and_interrupt_resumes_after_it() {
        // halt ; nop
        let (mut cpu, mut bus) = cpu_at(&[0x76, 0x00]);
        cpu.regs.im = 1;
        cpu.regs.iff1 = true;
        cpu.step(&mut bus);
        assert!(cpu.halted);
        let r = cpu.regs.r;
        assert_eq!(cpu.step(&mut bus), 4);
        assert_eq!(cpu.regs.pc, 1, "PC stays after HALT");
        assert_eq!(cpu.regs.r, r + 1, "refresh keeps running");
        cpu.set_int(true);
        cpu.step(&mut bus);
        assert!(!cpu.halted);
        assert_eq!(cpu.regs.pc, 0x38);
        assert_eq!(bus.mem[0x7FFE], 0x01, "returns after the HALT");
    }

    #[test]
    fn nmi_keeps_iff2() {
        let (mut cpu, mut bus) = cpu_at(&[0x00]);
        cpu.regs.iff1 = true;
        cpu.regs.iff2 = true;
        cpu.trigger_nmi();
        assert_eq!(cpu.step(&mut bus), 11);
        assert_eq!(cpu.regs.pc, 0x66);
        assert!(!cpu.regs.iff1);
        assert!(cpu.regs.iff2, "RETN restores IFF1 from IFF2");
    }

    #[test]
    fn interrupt_after_ld_a_i_clears_pv() {
        // ld a,i with IFF2 set gives P/V = 1, cleared if an interrupt follows.
        let (mut cpu, mut bus) = cpu_at(&[0xED, 0x57]);
        cpu.regs.im = 1;
        cpu.regs.iff1 = true;
        cpu.regs.iff2 = true;
        cpu.step(&mut bus);
        assert_ne!(cpu.regs.f & crate::registers::flag::PV, 0);
        cpu.set_int(true);
        cpu.step(&mut bus);
        assert_eq!(cpu.regs.f & crate::registers::flag::PV, 0);
    }
}

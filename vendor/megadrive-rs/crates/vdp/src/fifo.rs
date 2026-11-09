//! The 4-entry write FIFO of the data port (GPGX core/vdp_ctrl.c,
//! `vdp_68k_data_w_m5`, `fifo_timing_h32/h40`, `vdp_68k_ctrl_r`).
//!
//! During active display the VDP drains the FIFO only at the access slots of
//! the line; a 68000 data write finding the FIFO full waits until its oldest
//! entry is read out. Memory itself is written immediately (as GPGX
//! `vdp_bus_w`): only the 68000's time and the FIFO status bits are
//! affected. In VBLANK or with the display disabled there is no wait.
//!
//! Approximation: the access time is the start of the 68000 instruction (the
//! scheduler's `line_cycle`), plus the waits of earlier writes in the same
//! instruction.

use crate::Vdp;

const MASTER_CYCLES_PER_LINE: i32 = 3420;
/// 68000 clock divider: a wait ends on a 68000 clock edge (GPGX rounds the
/// 68000 cycle count up to a multiple of 7).
const M68K_DIVIDER: u32 = 7;
/// Master cycles of the access slots inside a line (GPGX fifo_timing_*; the
/// tables continue on the next line, + 3420).
const SLOTS_H32: [i32; 16] = [
    230, 510, 810, 970, 1130, 1450, 1610, 1770, 2090, 2250, 2410, 2730, 2890, 3050, 3350, 3370,
];
const SLOTS_H40: [i32; 18] = [
    352, 820, 948, 1076, 1332, 1460, 1588, 1844, 1972, 2100, 2356, 2484, 2612, 2868, 2996, 3124,
    3364, 3380,
];
/// Read-out times older than this are all equivalent (FIFO drained); the
/// floor keeps them from overflowing over many lines.
const OLDEST: i32 = -4 * MASTER_CYCLES_PER_LINE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Fifo {
    /// Read-out cycle of each entry, relative to the start of the current
    /// line (GPGX fifo_cycles).
    read_out: [i32; 4],
    /// Next entry written (GPGX fifo_idx).
    index: usize,
}

impl savestate::Field for Fifo {
    fn write(&self, w: &mut savestate::Writer) {
        for cycle in self.read_out {
            (cycle as u32).write(w);
        }
        (self.index as u8).write(w);
    }
    fn read(&mut self, r: &mut savestate::Reader) -> savestate::Result {
        for cycle in &mut self.read_out {
            let mut raw = 0u32;
            raw.read(r)?;
            *cycle = raw as i32;
        }
        let mut index = 0u8;
        index.read(r)?;
        if index > 3 {
            return Err(savestate::StateError::InvalidValue("FIFO index"));
        }
        self.index = index as usize;
        Ok(())
    }
}

impl Fifo {
    fn last(&self) -> i32 {
        self.read_out[(self.index + 3) & 3]
    }

    fn oldest(&self) -> i32 {
        self.read_out[self.index]
    }

    /// Start of the next line: times become relative to it.
    pub(crate) fn next_line(&mut self) {
        for cycle in &mut self.read_out {
            *cycle = (*cycle - MASTER_CYCLES_PER_LINE).max(OLDEST);
        }
    }

    /// Status bits at line cycle `now`: (empty, full).
    pub(crate) fn status(&self, now: i32) -> (bool, bool) {
        if now >= self.last() {
            (true, false)
        } else {
            (false, now < self.oldest())
        }
    }

    /// A data write at line cycle `now`: returns the master cycles the 68000
    /// waits. `extra_slot`: VRAM writes are read out one slot later.
    fn write(&mut self, now: i32, h40: bool, extra_slot: bool) -> u32 {
        let mut wait = 0;
        let mut cycles = now;
        if now < self.last() {
            if now < self.oldest() {
                wait = ((self.oldest() - now) as u32).div_ceil(M68K_DIVIDER) * M68K_DIVIDER;
            }
            cycles = self.last();
        }
        let slots: &[i32] = if h40 { &SLOTS_H40 } else { &SLOTS_H32 };
        let slot_at =
            |k: usize| slots[k % slots.len()] + MASTER_CYCLES_PER_LINE * (k / slots.len()) as i32;
        let mut slot = 0;
        while cycles >= slot_at(slot) {
            slot += 1;
        }
        self.read_out[self.index] = slot_at(slot + extra_slot as usize);
        self.index = (self.index + 1) & 3;
        wait
    }
}

impl Vdp {
    /// FIFO bookkeeping of a 68000 data port write (before memory is
    /// written): restricted to active display (GPGX: `!(status & 8) &&
    /// (reg[1] & 0x40)`).
    pub(crate) fn fifo_write(&mut self) {
        if self.in_vblank() || !self.regs.display_enabled() {
            return;
        }
        let now = (self.line_cycle + self.fifo_stall) as i32;
        // GPGX fifo_byte_access = (code & 0x06) ? 0 : 1: VRAM writes.
        let extra_slot = self.code & 0x06 == 0;
        self.fifo_stall += self.fifo.write(now, self.regs.h40(), extra_slot);
    }

    /// Master cycles the 68000 waited on a full FIFO since the last call.
    pub fn take_fifo_stall(&mut self) -> u32 {
        std::mem::take(&mut self.fifo_stall)
    }

    /// FIFO status bits (empty, full) now.
    pub(crate) fn fifo_status(&self) -> (bool, bool) {
        self.fifo.status((self.line_cycle + self.fifo_stall) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status;

    fn command(v: &mut Vdp, long: u32) {
        v.write_control((long >> 16) as u16);
        v.write_control(long as u16);
    }

    /// Mode 5, display on, auto-increment 2, H40 or H32, on active line 10.
    fn active(h40: bool) -> Vdp {
        let mut v = Vdp::new(false);
        for w in [0x8004, 0x8144, 0x8F02, if h40 { 0x8C81 } else { 0x8C00 }] {
            v.write_control(w);
        }
        v.start_line(10);
        v
    }

    fn fifo_bits(v: &mut Vdp) -> (bool, bool) {
        let s = v.read_status();
        (s & status::FIFO_EMPTY != 0, s & status::FIFO_FULL != 0)
    }

    #[test]
    fn four_writes_fit_the_fifth_waits() {
        let mut v = active(true);
        command(&mut v, 0xC000_0000); // CRAM: one slot per entry
        v.set_line_cycle(0);
        for _ in 0..4 {
            v.write_data(0x0EEE);
        }
        assert_eq!(v.take_fifo_stall(), 0, "4 entries fit");
        assert_eq!(fifo_bits(&mut v), (false, true), "full");
        // Entries are read out at the first 4 H40 slots: 352, 820, 948, 1076.
        v.write_data(0x0EEE);
        assert_eq!(
            v.take_fifo_stall(),
            352_u32.div_ceil(7) * 7,
            "waits for the oldest"
        );
        v.set_line_cycle(2000);
        assert_eq!(fifo_bits(&mut v), (true, false), "drained by then");
    }

    #[test]
    fn vram_entries_take_one_more_slot() {
        let mut v = active(true);
        command(&mut v, 0x4000_0000); // VRAM
        v.set_line_cycle(0);
        for _ in 0..5 {
            v.write_data(0x1234);
        }
        // The first entry is read out at the second slot (820), not 352.
        assert_eq!(v.take_fifo_stall(), 820_u32.div_ceil(7) * 7);
    }

    #[test]
    fn no_wait_in_blanking_or_with_the_display_off() {
        let mut v = active(true);
        command(&mut v, 0xC000_0000);
        v.write_control(0x8104); // display off
        v.set_line_cycle(0);
        for _ in 0..20 {
            v.write_data(0);
        }
        assert_eq!(v.take_fifo_stall(), 0);
        let mut v = active(true);
        command(&mut v, 0xC000_0000);
        v.start_line(230); // VBLANK
        for _ in 0..20 {
            v.write_data(0);
        }
        assert_eq!(v.take_fifo_stall(), 0);
        assert_eq!(fifo_bits(&mut v), (true, false));
    }

    #[test]
    fn read_out_carries_into_the_next_line() {
        let mut v = active(true);
        command(&mut v, 0xC000_0000);
        v.set_line_cycle(3300);
        for _ in 0..4 {
            v.write_data(0);
        }
        // Slots 3364, 3380, then 352 + 3420 and 820 + 3420 on the next line.
        v.start_line(11);
        v.set_line_cycle(400);
        assert_eq!(fifo_bits(&mut v), (false, false), "two left");
        v.set_line_cycle(900);
        assert_eq!(fifo_bits(&mut v), (true, false));
    }

    #[test]
    fn h32_uses_its_own_slots() {
        let mut v = active(false);
        command(&mut v, 0xC000_0000);
        v.set_line_cycle(0);
        for _ in 0..5 {
            v.write_data(0);
        }
        assert_eq!(v.take_fifo_stall(), 230_u32.div_ceil(7) * 7);
    }

    #[test]
    fn a_long_write_sees_the_wait_of_its_first_word() {
        let mut v = active(true);
        command(&mut v, 0xC000_0000);
        v.set_line_cycle(0);
        for _ in 0..6 {
            v.write_data(0); // same instruction time: the 6th waits for entry 2
        }
        // Entry 1 out at 352 (wait to 357), then entry 2 at 820 from 357.
        assert_eq!(v.take_fifo_stall(), 357 + (820 - 357_u32).div_ceil(7) * 7);
    }
}

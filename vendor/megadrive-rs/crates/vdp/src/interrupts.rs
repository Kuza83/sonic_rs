//! Line events, interrupts and the HV counter (J5).
//!
//! Sequencing follows Genesis Plus GX core/system.c (`system_frame_gen`) and
//! core/vdp_ctrl.c (`vdp_68k_irq_ack`, register 0/1 writes):
//! - active lines: the H-int counter is checked at the start of each line;
//!   at 0 it is reloaded from register 10 and a horizontal interrupt becomes
//!   pending, otherwise it is decremented;
//! - first VBlank line (224, or 240 in V30): H-int fires if the counter is 0
//!   (no reload);
//!   the vertical interrupt becomes pending 788 (H40) / 770 (H32) master
//!   cycles into the line;
//! - last line of the frame: the counter is reloaded and VBLANK ends.

use crate::Vdp;

/// Lines per frame.
pub const LINES_NTSC: u16 = 262;
pub const LINES_PAL: u16 = 313;
/// Master cycles from the start of line 224 to the vertical interrupt
/// (GPGX VINT_H40_MCYCLE / VINT_H32_MCYCLE).
pub const VINT_CYCLE_H40: u32 = 788;
pub const VINT_CYCLE_H32: u32 = 770;
/// 68000 interrupt levels used by the VDP.
pub const VINT_LEVEL: u8 = 6;
pub const HINT_LEVEL: u8 = 4;

/// Last V counter value before the jump (GPGX vdp_ctrl.c vc_table): NTSC
/// $EA in V28, $106 in V30; PAL $102 in V28, $10A in V30. The counter then
/// jumps so that it ends at $1FF on the last line: V28 NTSC $00-$EA then
/// $1E5-$1FF, PAL $00-$102 then $1CA-$1FF (Plutiedev "Raster effects"); in
/// NTSC V30 it never jumps. Only the low 8 bits are readable.
const NTSC_V28_LAST: u16 = 0xEA;
const NTSC_V30_LAST: u16 = 0x106;
const PAL_V28_LAST: u16 = 0x102;
const PAL_V30_LAST: u16 = 0x10A;
const V_COUNTER_END: u16 = 0x200;
const MASTER_CYCLES_PER_LINE: u32 = 3420;
/// H counter: H32 runs $00-$93 then $E9-$FF, H40 $00-$B6 then $E4-$FF
/// (Plutiedev). The position is mapped linearly on the line's master cycles,
/// with H = 0 at the start of the line: an approximation (the exact per-pixel
/// tables and origin come later).
const H32_LAST_BEFORE_JUMP: u32 = 0x93;
const H32_JUMP_TO: u32 = 0xE9;
const H40_LAST_BEFORE_JUMP: u32 = 0xB6;
const H40_JUMP_TO: u32 = 0xE4;

/// Number of distinct H counter values in a line for a given range.
const fn h_steps(last_before_jump: u32, jump_to: u32) -> u32 {
    (last_before_jump + 1) + (0x100 - jump_to)
}

impl Vdp {
    pub fn lines_per_frame(&self) -> u16 {
        if self.pal { LINES_PAL } else { LINES_NTSC }
    }

    /// VBLANK spans the first line after the picture (224, or 240 in V30)
    /// up to (not including) the last line of the frame.
    pub(crate) fn in_vblank(&self) -> bool {
        self.line >= self.active_lines && self.line < self.lines_per_frame() - 1
    }

    /// Start of scanline `line`: updates the H-int counter.
    pub fn start_line(&mut self, line: u16) {
        self.end_line_dma();
        self.fifo.next_line();
        self.line = line;
        self.line_cycle = 0;
        if line == 0 {
            self.active_lines = if self.regs.v30() {
                crate::MAX_ACTIVE_LINES
            } else {
                crate::ACTIVE_LINES
            };
        }
        let active = self.active_lines;
        if line < active {
            if self.hint_counter == 0 {
                self.hint_counter = self.regs.hint_counter_reload();
                self.hint_pending = true;
            } else {
                self.hint_counter -= 1;
            }
        } else if line == active {
            if self.hint_counter == 0 {
                self.hint_pending = true;
            }
        } else if line == self.lines_per_frame() - 1 {
            self.hint_counter = self.regs.hint_counter_reload();
        }
    }

    /// Master cycle of the first VBlank line at which the vertical
    /// interrupt occurs.
    pub fn vint_cycle(&self) -> u32 {
        if self.regs.h40() {
            VINT_CYCLE_H40
        } else {
            VINT_CYCLE_H32
        }
    }

    /// Raises the vertical interrupt (status F flag).
    pub fn trigger_vint(&mut self) {
        self.vint_pending = true;
    }

    /// Interrupt level requested from the 68000: VINT (6) has priority over
    /// HINT (4); each needs its enable bit.
    pub fn irq_level(&self) -> u8 {
        if self.vint_pending && self.regs.vint_enabled() {
            VINT_LEVEL
        } else if self.hint_pending && self.regs.hint_enabled() {
            HINT_LEVEL
        } else {
            0
        }
    }

    /// 68000 interrupt acknowledge: clears VINT if it is the one being
    /// serviced (enabled and pending), HINT otherwise (GPGX vdp_68k_irq_ack).
    pub fn acknowledge_interrupt(&mut self) {
        if self.vint_pending && self.regs.vint_enabled() {
            self.vint_pending = false;
        } else {
            self.hint_pending = false;
        }
    }

    /// HV counter word: V counter in the high byte, H counter in the low byte.
    pub fn hv_counter(&self) -> u16 {
        (self.v_counter() << 8) | self.h_counter()
    }

    fn v_counter(&self) -> u16 {
        let line = self.line;
        let v30 = self.active_lines == crate::MAX_ACTIVE_LINES;
        let last = match (self.pal, v30) {
            (false, false) => NTSC_V28_LAST,
            (false, true) => NTSC_V30_LAST,
            (true, false) => PAL_V28_LAST,
            (true, true) => PAL_V30_LAST,
        };
        let v = if line > last {
            // Remaining lines count up to $1FF.
            let jump_to = V_COUNTER_END - (self.lines_per_frame() - last - 1);
            line - (last + 1) + jump_to
        } else {
            line
        };
        v & 0xFF
    }

    fn h_counter(&self) -> u16 {
        let (last, jump) = if self.regs.h40() {
            (H40_LAST_BEFORE_JUMP, H40_JUMP_TO)
        } else {
            (H32_LAST_BEFORE_JUMP, H32_JUMP_TO)
        };
        let steps = h_steps(last, jump);
        let step = self.line_cycle.min(MASTER_CYCLES_PER_LINE - 1) * steps / MASTER_CYCLES_PER_LINE;
        let h = if step > last {
            step - (last + 1) + jump
        } else {
            step
        };
        (h & 0xFF) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status;

    fn vdp() -> Vdp {
        let mut v = Vdp::new(false);
        v.write_control(0x8104); // mode 5
        v
    }

    /// Runs a whole frame of line starts, returning the lines where a
    /// horizontal interrupt became pending (acknowledged right away).
    fn hint_lines(v: &mut Vdp) -> Vec<u16> {
        let mut lines = Vec::new();
        for line in 0..v.lines_per_frame() {
            v.start_line(line);
            if v.irq_level() == HINT_LEVEL {
                lines.push(line);
                v.acknowledge_interrupt();
            }
        }
        lines
    }

    #[test]
    fn hint_every_eighth_line() {
        let mut v = vdp();
        v.write_control(0x8014); // IE1
        v.write_control(0x8A07); // every 8 lines
        hint_lines(&mut v); // first frame: counter starts at 0
        let lines = hint_lines(&mut v);
        assert_eq!(lines.len(), 28);
        assert_eq!(lines[0], 7);
        assert_eq!(*lines.last().unwrap(), 223);
    }

    #[test]
    fn hint_counter_zero_fires_on_every_active_line_and_line_224() {
        let mut v = vdp();
        v.write_control(0x8014);
        v.write_control(0x8A00);
        let lines = hint_lines(&mut v);
        assert_eq!(lines.len(), 225);
        assert_eq!(*lines.last().unwrap(), 224);
    }

    #[test]
    fn vint_priority_ack_and_status_flag() {
        let mut v = vdp();
        v.write_control(0x8014); // IE1
        v.write_control(0x8164); // display, IE0, mode 5
        v.write_control(0x8A00);
        v.start_line(0); // HINT pending
        v.trigger_vint();
        assert_ne!(v.read_status() & status::VINT_PENDING, 0);
        assert_eq!(v.irq_level(), VINT_LEVEL);
        v.acknowledge_interrupt();
        assert_eq!(v.read_status() & status::VINT_PENDING, 0);
        assert_eq!(v.irq_level(), HINT_LEVEL);
        v.acknowledge_interrupt();
        assert_eq!(v.irq_level(), 0);
    }

    #[test]
    fn pending_interrupt_fires_when_enabled_later() {
        let mut v = vdp();
        v.trigger_vint();
        assert_eq!(v.irq_level(), 0, "IE0 clear");
        v.write_control(0x8124);
        assert_eq!(v.irq_level(), VINT_LEVEL);
    }

    #[test]
    fn vblank_flag_span() {
        let mut v = vdp();
        v.write_control(0x8144);
        v.start_line(223);
        assert_eq!(v.read_status() & status::VBLANK, 0);
        v.start_line(224);
        assert_ne!(v.read_status() & status::VBLANK, 0);
        v.start_line(LINES_NTSC - 1);
        assert_eq!(v.read_status() & status::VBLANK, 0);
    }

    /// PAL VDP in mode 5 with V30 (register 1 = $0C), one frame started so
    /// that the height is latched.
    fn pal_v30() -> Vdp {
        let mut v = Vdp::new(true);
        v.write_control(0x810C);
        v.start_line(0);
        v
    }

    #[test]
    fn v30_moves_vblank_to_line_240() {
        let mut v = pal_v30();
        assert_eq!(v.active_lines(), 240);
        v.start_line(239);
        assert!(!v.in_vblank(), "line 239 is active in V30");
        v.start_line(240);
        assert!(v.in_vblank());
        // V28 (default): VBLANK from line 224.
        let mut v = vdp();
        v.start_line(0);
        assert_eq!(v.active_lines(), 224);
        v.start_line(224);
        assert!(v.in_vblank());
    }

    #[test]
    fn v30_takes_effect_on_the_next_frame_and_needs_mode_5() {
        let mut v = vdp();
        v.start_line(0);
        v.write_control(0x810C); // V30 in the middle of the frame
        v.start_line(100);
        assert_eq!(v.active_lines(), 224, "unchanged until the next frame");
        v.start_line(0);
        assert_eq!(v.active_lines(), 240);
        // Bit 3 without mode 5 (register 1 = $08): ignored.
        let mut v = Vdp::new(true);
        v.write_control(0x8108);
        v.start_line(0);
        assert_eq!(v.active_lines(), 224);
    }

    #[test]
    fn v30_hint_counter_runs_over_240_lines() {
        let mut v = pal_v30();
        v.write_control(0x8A00 | 59); // H-int every 60 lines
        let mut lines = Vec::new();
        for line in 0..v.lines_per_frame() {
            v.start_line(line);
            if std::mem::take(&mut v.hint_pending) {
                lines.push(line);
            }
        }
        // Lines 0 (counter starts at 0), 60, 120, 180, then 240 (first VBLANK
        // line, counter at 0 again).
        assert_eq!(lines, [0, 60, 120, 180, 240]);
    }

    #[test]
    fn v30_v_counter() {
        // PAL V30 (GPGX vc_table): $00-$10A, then $1D2-$1FF.
        let mut v = pal_v30();
        v.start_line(0x10A);
        assert_eq!(v.hv_counter() >> 8, 0x0A);
        v.start_line(0x10B);
        assert_eq!(v.hv_counter() >> 8, 0xD2);
        v.start_line(LINES_PAL - 1);
        assert_eq!(v.hv_counter() >> 8, 0xFF);
        // NTSC V30: no jump, the counter follows the line.
        let mut v = Vdp::new(false);
        v.write_control(0x810C);
        v.start_line(0);
        v.start_line(LINES_NTSC - 1);
        assert_eq!(v.hv_counter() >> 8, (LINES_NTSC - 1) & 0xFF);
    }

    #[test]
    fn v30_renders_and_saves_240_lines() {
        use savestate::{Reader, State, Writer};
        let mut v = pal_v30();
        v.write_control(0x814C); // display on, mode 5, V30
        v.write_control(0x8004); // full palette
        v.write_control(0x8F02); // auto-increment 2
        v.write_control(0x8701); // backdrop: CRAM entry 1
        v.write_control(0xC000);
        v.write_control(0x0000); // CRAM write at 0
        v.write_data(0);
        v.write_data(0x00E0); // green
        v.render_line(239);
        assert_eq!(
            v.framebuffer()[239 * crate::FRAME_WIDTH],
            crate::cram_to_rgba(0x00E0)
        );
        let mut w = Writer::new();
        v.visit(&mut w).unwrap();
        let mut restored = Vdp::new(true);
        restored.visit(&mut Reader::new(&w.into_bytes())).unwrap();
        assert_eq!(restored.active_lines(), 240);
    }

    #[test]
    fn v_counter_jumps() {
        let mut v = vdp();
        let mut at = |line| {
            v.start_line(line);
            v.hv_counter() >> 8
        };
        assert_eq!(at(0), 0x00);
        assert_eq!(at(0xEA), 0xEA);
        assert_eq!(at(0xEB), 0xE5);
        assert_eq!(at(LINES_NTSC - 1), 0xFF);
        let mut pal = Vdp::new(true);
        pal.start_line(0x103);
        assert_eq!(pal.hv_counter() >> 8, 0xCA);
        pal.start_line(LINES_PAL - 1);
        assert_eq!(pal.hv_counter() >> 8, 0xFF);
    }

    #[test]
    fn h_counter_range_and_jump() {
        let mut v = vdp();
        v.write_control(0x8C81); // H40
        v.start_line(0);
        assert_eq!(v.hv_counter() & 0xFF, 0x00);
        v.set_line_cycle(3419);
        assert_eq!(v.hv_counter() & 0xFF, 0xFF);
        // Just past the jump point the counter reads $E4.
        let steps = h_steps(H40_LAST_BEFORE_JUMP, H40_JUMP_TO);
        v.set_line_cycle((H40_LAST_BEFORE_JUMP + 1) * MASTER_CYCLES_PER_LINE / steps + 16);
        assert_eq!(v.hv_counter() & 0xFF, 0xE4);
    }
}

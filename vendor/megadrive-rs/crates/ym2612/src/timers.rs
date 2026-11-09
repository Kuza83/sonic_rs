//! Timers A (10-bit, one tick per sample) and B (8-bit, one tick per 16
//! samples): load on the cycle the load bit is seen, overflow flags set when
//! enabled, CSM key-on from timer A in channel 3 CSM mode.

use crate::Ym2612;

/// Test register $21 bit 2 makes the timers count every cycle.
const TEST21_FAST_TIMERS: u8 = 1 << 2;
const CSM_MODE: u8 = 2;

#[derive(Debug, Clone, Default)]
pub(crate) struct Timers {
    pub a_reg: u16,
    pub a_load: bool,
    pub a_enable: bool,
    pub a_reset: bool,
    pub a_flag: bool,
    a_count: u16,
    a_overflow: bool,
    a_load_lock: bool,
    a_load_latch: bool,
    pub b_reg: u8,
    pub b_load: bool,
    pub b_enable: bool,
    pub b_reset: bool,
    pub b_flag: bool,
    b_count: u16,
    b_sub: u8,
    b_overflow: bool,
    b_load_lock: bool,
    b_load_latch: bool,
}

impl Ym2612 {
    pub(crate) fn clock_timers(&mut self) {
        let fast = self.test21 & TEST21_FAST_TIMERS != 0;
        let cycle = self.cycle;
        let csm = self.mode_ch3 == CSM_MODE;
        let t = &mut self.timers;

        // Timer A.
        let mut load = t.a_overflow;
        if cycle == 2 {
            load |= !t.a_load_lock && t.a_load;
            t.a_load_lock = t.a_load;
            self.kon_csm = csm && load;
        }
        let mut time = if t.a_load_latch { t.a_reg } else { t.a_count };
        t.a_load_latch = load;
        if (cycle == 1 && t.a_load_lock) || fast {
            time += 1;
        }
        if t.a_reset {
            t.a_reset = false;
            t.a_flag = false;
        } else {
            t.a_flag |= t.a_overflow && t.a_enable;
        }
        t.a_overflow = time >> 10 != 0;
        t.a_count = time & 0x3FF;

        // Timer B.
        let mut load = t.b_overflow;
        if cycle == 2 {
            load |= !t.b_load_lock && t.b_load;
            t.b_load_lock = t.b_load;
        }
        let mut time = if t.b_load_latch {
            t.b_reg as u16
        } else {
            t.b_count
        };
        t.b_load_latch = load;
        if cycle == 1 {
            t.b_sub += 1;
        }
        if (t.b_sub == 0x10 && t.b_load_lock) || fast {
            time += 1;
        }
        t.b_sub &= 0x0F;
        if t.b_reset {
            t.b_reset = false;
            t.b_flag = false;
        } else {
            t.b_flag |= t.b_overflow && t.b_enable;
        }
        t.b_overflow = time >> 8 != 0;
        t.b_count = time & 0xFF;
    }
}

impl savestate::State for Timers {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.a_reg)?;
        for flag in [
            &mut self.a_load,
            &mut self.a_enable,
            &mut self.a_reset,
            &mut self.a_flag,
            &mut self.a_overflow,
            &mut self.a_load_lock,
            &mut self.a_load_latch,
        ] {
            v.field(flag)?;
        }
        v.field(&mut self.a_count)?;
        v.field(&mut self.b_reg)?;
        for flag in [
            &mut self.b_load,
            &mut self.b_enable,
            &mut self.b_reset,
            &mut self.b_flag,
            &mut self.b_overflow,
            &mut self.b_load_lock,
            &mut self.b_load_latch,
        ] {
            v.field(flag)?;
        }
        v.field(&mut self.b_count)?;
        v.field(&mut self.b_sub)
    }
}

//! The YM2612 inside the machine: kept in step with the CPUs.
//!
//! The chip advances one internal cycle every 42 master cycles. It is caught
//! up to the current CPU time whenever it is accessed (so the busy flag and
//! the timers are seen at the right moment by polling sound drivers) and at
//! the end of every line. Each 24 internal cycles make one native sample
//! (~53.3 kHz), handed to `audio.rs` for resampling.

use ym2612::{CYCLES_PER_SAMPLE, MASTER_CYCLES_PER_CLOCK, Ym2612};

/// Native samples per line, rounded up (3420 / (42 × 24) ≈ 3.4).
const SAMPLES_PER_LINE_CAPACITY: usize = 8;

#[derive(Debug, Clone)]
pub struct FmUnit {
    chip: Ym2612,
    /// Master cycles of the current line already simulated.
    time: u32,
    /// Master cycles carried towards the next internal clock.
    phase: u32,
    sum: (i32, i32),
    clocks_in_sample: u32,
    samples: Vec<(i32, i32)>,
}

impl Default for FmUnit {
    fn default() -> Self {
        Self::new()
    }
}

impl FmUnit {
    pub fn new() -> Self {
        Self {
            chip: Ym2612::new(),
            time: 0,
            phase: 0,
            sum: (0, 0),
            clocks_in_sample: 0,
            samples: Vec::with_capacity(SAMPLES_PER_LINE_CAPACITY),
        }
    }

    /// Runs the chip up to master cycle `now` of the line.
    pub fn catch_up(&mut self, now: u32) {
        if now <= self.time {
            return;
        }
        let mut cycles = self.phase + (now - self.time);
        self.time = now;
        while cycles >= MASTER_CYCLES_PER_CLOCK {
            cycles -= MASTER_CYCLES_PER_CLOCK;
            let (l, r) = self.chip.clock();
            self.sum.0 += l as i32;
            self.sum.1 += r as i32;
            self.clocks_in_sample += 1;
            if self.clocks_in_sample == CYCLES_PER_SAMPLE {
                self.samples.push(self.sum);
                self.sum = (0, 0);
                self.clocks_in_sample = 0;
            }
        }
        self.phase = cycles;
    }

    /// Sound synthesis on or off (`Ym2612::set_synthesis`).
    pub fn set_synthesis(&mut self, on: bool) {
        self.chip.set_synthesis(on);
    }

    pub fn synthesis(&self) -> bool {
        self.chip.synthesis()
    }

    /// /IC: the chip restarts from its power-on state; time base kept (and
    /// the synthesis setting, a run-time option).
    pub fn reset_chip(&mut self) {
        let synthesis = self.chip.synthesis();
        self.chip = Ym2612::new();
        self.chip.set_synthesis(synthesis);
    }

    pub fn write(&mut self, now: u32, port: u8, data: u8) {
        self.catch_up(now);
        self.chip.write(port, data);
    }

    pub fn read(&mut self, now: u32, port: u8) -> u8 {
        self.catch_up(now);
        self.chip.read(port)
    }

    /// Completes the line (`length` master cycles) and restarts the time base.
    pub fn end_line(&mut self, length: u32) {
        self.catch_up(length);
        self.time = 0;
    }

    /// Native samples produced since the last call.
    pub fn drain(&mut self) -> std::vec::Drain<'_, (i32, i32)> {
        self.samples.drain(..)
    }
}

impl savestate::State for FmUnit {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"FMU ", |v| {
            v.state(&mut self.chip)?;
            v.field(&mut self.time)?;
            v.field(&mut self.phase)?;
            v.field(&mut self.sum)?;
            v.field(&mut self.clocks_in_sample)?;
            v.field(&mut self.samples)
        })
    }
}

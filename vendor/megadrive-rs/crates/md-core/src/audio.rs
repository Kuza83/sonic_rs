//! Audio sample generation, resampling and mixing.
//!
//! The PSG ticks every 15 × 16 = 240 master cycles (its input clock is the
//! master clock / 15, divided by 16 inside the chip); the YM2612 produces a
//! native sample every 42 × 24 = 1008 master cycles (`fm.rs`). Each source is
//! resampled to `OUTPUT_RATE` Hz by averaging the inputs each output sample
//! covers (box filter), timed in integer master cycles (no drift, no float);
//! the two streams are then mixed pairwise.
//!
//! Mix (a documented choice, not a measured console balance): YM2612 × 4
//! (its native sum spans about ±4.6k), PSG / 16 (four channels at ±32767).

use std::collections::VecDeque;

use crate::bus::MdBus;

pub const OUTPUT_RATE: u32 = 44_100;
/// Master clock (Sega Retro): NTSC 53.693175 MHz, PAL 53.203424 MHz.
pub const MASTER_CLOCK_NTSC: u32 = 53_693_175;
pub const MASTER_CLOCK_PAL: u32 = 53_203_424;
/// Master cycles per PSG tick.
const PSG_TICK_MASTER: u32 = 15 * psg::CLOCK_DIVIDER;
/// Mixing gains. YM2612: the sum of the 24 internal cycles of a native
/// sample (as `FmUnit` makes it) times 11, as Genesis Plus GX does with the
/// Nuked core (sound.c `*buffer++ = ym3438_sample[0] * 11;`, FM preamp
/// 100 %). PSG: output / 16, measured at GPGX's level (PSG preamp 150 %) on
/// `psg_tone` (docs/decisions.md, A3).
const PSG_GAIN_SHIFT: u32 = 4;
const YM_GAIN: i32 = 11;
/// Master cycles per YM2612 native sample.
const YM_SAMPLE_MASTER: u64 = (ym2612::MASTER_CYCLES_PER_CLOCK * ym2612::CYCLES_PER_SAMPLE) as u64;
/// Queues between the resamplers and the mixer (a frame at most).
const QUEUE_CAPACITY: usize = 1024;

/// Box-filter resampler from a fixed input period to `OUTPUT_RATE`.
#[derive(Debug, Clone)]
struct Resampler {
    input_period: u64,
    phase: u64,
    sum: (i64, i64),
    count: u32,
}

impl Resampler {
    fn new(input_period: u64) -> Self {
        Self {
            input_period,
            phase: 0,
            sum: (0, 0),
            count: 0,
        }
    }

    /// Adds one input sample; pushes the output samples it completes.
    fn push(&mut self, master_clock: u64, l: i64, r: i64, out: &mut VecDeque<(i32, i32)>) {
        self.sum.0 += l;
        self.sum.1 += r;
        self.count += 1;
        self.phase += self.input_period * OUTPUT_RATE as u64;
        while self.phase >= master_clock {
            self.phase -= master_clock;
            let n = self.count.max(1) as i64;
            out.push_back(((self.sum.0 / n) as i32, (self.sum.1 / n) as i32));
            self.sum = (0, 0);
            self.count = 0;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Audio {
    master_clock: u32,
    /// Master cycles until the next PSG tick.
    psg_countdown: u32,
    psg: Resampler,
    ym: Resampler,
    psg_out: VecDeque<(i32, i32)>,
    ym_out: VecDeque<(i32, i32)>,
    /// Interleaved left/right samples produced since the last `clear`.
    samples: Vec<i16>,
}

impl Audio {
    pub fn new(pal: bool) -> Self {
        Self {
            master_clock: if pal {
                MASTER_CLOCK_PAL
            } else {
                MASTER_CLOCK_NTSC
            },
            psg_countdown: PSG_TICK_MASTER,
            psg: Resampler::new(PSG_TICK_MASTER as u64),
            ym: Resampler::new(YM_SAMPLE_MASTER),
            psg_out: VecDeque::with_capacity(QUEUE_CAPACITY),
            ym_out: VecDeque::with_capacity(QUEUE_CAPACITY),
            samples: Vec::with_capacity(2 * OUTPUT_RATE as usize / 50 + 16),
        }
    }

    /// Advances the PSG by `master` cycles, takes the YM2612 samples of the
    /// same period, and mixes what both streams have produced.
    pub fn run(&mut self, bus: &mut MdBus, mut master: u32) {
        let clock = self.master_clock as u64;
        while master >= self.psg_countdown {
            master -= self.psg_countdown;
            self.psg_countdown = PSG_TICK_MASTER;
            let psg = bus.psg_mut();
            psg.tick();
            let v = psg.output() as i64;
            self.psg.push(clock, v, v, &mut self.psg_out);
        }
        self.psg_countdown -= master;
        for (l, r) in bus.fm_mut().drain() {
            self.ym.push(clock, l as i64, r as i64, &mut self.ym_out);
        }
        while let (Some(p), Some(y)) = (self.psg_out.front(), self.ym_out.front()) {
            let mix = |p: i32, y: i32| {
                ((p >> PSG_GAIN_SHIFT) + y * YM_GAIN).clamp(i16::MIN as i32, i16::MAX as i32) as i16
            };
            self.samples.push(mix(p.0, y.0));
            self.samples.push(mix(p.1, y.1));
            self.psg_out.pop_front();
            self.ym_out.pop_front();
        }
    }

    /// Samples produced so far (interleaved stereo); cleared by `clear`.
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

impl savestate::State for Resampler {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.input_period)?;
        v.field(&mut self.phase)?;
        v.field(&mut self.sum)?;
        v.field(&mut self.count)
    }
}

impl savestate::State for Audio {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"AUDI", |v| {
            v.field(&mut self.master_clock)?;
            v.field(&mut self.psg_countdown)?;
            v.state(&mut self.psg)?;
            v.state(&mut self.ym)?;
            v.field(&mut self.psg_out)?;
            v.field(&mut self.ym_out)?;
            v.field(&mut self.samples)
        })
    }
}

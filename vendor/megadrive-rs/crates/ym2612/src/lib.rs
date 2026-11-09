//! Yamaha YM2612 (OPN2), modelled cycle by cycle.
//!
//! The chip is a pipeline time-multiplexed over 24 internal cycles (one per
//! operator slot; the master clock is divided by 7, then by 6 by the chip's
//! prescaler). Each `clock()` advances one internal cycle and returns the
//! DAC output of that cycle; an audio sample is the sum of 24 cycles.
//!
//! The model follows the behaviour reverse-engineered by Nuked-OPN2
//! (nukeykt), read to understand the hardware and used as a test oracle;
//! the code here is an independent implementation (CLAUDE.md).

mod generator;
mod io;
mod output;
mod tables;
mod timers;

/// Internal cycles per output sample (24 operator slots).
pub const CYCLES_PER_SAMPLE: u32 = 24;
/// Master clock divider per internal cycle: /7 (YM clock) then /6 (prescaler).
pub const MASTER_CYCLES_PER_CLOCK: u32 = 7 * 6;
const SLOTS: usize = 24;
const CHANNELS: usize = 6;

/// Per-operator registers ($30-$9F), indexed by slot.
#[derive(Debug, Clone, Default)]
pub(crate) struct OperatorRegs {
    /// Multiplier ×2 (MUL 0 means ×½, stored as 1).
    pub multi: [u8; SLOTS],
    pub dt: [u8; SLOTS],
    pub tl: [u8; SLOTS],
    pub ar: [u8; SLOTS],
    pub ks: [u8; SLOTS],
    pub dr: [u8; SLOTS],
    pub am: [bool; SLOTS],
    pub sr: [u8; SLOTS],
    pub rr: [u8; SLOTS],
    /// Sustain level, 15 extended to 31.
    pub sl: [u8; SLOTS],
    pub ssg_eg: [u8; SLOTS],
}

/// Per-channel registers ($A0-$B6).
#[derive(Debug, Clone, Default)]
pub(crate) struct ChannelRegs {
    pub fnum: [u16; CHANNELS],
    pub block: [u8; CHANNELS],
    pub kcode: [u8; CHANNELS],
    /// Channel 3 special mode frequencies ($A8-$AE) for OP1, OP2, OP3.
    pub fnum_3ch: [u16; 3],
    pub block_3ch: [u8; 3],
    pub kcode_3ch: [u8; 3],
    pub reg_a4: u8,
    pub reg_ac: u8,
    pub connect: [u8; CHANNELS],
    pub fb: [u8; CHANNELS],
    pub pms: [u8; CHANNELS],
    pub ams: [u8; CHANNELS],
    pub pan_l: [bool; CHANNELS],
    pub pan_r: [bool; CHANNELS],
}

#[derive(Debug, Clone)]
pub struct Ym2612 {
    /// Current internal cycle (0-23) and channel (cycle % 6).
    pub(crate) cycle: usize,
    pub(crate) io: io::Io,
    pub(crate) op: OperatorRegs,
    pub(crate) ch: ChannelRegs,
    pub(crate) timers: timers::Timers,
    pub(crate) out: output::Output,
    /// Test registers $21 and $2C, bit by bit.
    pub(crate) test21: u8,
    pub(crate) test2c: u8,
    pub(crate) lfo_enable: bool,
    pub(crate) lfo_freq: u8,
    /// Channel 3 mode ($27 bits 7-6): 0 normal, 1 special, 2 CSM.
    pub(crate) mode_ch3: u8,
    /// Key-on register ($28) latch: operators and target channel.
    pub(crate) kon_operators: [bool; 4],
    pub(crate) kon_channel: u8,
    pub(crate) mode_kon: [bool; SLOTS],
    pub(crate) kon_csm: bool,
    /// DAC: 9-bit data ($2A, sign-flipped, plus the $2C test bit) and enable ($2B).
    pub(crate) dac_data: u16,
    pub(crate) dac_enable: bool,
    /// Operator outputs (14-bit) per slot, produced by the FM stage.
    pub(crate) fm_out: [i16; SLOTS],
    pub(crate) generator: generator::Generator,
    /// Last status byte and how long reads on other ports still return it.
    status: u8,
    status_time: u32,
    /// Sound synthesis on (default). Off, a cycle only keeps what the CPU
    /// can read (status: busy flag, timer flags) and outputs silence: for
    /// training an agent, where sound is not needed. A run-time option, not
    /// part of the saved state.
    synthesis: bool,
}

/// Slots and channel the pipeline stages work on at a given cycle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CycleIndices {
    pub channel: usize,
    /// `(cycle + n) % SLOTS`.
    pub plus6: usize,
    pub plus18: usize,
    pub plus19: usize,
    pub plus20: usize,
    pub plus22: usize,
    pub plus23: usize,
}

pub(crate) const CYCLE_INDICES: [CycleIndices; SLOTS] = {
    let mut table = [CycleIndices {
        channel: 0,
        plus6: 0,
        plus18: 0,
        plus19: 0,
        plus20: 0,
        plus22: 0,
        plus23: 0,
    }; SLOTS];
    let mut c = 0;
    while c < SLOTS {
        table[c] = CycleIndices {
            channel: c % CHANNELS,
            plus6: (c + 6) % SLOTS,
            plus18: (c + 18) % SLOTS,
            plus19: (c + 19) % SLOTS,
            plus20: (c + 20) % SLOTS,
            plus22: (c + 22) % SLOTS,
            plus23: (c + 23) % SLOTS,
        };
        c += 1;
    }
    table
};

impl Default for Ym2612 {
    fn default() -> Self {
        Self::new()
    }
}

/// How long the last status stays readable on ports 1-3 of a YM2612
/// (in internal cycles), as modelled by Nuked-OPN2.
const STATUS_HOLD_CYCLES: u32 = 300_000;

impl Ym2612 {
    /// Power-on state (all operators released, both pans on).
    pub fn new() -> Self {
        let op = OperatorRegs {
            multi: [1; SLOTS],
            ..Default::default()
        };
        let ch = ChannelRegs {
            pan_l: [true; CHANNELS],
            pan_r: [true; CHANNELS],
            ..Default::default()
        };
        Self {
            cycle: 0,
            io: io::Io::default(),
            op,
            ch,
            timers: timers::Timers::default(),
            out: output::Output::default(),
            test21: 0,
            test2c: 0,
            lfo_enable: false,
            lfo_freq: 0,
            mode_ch3: 0,
            kon_operators: [false; 4],
            kon_channel: 0,
            mode_kon: [false; SLOTS],
            kon_csm: false,
            dac_data: 0,
            dac_enable: false,
            fm_out: [0; SLOTS],
            generator: generator::Generator::default(),
            status: 0,
            status_time: 0,
            synthesis: true,
        }
    }

    /// Turns the sound synthesis on or off (see the `synthesis` field). The
    /// game sees no difference: the status register behaves the same.
    /// Turned back on, the sound resumes from stale operator state.
    pub fn set_synthesis(&mut self, on: bool) {
        self.synthesis = on;
    }

    pub fn synthesis(&self) -> bool {
        self.synthesis
    }

    pub(crate) fn channel(&self) -> usize {
        self.at().channel
    }

    /// Indices derived from the current cycle (precomputed: every stage
    /// used to recompute them with divisions).
    pub(crate) fn at(&self) -> CycleIndices {
        CYCLE_INDICES[self.cycle]
    }

    /// CPU write to port 0-3 (0/2: address of part I/II, 1/3: data).
    pub fn write(&mut self, port: u8, data: u8) {
        self.io.cpu_write(port, data);
    }

    /// CPU read. Port 0 returns busy (bit 7) and the timer flags (bits 1-0);
    /// the other ports return the last status for a while, then 0.
    pub fn read(&mut self, port: u8) -> u8 {
        if port & 3 == 0 {
            self.status = (self.io.busy as u8) << 7
                | (self.timers.b_flag as u8) << 1
                | self.timers.a_flag as u8;
            self.status_time = STATUS_HOLD_CYCLES;
        }
        if self.status_time > 0 { self.status } else { 0 }
    }

    /// One internal cycle; returns this cycle's (left, right) DAC output.
    pub fn clock(&mut self) -> (i16, i16) {
        if !self.synthesis {
            return self.clock_status_only();
        }
        self.clock_generator_start();
        self.io.detect_writes();
        self.clock_timers();
        self.clock_key_on();
        let out = self.channel_output();
        self.channel_accumulate();
        self.fm_prepare();
        self.fm_generate();
        self.phase_generate();
        self.phase_increment();
        self.envelope_adsr();
        self.envelope_output();
        self.envelope_ssg();
        self.envelope_prepare();
        self.prepare_frequency();
        self.update_lfo();
        self.apply_register_writes();
        self.cycle = if self.cycle + 1 == SLOTS {
            0
        } else {
            self.cycle + 1
        };
        if self.status_time > 0 {
            self.status_time -= 1;
        }
        out
    }

    /// A cycle without synthesis: only what the status register depends on
    /// (the busy flag from the write detection, the timers, the register
    /// writes that program them, the cycle and the status hold time), in
    /// the order of `clock`. The skipped stages produce sound only; the one
    /// signal from the timers to them (`kon_csm`, CSM key-on) is read by the
    /// envelope generator alone.
    fn clock_status_only(&mut self) -> (i16, i16) {
        self.io.detect_writes();
        self.clock_timers();
        self.apply_register_writes();
        self.cycle = if self.cycle + 1 == SLOTS {
            0
        } else {
            self.cycle + 1
        };
        if self.status_time > 0 {
            self.status_time -= 1;
        }
        (0, 0)
    }

    /// Key-on: per-slot latch for the envelope generator, then the key-on
    /// register applied on its channel's cycle.
    fn clock_key_on(&mut self) {
        self.latch_key_on();
        let chan = self.channel();
        if self.cycle == self.kon_channel as usize {
            let [op1, op2, op3, op4] = self.kon_operators;
            self.mode_kon[chan] = op1;
            self.mode_kon[chan + 12] = op2;
            self.mode_kon[chan + 6] = op3;
            self.mode_kon[chan + 18] = op4;
        }
    }

    /// Runs `clocks` internal cycles and returns the summed outputs.
    pub fn run(&mut self, clocks: u32) -> (i32, i32) {
        let (mut l, mut r) = (0, 0);
        for _ in 0..clocks {
            let (a, b) = self.clock();
            l += a as i32;
            r += b as i32;
        }
        (l, r)
    }
}

impl savestate::State for OperatorRegs {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        for table in [
            &mut self.multi,
            &mut self.dt,
            &mut self.tl,
            &mut self.ar,
            &mut self.ks,
            &mut self.dr,
            &mut self.sr,
            &mut self.rr,
            &mut self.sl,
            &mut self.ssg_eg,
        ] {
            v.field(table)?;
        }
        v.field(&mut self.am)
    }
}

impl savestate::State for ChannelRegs {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.fnum)?;
        v.field(&mut self.block)?;
        v.field(&mut self.kcode)?;
        v.field(&mut self.fnum_3ch)?;
        v.field(&mut self.block_3ch)?;
        v.field(&mut self.kcode_3ch)?;
        v.field(&mut self.reg_a4)?;
        v.field(&mut self.reg_ac)?;
        v.field(&mut self.connect)?;
        v.field(&mut self.fb)?;
        v.field(&mut self.pms)?;
        v.field(&mut self.ams)?;
        v.field(&mut self.pan_l)?;
        v.field(&mut self.pan_r)
    }
}

/// The whole chip, pipeline included. Values used as table indices or
/// shift amounts are checked so a damaged state is an error, not a panic.
impl savestate::State for Ym2612 {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"YM26", |v| {
            v.field(&mut self.cycle)?;
            v.state(&mut self.io)?;
            v.state(&mut self.op)?;
            v.state(&mut self.ch)?;
            v.state(&mut self.timers)?;
            v.state(&mut self.out)?;
            v.field(&mut self.test21)?;
            v.field(&mut self.test2c)?;
            v.field(&mut self.lfo_enable)?;
            v.field(&mut self.lfo_freq)?;
            v.field(&mut self.mode_ch3)?;
            v.field(&mut self.kon_operators)?;
            v.field(&mut self.kon_channel)?;
            v.field(&mut self.mode_kon)?;
            v.field(&mut self.kon_csm)?;
            v.field(&mut self.dac_data)?;
            v.field(&mut self.dac_enable)?;
            v.field(&mut self.fm_out)?;
            v.state(&mut self.generator)?;
            v.field(&mut self.status)?;
            v.field(&mut self.status_time)?;
            self.check_ranges()
        })
    }
}

impl Ym2612 {
    fn check_ranges(&self) -> savestate::Result {
        let invalid = savestate::StateError::InvalidValue;
        let ok = self.cycle < SLOTS
            && self.lfo_freq < 8
            && self.ch.connect.iter().all(|&a| a < 8)
            && self.ch.fb.iter().all(|&f| f < 8)
            && self.ch.pms.iter().all(|&p| p < 8)
            && self.ch.ams.iter().all(|&a| a < 4)
            && self
                .ch
                .block
                .iter()
                .chain(&self.ch.block_3ch)
                .all(|&b| b < 8)
            && self.op.dt.iter().all(|&d| d < 8)
            && self.op.ks.iter().all(|&k| k < 4);
        if ok {
            Ok(())
        } else {
            Err(invalid("YM2612 register"))
        }
    }
}

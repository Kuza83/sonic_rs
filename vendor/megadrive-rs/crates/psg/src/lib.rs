//! Texas Instruments SN76489, Sega variant (Mega Drive PSG).
//!
//! Source: "SN76489 notes" by Maxim (SMS Power, Development/SN76489):
//! - write protocol: `1 cc t dddd` latches channel cc and type t (0 tone /
//!   noise, 1 volume) with the low data bits; `0 - dddddd` data bytes update
//!   the latched register (tone: bits 9-4; volume/noise: bits 3-0); the
//!   latch is never cleared by data bytes;
//! - internal clock = input clock / 16; a tone counter reloads from its
//!   10-bit register when it reaches zero and flips the channel output; a
//!   register value of 0 or 1 gives a constant output;
//! - noise: reload 0x10/0x20/0x40 or tone 2's output; 16-bit LFSR (Sega),
//!   white noise taps bits 0 and 3, reset to 0x8000 on any noise register
//!   write; it shifts on each 0→1 transition of its input and the bit shifted
//!   out goes to the mixer;
//! - volume: 2 dB per step, 15 = silence.
//!
//! Channel outputs are bipolar (±volume), as the notes advise for emulation.

/// Volume table (SMS Power, 2 dB steps, 0xF forced to 0).
pub const VOLUME_TABLE: [i32; 16] = [
    32767, 26028, 20675, 16422, 13045, 10362, 8231, 6568, 5193, 4125, 3277, 2603, 2067, 1642, 1304,
    0,
];

/// Input clock divider to the internal tick.
pub const CLOCK_DIVIDER: u32 = 16;
const LATCH_BIT: u8 = 0x80;
const VOLUME_TYPE_BIT: u8 = 0x10;
const NOISE_CHANNEL: usize = 3;
const NOISE_WHITE_BIT: u8 = 0x04;
const NOISE_RATE_MASK: u8 = 0x03;
/// Noise counter reload values for rates 0-2; rate 3 follows tone 2.
const NOISE_RELOADS: [u16; 3] = [0x10, 0x20, 0x40];
const NOISE_RATE_TONE2: u8 = 3;
const LFSR_RESET: u16 = 0x8000;
/// White noise feedback taps (bits 0 and 3), into bit 15.
const LFSR_TAPS: u16 = 0x0009;
const TONE_MASK: u16 = 0x3FF;
const SILENT: u8 = 0x0F;

#[derive(Debug, Clone)]
pub struct Psg {
    tone: [u16; 3],
    volume: [u8; 4],
    noise: u8,
    /// Latched channel (0-3) and whether the volume register is latched.
    latch_channel: usize,
    latch_volume: bool,
    counters: [u16; 4],
    /// Square wave flip-flops of the tone channels, and the noise input.
    flip: [bool; 4],
    lfsr: u16,
    noise_out: bool,
}

impl Default for Psg {
    fn default() -> Self {
        Self::new()
    }
}

impl Psg {
    /// Power-on state: all channels silent.
    pub fn new() -> Self {
        Self {
            tone: [0; 3],
            volume: [SILENT; 4],
            noise: 0,
            latch_channel: 0,
            latch_volume: false,
            counters: [0; 4],
            flip: [false; 4],
            lfsr: LFSR_RESET,
            noise_out: false,
        }
    }

    pub fn write(&mut self, byte: u8) {
        if byte & LATCH_BIT != 0 {
            self.latch_channel = ((byte >> 5) & 3) as usize;
            self.latch_volume = byte & VOLUME_TYPE_BIT != 0;
            self.write_latched(byte & 0x0F, true);
        } else {
            self.write_latched(byte & 0x3F, false);
        }
    }

    fn write_latched(&mut self, data: u8, from_latch: bool) {
        let ch = self.latch_channel;
        if self.latch_volume {
            self.volume[ch] = data & 0x0F;
        } else if ch == NOISE_CHANNEL {
            self.noise = data & 0x07;
            self.lfsr = LFSR_RESET;
        } else if from_latch {
            self.tone[ch] = (self.tone[ch] & !0x0F) | (data & 0x0F) as u16;
        } else {
            self.tone[ch] = (self.tone[ch] & 0x0F) | (((data & 0x3F) as u16) << 4);
        }
    }

    /// One internal tick (input clock / 16).
    pub fn tick(&mut self) {
        for ch in 0..3 {
            self.counters[ch] = self.counters[ch].wrapping_sub(1) & TONE_MASK;
            if self.counters[ch] == 0 {
                self.counters[ch] = self.tone[ch];
                self.flip[ch] = !self.flip[ch];
            }
        }
        // The noise input toggles at its own rate, or with tone 2.
        let rate = self.noise & NOISE_RATE_MASK;
        let toggled = if rate == NOISE_RATE_TONE2 {
            self.counters[2] == self.tone[2] && self.flip[2]
        } else {
            self.counters[NOISE_CHANNEL] = self.counters[NOISE_CHANNEL].wrapping_sub(1) & TONE_MASK;
            if self.counters[NOISE_CHANNEL] == 0 {
                self.counters[NOISE_CHANNEL] = NOISE_RELOADS[rate as usize];
                self.flip[NOISE_CHANNEL] = !self.flip[NOISE_CHANNEL];
                self.flip[NOISE_CHANNEL]
            } else {
                false
            }
        };
        if toggled {
            self.shift_lfsr();
        }
    }

    fn shift_lfsr(&mut self) {
        self.noise_out = self.lfsr & 1 != 0;
        let feedback = if self.noise & NOISE_WHITE_BIT != 0 {
            ((self.lfsr & LFSR_TAPS).count_ones() & 1) as u16
        } else {
            self.lfsr & 1
        };
        self.lfsr = (self.lfsr >> 1) | (feedback << 15);
    }

    /// Output level of one channel (bipolar, ±volume).
    pub fn channel_output(&self, ch: usize) -> i32 {
        let amplitude = VOLUME_TABLE[self.volume[ch] as usize];
        let high = if ch == NOISE_CHANNEL {
            self.noise_out
        } else if self.tone[ch] <= 1 {
            true // constant output
        } else {
            self.flip[ch]
        };
        if high { amplitude } else { -amplitude }
    }

    /// Sum of the four channels.
    pub fn output(&self) -> i32 {
        (0..4).map(|ch| self.channel_output(ch)).sum()
    }

    pub fn tone_register(&self, ch: usize) -> u16 {
        self.tone[ch]
    }

    pub fn volume_register(&self, ch: usize) -> u8 {
        self.volume[ch]
    }
}

impl savestate::State for Psg {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"PSG ", |v| {
            v.field(&mut self.tone)?;
            v.field(&mut self.volume)?;
            v.field(&mut self.noise)?;
            v.field(&mut self.latch_channel)?;
            v.field(&mut self.latch_volume)?;
            v.field(&mut self.counters)?;
            v.field(&mut self.flip)?;
            v.field(&mut self.lfsr)?;
            v.field(&mut self.noise_out)?;
            if self.latch_channel > 3 {
                return Err(savestate::StateError::InvalidValue("PSG latch"));
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Latch + data bytes setting tone `ch` to `value` at volume 0 (loudest).
    fn set_tone(psg: &mut Psg, ch: u8, value: u16) {
        psg.write(0x80 | (ch << 5) | (value & 0x0F) as u8);
        psg.write(((value >> 4) & 0x3F) as u8);
        psg.write(0x90 | (ch << 5));
    }

    #[test]
    fn latch_and_data_bytes() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 1, 0x2AB);
        assert_eq!(psg.tone_register(1), 0x2AB);
        // A data byte goes to the still-latched volume register of channel 1.
        psg.write(0x07);
        assert_eq!(psg.volume_register(1), 7);
        assert_eq!(psg.tone_register(1), 0x2AB);
    }

    #[test]
    fn tone_half_period_is_the_register_value() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 0, 5);
        // Count ticks between output flips.
        let mut last = psg.channel_output(0);
        let mut flips = Vec::new();
        for t in 0..60 {
            psg.tick();
            let now = psg.channel_output(0);
            if now != last {
                flips.push(t);
            }
            last = now;
        }
        let gaps: Vec<i32> = flips.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g == 5), "{gaps:?}");
    }

    #[test]
    fn tone_register_0_or_1_is_constant() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 2, 1);
        for _ in 0..20 {
            psg.tick();
            assert_eq!(psg.channel_output(2), VOLUME_TABLE[0]);
        }
    }

    #[test]
    fn silent_by_default() {
        let psg = Psg::new();
        assert_eq!(psg.output(), 0);
    }

    /// White noise at rate 0: reference bits computed directly from the
    /// LFSR definition (taps 0 and 3 into bit 15, from 0x8000).
    #[test]
    fn white_noise_sequence() {
        let mut psg = Psg::new();
        psg.write(0xE4); // noise: white, rate 0
        psg.write(0xF0); // noise volume 0
        let mut reference = Vec::new();
        let mut lfsr: u16 = 0x8000;
        for _ in 0..40 {
            reference.push(lfsr & 1 != 0);
            let fb = (lfsr ^ (lfsr >> 3)) & 1;
            lfsr = (lfsr >> 1) | (fb << 15);
        }
        // The 10-bit counter starts at 0: it wraps and first reaches 0 on tick
        // 1024 (first shift); then the input goes high every 2 × 0x10 ticks.
        let mut bits = Vec::new();
        for _ in 0..1024 {
            psg.tick();
        }
        bits.push(psg.channel_output(3) > 0);
        for _ in 1..40 {
            for _ in 0..2 * 0x10 {
                psg.tick();
            }
            bits.push(psg.channel_output(3) > 0);
        }
        assert_eq!(bits, reference);
    }
}

//! Channel accumulation and the DAC output stage.
//!
//! Operator outputs are summed per channel over the 4 operator cycles
//! (clamped to 9 bits); the result is latched and sent to the DAC, one
//! channel at a time. The YM2612's DAC shows a "ladder" offset: in every
//! cycle the output is either the channel value (the cycle it is enabled
//! for) or just its sign, both shifted up by one for non-negative values,
//! and the whole is amplified ×3 (Nuked-OPN2 "YM2612 DAC emulation").
//! Channel 6 can be replaced by the 9-bit DAC register.

use crate::Ym2612;

/// Test register $2C bit 5: DAC test mode (all cycles output the DAC).
const TEST2C_DAC: u8 = 1 << 5;
const DAC_AMPLIFY: i16 = 3;
const DAC_CYCLES_CHANNEL6: usize = 1;
/// Channel accumulator range (9-bit signed).
const ACC_MAX: i16 = 255;
const ACC_MIN: i16 = -256;

/// Algorithm output routing: whether operator `op` (0 = OP1 … 3 = OP4,
/// in slot order) is summed into the channel output, for algorithms 0-7.
const OUTPUT_OPERATORS: [[bool; 8]; 4] = [
    [false, false, false, false, false, false, false, true],
    [false, false, false, false, false, true, true, true],
    [false, false, false, false, true, true, true, true],
    [true, true, true, true, true, true, true, true],
];

#[derive(Debug, Clone, Default)]
pub(crate) struct Output {
    ch_acc: [i16; 6],
    ch_out: [i16; 6],
    ch_lock: i16,
    ch_lock_l: bool,
    ch_lock_r: bool,
}

fn sign_extend_9(v: u16) -> i16 {
    ((v << 7) as i16) >> 7
}

impl Ym2612 {
    pub(crate) fn channel_output(&mut self) -> (i16, i16) {
        let cycle = self.cycle;
        let test_dac = self.test2c & TEST2C_DAC != 0;
        let mut channel = self.channel();
        if cycle < 12 {
            channel += 1; // channels 4-6 are output during the first half
        }
        let channel = channel % 6;
        if cycle & 3 == 0 {
            if !test_dac {
                self.out.ch_lock = self.out.ch_out[channel];
            }
            self.out.ch_lock_l = self.ch.pan_l[channel];
            self.out.ch_lock_r = self.ch.pan_r[channel];
        }
        let value = if (cycle >> 2 == DAC_CYCLES_CHANNEL6 && self.dac_enable) || test_dac {
            sign_extend_9(self.dac_data)
        } else {
            self.out.ch_lock
        };
        let enabled = cycle & 3 == 3 || test_dac;
        let mut sign = value >> 8;
        let mut value = value;
        if value >= 0 {
            value += 1;
            sign += 1;
        }
        let l = if self.out.ch_lock_l && enabled {
            value
        } else {
            sign
        };
        let r = if self.out.ch_lock_r && enabled {
            value
        } else {
            sign
        };
        (l * DAC_AMPLIFY, r * DAC_AMPLIFY)
    }

    /// Sums the operator outputs of the channel being processed.
    pub(crate) fn channel_accumulate(&mut self) {
        let slot = (self.cycle + 18) % 24;
        let channel = self.channel();
        let op = slot / 6;
        let test_dac = self.test2c & TEST2C_DAC != 0;
        let mut acc = self.out.ch_acc[channel];
        if op == 0 && !test_dac {
            acc = 0;
        }
        let mut add = test_dac as i16;
        if OUTPUT_OPERATORS[op][self.ch.connect[channel] as usize] && !test_dac {
            add += self.fm_out[slot] >> 5;
        }
        let sum = (acc + add).clamp(ACC_MIN, ACC_MAX);
        if op == 0 || test_dac {
            self.out.ch_out[channel] = self.out.ch_acc[channel];
        }
        self.out.ch_acc[channel] = sum;
    }
}

impl savestate::State for Output {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.ch_acc)?;
        v.field(&mut self.ch_out)?;
        v.field(&mut self.ch_lock)?;
        v.field(&mut self.ch_lock_l)?;
        v.field(&mut self.ch_lock_r)
    }
}

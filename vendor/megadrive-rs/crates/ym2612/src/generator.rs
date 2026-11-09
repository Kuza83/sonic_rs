//! Phase generator, envelope generator, LFO and FM operator stages.
//!
//! Each stage works on a different slot of the 24-slot pipeline in a given
//! cycle (the offsets below), reproducing the chip's pipelining as
//! reverse-engineered by Nuked-OPN2. Envelope levels are 10-bit attenuations
//! (0 = loudest, $3FF = silent); phases are 20-bit; operator outputs 14-bit.

use crate::tables::{
    AM_SHIFT, DETUNE, EG_STEP_HIGH, LFO_CYCLES, LFO_PM_SHIFT1, LFO_PM_SHIFT2, tables,
};
use crate::{CYCLE_INDICES, SLOTS, Ym2612};

const ENVELOPE_MAX: u16 = 0x3FF;
const PHASE_MASK: u32 = 0xF_FFFF;
/// Envelope states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EgState {
    Attack,
    Decay,
    Sustain,
    #[default]
    Release,
}

/// Test register $21 bits used by the generators.
const TEST21_LFO: u8 = 1 << 1;
const TEST21_PHASE_RESET: u8 = 1 << 3;
const TEST21_FM_INVERT: u8 = 1 << 4;
const TEST21_EG_LEVEL_ZERO: u8 = 1 << 5;
const CSM_MODE: u8 = 2;

/// Modulation sources of an operator.
#[derive(Clone, Copy)]
enum Source {
    /// Last OP1 output (and the one before, for feedback).
    Op1,
    Op1Previous,
    Op2,
    /// Output of the slot computed 12 cycles earlier in the same channel.
    PreviousSlot,
}

/// Modulation of each operator group (slot order OP1, OP3, OP2, OP4) for
/// algorithms 0-7, derived from the algorithm diagrams:
/// 0: 1→2→3→4  1: (1+2)→3→4  2: (1+(2→3))→4  3: ((1→2)+3)→4
/// 4: (1→2)+(3→4)  5: 1→2, 1→3, 1→4  6: (1→2)+3+4  7: 1+2+3+4.
/// OP1 always feeds back on itself (scaled by FB).
fn modulation(group: usize, algorithm: u8) -> &'static [Source] {
    use Source::*;
    match (group, algorithm) {
        (0, _) => &[Op1, Op1Previous],
        (1, 0 | 2) => &[Op2],              // OP3 ← OP2
        (1, 1) => &[Op1, Op2],             // OP3 ← OP1 + OP2
        (1, 5) => &[Op1],                  // OP3 ← OP1
        (2, 0 | 3..=6) => &[PreviousSlot], // OP2 ← OP1
        (3, 0 | 1 | 4) => &[PreviousSlot], // OP4 ← OP3
        (3, 2) => &[Op1, PreviousSlot],    // OP4 ← OP1 + OP3
        (3, 3) => &[Op2, PreviousSlot],    // OP4 ← OP2 + OP3
        (3, 5) => &[Op1],                  // OP4 ← OP1
        _ => &[],
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Generator {
    // Phase.
    pg_phase: [u32; SLOTS],
    pg_inc: [u32; SLOTS],
    pg_reset: [bool; SLOTS],
    pg_fnum: u16,
    pg_block: u8,
    pg_kcode: u8,
    // Envelope.
    eg_level: [u16; SLOTS],
    eg_state: [EgState; SLOTS],
    eg_kon: [bool; SLOTS],
    pub eg_kon_latch: [bool; SLOTS],
    pub eg_kon_csm: [bool; SLOTS],
    eg_out: [u16; SLOTS],
    eg_ssg_dir: [bool; SLOTS],
    eg_ssg_inv: [bool; SLOTS],
    eg_ssg_enable: [bool; SLOTS],
    eg_ssg_reset_latch: [bool; SLOTS],
    eg_ssg_repeat_latch: [bool; SLOTS],
    eg_ssg_hold_latch: [bool; SLOTS],
    eg_cycle: u32,
    eg_cycle_stop: bool,
    eg_shift: u32,
    eg_shift_lock: u32,
    eg_timer: u32,
    eg_timer_inc: u32,
    eg_quotient: u32,
    eg_timer_low_lock: u32,
    eg_rate: u8,
    eg_ksv: u8,
    eg_inc: u8,
    eg_ratemax: bool,
    eg_sl: [u8; 2],
    eg_tl: [u8; 2],
    eg_lfo_am: u16,
    // LFO.
    lfo_cnt: u8,
    lfo_quotient: u32,
    lfo_inc: u32,
    lfo_pm: u8,
    lfo_am: u16,
    // FM.
    fm_mod: [i16; SLOTS],
    fm_op1: [[i16; 2]; 6],
    fm_op2: [i16; 6],
}

impl Default for Generator {
    fn default() -> Self {
        Self {
            pg_phase: [0; SLOTS],
            pg_inc: [0; SLOTS],
            pg_reset: [false; SLOTS],
            pg_fnum: 0,
            pg_block: 0,
            pg_kcode: 0,
            eg_level: [ENVELOPE_MAX; SLOTS],
            eg_state: [EgState::Release; SLOTS],
            eg_kon: [false; SLOTS],
            eg_kon_latch: [false; SLOTS],
            eg_kon_csm: [false; SLOTS],
            eg_out: [ENVELOPE_MAX; SLOTS],
            eg_ssg_dir: [false; SLOTS],
            eg_ssg_inv: [false; SLOTS],
            eg_ssg_enable: [false; SLOTS],
            eg_ssg_reset_latch: [false; SLOTS],
            eg_ssg_repeat_latch: [false; SLOTS],
            eg_ssg_hold_latch: [false; SLOTS],
            eg_cycle: 0,
            eg_cycle_stop: false,
            eg_shift: 0,
            eg_shift_lock: 0,
            eg_timer: 0,
            eg_timer_inc: 0,
            eg_quotient: 0,
            eg_timer_low_lock: 0,
            eg_rate: 0,
            eg_ksv: 0,
            eg_inc: 0,
            eg_ratemax: false,
            eg_sl: [0; 2],
            eg_tl: [0; 2],
            eg_lfo_am: 0,
            lfo_cnt: 0,
            lfo_quotient: 0,
            lfo_inc: 0,
            lfo_pm: 0,
            lfo_am: 0,
            fm_mod: [0; SLOTS],
            fm_op1: [[0; 2]; 6],
            fm_op2: [0; 6],
        }
    }
}

fn sign_extend_14(v: u16) -> i16 {
    ((v << 2) as i16) >> 2
}

impl Ym2612 {
    /// Start-of-cycle bookkeeping: envelope timer and LFO sampling.
    pub(crate) fn clock_generator_start(&mut self) {
        let g = &mut self.generator;
        g.lfo_inc = (self.test21 & TEST21_LFO != 0) as u32;
        g.eg_cycle += 1;
        if self.cycle == 1 && g.eg_quotient == 2 {
            g.eg_shift_lock = if g.eg_cycle_stop { 0 } else { g.eg_shift + 1 };
            g.eg_timer_low_lock = g.eg_timer & 3;
        }
        match self.cycle {
            0 => {
                g.lfo_pm = g.lfo_cnt >> 2;
                let am = if g.lfo_cnt & 0x40 != 0 {
                    g.lfo_cnt & 0x3F
                } else {
                    g.lfo_cnt ^ 0x3F
                };
                g.lfo_am = (am as u16) << 1;
            }
            1 => {
                g.eg_quotient = (g.eg_quotient + 1) % 3;
                g.eg_cycle = 0;
                g.eg_cycle_stop = true;
                g.eg_shift = 0;
                g.eg_timer_inc |= g.eg_quotient >> 1;
                g.eg_timer += g.eg_timer_inc;
                g.eg_timer_inc = g.eg_timer >> 12;
                g.eg_timer &= 0xFFF;
            }
            13 => {
                g.eg_cycle = 0;
                g.eg_cycle_stop = true;
                g.eg_shift = 0;
                g.eg_timer += g.eg_timer_inc;
                g.eg_timer_inc = g.eg_timer >> 12;
                g.eg_timer &= 0xFFF;
            }
            23 => g.lfo_inc |= 1,
            _ => {}
        }
        if self.test21 & TEST21_EG_LEVEL_ZERO != 0 {
            g.eg_timer &= !(1 << g.eg_cycle);
        }
        if (g.eg_timer >> g.eg_cycle) & 1 != 0 && g.eg_cycle_stop {
            g.eg_shift = g.eg_cycle;
            g.eg_cycle_stop = false;
        }
    }

    /// Key-on latches per slot (and CSM key-on of channel 3).
    pub(crate) fn latch_key_on(&mut self) {
        let slot = self.cycle;
        self.generator.eg_kon_latch[slot] = self.mode_kon[slot];
        self.generator.eg_kon_csm[slot] = false;
        if self.channel() == 2 && self.kon_csm {
            self.generator.eg_kon_latch[slot] = true;
            self.generator.eg_kon_csm[slot] = true;
        }
    }

    /// Modulation input of the operator 6 slots ahead.
    pub(crate) fn fm_prepare(&mut self) {
        let slot = CYCLE_INDICES[self.cycle].plus6;
        let channel = self.channel();
        let group = slot / 6;
        let algorithm = self.ch.connect[channel];
        let prev = CYCLE_INDICES[self.cycle].plus18;
        let g = &mut self.generator;
        let mut sum: i32 = 0;
        for source in modulation(group, algorithm) {
            sum += match source {
                Source::Op1 => g.fm_op1[channel][0],
                Source::Op1Previous => g.fm_op1[channel][1],
                Source::Op2 => g.fm_op2[channel],
                Source::PreviousSlot => self.fm_out[prev],
            } as i32;
        }
        let modulation = if group == 0 {
            let fb = self.ch.fb[channel];
            if fb == 0 { 0 } else { sum >> (10 - fb) }
        } else {
            sum >> 1
        };
        g.fm_mod[slot] = modulation as i16;

        // Remember OP1 (two values, for feedback) and OP2 outputs.
        let done = CYCLE_INDICES[self.cycle].plus18;
        match done / 6 {
            0 => {
                g.fm_op1[channel][1] = g.fm_op1[channel][0];
                g.fm_op1[channel][0] = self.fm_out[done];
            }
            2 => g.fm_op2[channel] = self.fm_out[done],
            _ => {}
        }
    }

    /// Operator output: log-sine of the phase plus the attenuation, then
    /// exponent, sign from the phase (14-bit result).
    pub(crate) fn fm_generate(&mut self) {
        let slot = CYCLE_INDICES[self.cycle].plus19;
        let g = &self.generator;
        let phase = ((g.fm_mod[slot] as i32 + (g.pg_phase[slot] >> 10) as i32) & 0x3FF) as u16;
        let quarter = if phase & 0x100 != 0 {
            (phase ^ 0xFF) & 0xFF
        } else {
            phase & 0xFF
        };
        let t = tables();
        let level = (t.log_sin[quarter as usize] + (g.eg_out[slot] << 2)).min(0x1FFF);
        // Shifts of up to 31 bits must yield 0 (done on 32 bits).
        let magnitude = ((((t.exp[((level & 0xFF) ^ 0xFF) as usize] | 0x400) as u32) << 2)
            >> (level >> 8)) as u16;
        let invert = ((self.test21 & TEST21_FM_INVERT != 0) as u16) << 13;
        let raw = if phase & 0x200 != 0 {
            (!magnitude ^ invert).wrapping_add(1)
        } else {
            magnitude ^ invert
        };
        self.fm_out[slot] = sign_extend_14(raw & 0x3FFF);
    }

    pub(crate) fn phase_generate(&mut self) {
        let g = &mut self.generator;
        let slot = CYCLE_INDICES[self.cycle].plus20;
        if g.pg_reset[slot] {
            g.pg_inc[slot] = 0;
        }
        let slot = CYCLE_INDICES[self.cycle].plus19;
        if g.pg_reset[slot] || self.test21 & TEST21_PHASE_RESET != 0 {
            g.pg_phase[slot] = 0;
        }
        g.pg_phase[slot] = (g.pg_phase[slot] + g.pg_inc[slot]) & PHASE_MASK;
    }

    /// Phase increment of the current slot: F-number with LFO PM, block,
    /// detune, multiplier.
    pub(crate) fn phase_increment(&mut self) {
        let chan = self.channel();
        let slot = self.cycle;
        let g = &mut self.generator;
        let mut fnum = (g.pg_fnum as u32) << 1;
        let fnum_h = (g.pg_fnum >> 4) as u32;
        let mut lfo_l = g.lfo_pm & 0x0F;
        if lfo_l & 0x08 != 0 {
            lfo_l ^= 0x0F;
        }
        let pms = self.ch.pms[chan] as usize;
        let mut fm = (fnum_h >> LFO_PM_SHIFT1[pms][lfo_l as usize])
            + (fnum_h >> LFO_PM_SHIFT2[pms][lfo_l as usize]);
        if pms > 5 {
            fm <<= pms - 5;
        }
        fm >>= 2;
        fnum = if g.lfo_pm & 0x10 != 0 {
            fnum.wrapping_sub(fm)
        } else {
            fnum + fm
        } & 0xFFF;
        let mut base = (fnum << g.pg_block) >> 2;

        let dt = self.op.dt[slot];
        let dt_l = dt & 3;
        let mut detune = 0;
        if dt_l != 0 {
            let kcode = g.pg_kcode.min(0x1C);
            let block = kcode >> 2;
            let note = kcode & 3;
            let sum = block + 9 + ((dt_l == 3) as u8 | (dt_l & 2));
            let (sum_h, sum_l) = (sum >> 1, sum & 1);
            detune = DETUNE[((sum_l << 2) | note) as usize] >> (9 - sum_h);
        }
        base = if dt & 4 != 0 {
            base.wrapping_sub(detune)
        } else {
            base + detune
        } & 0x1FFFF;
        g.pg_inc[slot] = ((base * self.op.multi[slot] as u32) >> 1) & PHASE_MASK;
    }

    /// SSG-EG: inversion, repeat and hold of the envelope.
    pub(crate) fn envelope_ssg(&mut self) {
        let slot = self.cycle;
        let ssg = self.op.ssg_eg[slot];
        let g = &mut self.generator;
        g.eg_ssg_reset_latch[slot] = false;
        g.eg_ssg_repeat_latch[slot] = false;
        g.eg_ssg_hold_latch[slot] = false;
        let mut direction = false;
        if ssg & 0x08 != 0 {
            direction = g.eg_ssg_dir[slot];
            if g.eg_level[slot] & 0x200 != 0 {
                if ssg & 3 == 0 {
                    g.eg_ssg_reset_latch[slot] = true;
                }
                if ssg & 1 == 0 {
                    g.eg_ssg_repeat_latch[slot] = true;
                }
                if ssg & 3 == 2 {
                    direction = !direction;
                }
                if ssg & 3 == 3 {
                    direction = true;
                }
            }
            if g.eg_kon_latch[slot] && (ssg & 7 == 5 || ssg & 7 == 3) {
                g.eg_ssg_hold_latch[slot] = true;
            }
            direction &= g.eg_kon[slot];
        }
        g.eg_ssg_dir[slot] = direction;
        g.eg_ssg_enable[slot] = ssg & 0x08 != 0;
        let attack_invert = ssg & 0x04 != 0 && ssg & 0x08 != 0;
        g.eg_ssg_inv[slot] = (direction ^ attack_invert) && g.eg_kon[slot];
    }

    /// Envelope state machine and level update (slot `cycle - 2`).
    pub(crate) fn envelope_adsr(&mut self) {
        let slot = CYCLE_INDICES[self.cycle].plus22;
        let g = &mut self.generator;
        let new_kon = g.eg_kon_latch[slot];
        let old_kon = g.eg_kon[slot];
        g.pg_reset[slot] = (new_kon && !old_kon) || g.eg_ssg_reset_latch[slot];
        let key_on = (new_kon && !old_kon) || (old_kon && g.eg_ssg_repeat_latch[slot]);
        let key_off = old_kon && !new_kon;

        let mut level = g.eg_level[slot] as i32;
        let mut ssg_level = level;
        if g.eg_ssg_inv[slot] {
            ssg_level = (512 - level) & 0x3FF;
        }
        if key_off {
            level = ssg_level;
        }
        let off = if g.eg_ssg_enable[slot] {
            level >> 9 != 0
        } else {
            level & 0x3F0 == 0x3F0
        };
        let mut next_level = level;
        let mut next_state = g.eg_state[slot];
        let mut inc: i32 = 0;
        let eg_inc = g.eg_inc as i32;
        let attack_step = |level: i32| ((!level) << eg_inc) >> 5;
        if key_on {
            next_state = EgState::Attack;
            if g.eg_ratemax {
                next_level = 0;
            } else if g.eg_state[slot] == EgState::Attack && level != 0 && eg_inc != 0 && new_kon {
                inc = attack_step(level);
            }
        } else {
            match g.eg_state[slot] {
                EgState::Attack => {
                    if level == 0 {
                        next_state = EgState::Decay;
                    } else if eg_inc != 0 && !g.eg_ratemax && new_kon {
                        inc = attack_step(level);
                    }
                }
                EgState::Decay => {
                    if level >> 4 == (g.eg_sl[1] as i32) << 1 {
                        next_state = EgState::Sustain;
                    } else if !off && eg_inc != 0 {
                        inc = 1 << (eg_inc - 1);
                        if g.eg_ssg_enable[slot] {
                            inc <<= 2;
                        }
                    }
                }
                EgState::Sustain | EgState::Release => {
                    if !off && eg_inc != 0 {
                        inc = 1 << (eg_inc - 1);
                        if g.eg_ssg_enable[slot] {
                            inc <<= 2;
                        }
                    }
                }
            }
            if !new_kon {
                next_state = EgState::Release;
            }
        }
        if g.eg_kon_csm[slot] {
            next_level |= (g.eg_tl[1] as i32) << 3;
        }
        if !key_on && !g.eg_ssg_hold_latch[slot] && g.eg_state[slot] != EgState::Attack && off {
            next_state = EgState::Release;
            next_level = ENVELOPE_MAX as i32;
        }
        next_level += inc;
        g.eg_kon[slot] = g.eg_kon_latch[slot];
        g.eg_level[slot] = (next_level & 0x3FF) as u16;
        g.eg_state[slot] = next_state;
    }

    /// Envelope output with SSG inversion, AM and total level (slot `cycle - 1`).
    pub(crate) fn envelope_output(&mut self) {
        let slot = CYCLE_INDICES[self.cycle].plus23;
        // In CSM mode the total level of channel 3 is applied at key-on instead.
        let apply_tl = !(self.mode_ch3 == CSM_MODE && self.channel() == 3);
        let g = &mut self.generator;
        let mut level = g.eg_level[slot];
        if g.eg_ssg_inv[slot] {
            level = 512u16.wrapping_sub(level);
        }
        if self.test21 & TEST21_EG_LEVEL_ZERO != 0 {
            level = 0;
        }
        level &= 0x3FF;
        level += g.eg_lfo_am;
        if apply_tl {
            level += (g.eg_tl[0] as u16) << 3;
        }
        g.eg_out[slot] = level.min(ENVELOPE_MAX);
    }

    /// Rate, key scale and increment for the next envelope update.
    pub(crate) fn envelope_prepare(&mut self) {
        let slot = self.cycle;
        let chan = self.channel();
        let g = &mut self.generator;
        let rate = ((g.eg_rate << 1) + g.eg_ksv).min(0x3F);
        let sum = ((rate as u32 >> 2) + g.eg_shift_lock) & 0x0F;
        let mut inc = 0;
        if g.eg_rate != 0 && g.eg_quotient == 2 {
            if rate < 48 {
                inc = match sum {
                    12 => 1,
                    13 => (rate >> 1) & 1,
                    14 => rate & 1,
                    _ => 0,
                };
            } else {
                inc = (EG_STEP_HIGH[(rate & 3) as usize][g.eg_timer_low_lock as usize]
                    + (rate >> 2))
                    .saturating_sub(11)
                    .min(4);
            }
        }
        g.eg_inc = inc;
        g.eg_ratemax = rate >> 1 == 0x1F;

        let mut selected = g.eg_state[slot];
        if (g.eg_kon[slot] && g.eg_ssg_repeat_latch[slot])
            || (!g.eg_kon[slot] && g.eg_kon_latch[slot])
        {
            selected = EgState::Attack;
        }
        g.eg_rate = match selected {
            EgState::Attack => self.op.ar[slot],
            EgState::Decay => self.op.dr[slot],
            EgState::Sustain => self.op.sr[slot],
            EgState::Release => (self.op.rr[slot] << 1) | 1,
        };
        g.eg_ksv = g.pg_kcode >> (self.op.ks[slot] ^ 3);
        g.eg_lfo_am = if self.op.am[slot] {
            g.lfo_am >> AM_SHIFT[self.ch.ams[chan] as usize]
        } else {
            0
        };
        g.eg_tl[1] = g.eg_tl[0];
        g.eg_tl[0] = self.op.tl[slot];
        g.eg_sl[1] = g.eg_sl[0];
        g.eg_sl[0] = self.op.sl[slot];
    }

    /// F-number, block and key code for the next cycle's slot (channel 3
    /// special mode uses per-operator frequencies).
    pub(crate) fn prepare_frequency(&mut self) {
        let next = (self.channel() + 1) % 6;
        let special = self.mode_ch3 != 0;
        let index = match self.cycle {
            1 if special => Some(1),  // OP1
            7 if special => Some(0),  // OP3
            13 if special => Some(2), // OP2
            _ => None,
        };
        let ch = &self.ch;
        let g = &mut self.generator;
        (g.pg_fnum, g.pg_block, g.pg_kcode) = match index {
            Some(i) => (ch.fnum_3ch[i], ch.block_3ch[i], ch.kcode_3ch[i]),
            None => (ch.fnum[next], ch.block[next], ch.kcode[next]),
        };
    }

    pub(crate) fn update_lfo(&mut self) {
        let g = &mut self.generator;
        let period = LFO_CYCLES[self.lfo_freq as usize];
        if g.lfo_quotient & period == period {
            g.lfo_quotient = 0;
            g.lfo_cnt = g.lfo_cnt.wrapping_add(1);
        } else {
            g.lfo_quotient += g.lfo_inc;
        }
        if !self.lfo_enable {
            g.lfo_cnt = 0;
        } else {
            g.lfo_cnt &= 0x7F;
        }
    }
}

impl savestate::State for Generator {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.pg_phase)?;
        v.field(&mut self.pg_inc)?;
        v.field(&mut self.pg_reset)?;
        v.field(&mut self.pg_fnum)?;
        v.field(&mut self.pg_block)?;
        v.field(&mut self.pg_kcode)?;
        v.field(&mut self.eg_level)?;
        let mut codes = self.eg_state.map(|s| s as u8);
        v.field(&mut codes)?;
        for (state, code) in self.eg_state.iter_mut().zip(codes) {
            *state = match code {
                0 => EgState::Attack,
                1 => EgState::Decay,
                2 => EgState::Sustain,
                3 => EgState::Release,
                _ => return Err(savestate::StateError::InvalidValue("envelope state")),
            };
        }
        for slots in [
            &mut self.eg_kon,
            &mut self.eg_kon_latch,
            &mut self.eg_kon_csm,
            &mut self.eg_ssg_dir,
            &mut self.eg_ssg_inv,
            &mut self.eg_ssg_enable,
            &mut self.eg_ssg_reset_latch,
            &mut self.eg_ssg_repeat_latch,
            &mut self.eg_ssg_hold_latch,
        ] {
            v.field(slots)?;
        }
        v.field(&mut self.eg_out)?;
        for word in [
            &mut self.eg_cycle,
            &mut self.eg_shift,
            &mut self.eg_shift_lock,
            &mut self.eg_timer,
            &mut self.eg_timer_inc,
            &mut self.eg_quotient,
            &mut self.eg_timer_low_lock,
        ] {
            v.field(word)?;
        }
        v.field(&mut self.eg_cycle_stop)?;
        v.field(&mut self.eg_rate)?;
        v.field(&mut self.eg_ksv)?;
        v.field(&mut self.eg_inc)?;
        v.field(&mut self.eg_ratemax)?;
        v.field(&mut self.eg_sl)?;
        v.field(&mut self.eg_tl)?;
        v.field(&mut self.eg_lfo_am)?;
        v.field(&mut self.lfo_cnt)?;
        v.field(&mut self.lfo_quotient)?;
        v.field(&mut self.lfo_inc)?;
        v.field(&mut self.lfo_pm)?;
        v.field(&mut self.lfo_am)?;
        v.field(&mut self.fm_mod)?;
        v.field(&mut self.fm_op1)?;
        v.field(&mut self.fm_op2)
    }
}

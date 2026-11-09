//! CPU interface: write detection, busy flag and register decoding.
//!
//! A CPU write is latched and recognised on the next internal cycle; a data
//! write keeps the chip busy for 32 cycles. Operator and channel registers
//! are applied only on the cycle of the slot/channel they address (the chip
//! updates its multiplexed register files as the slots go by).

use crate::Ym2612;

/// Address decoder: register address bits 8, 2-0 for each slot pair
/// (OP1/OP2 for slots 0-5, OP3/OP4 for 6-11) and for each channel.
const OP_OFFSET: [u16; 12] = [
    0x000, 0x001, 0x002, 0x100, 0x101, 0x102, 0x004, 0x005, 0x006, 0x104, 0x105, 0x106,
];
const CH_OFFSET: [u16; 6] = [0x000, 0x001, 0x002, 0x100, 0x101, 0x102];
/// Note code from F-number bits 10-7 (key scaling).
pub(crate) const FN_NOTE: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 3, 3, 3, 3, 3, 3];
/// Data writes keep the busy flag for 32 cycles.
const BUSY_COUNT_BITS: u32 = 5;
/// Bit 8 of a latched value: part II (ports 2/3).
const PART2: u16 = 0x100;

#[derive(Debug, Clone, Default)]
pub(crate) struct Io {
    /// Value latched by the last CPU write (bit 8 = part II).
    write_data: u16,
    /// Shift registers of the address/data write strobes.
    write_a: u8,
    write_d: u8,
    write_a_en: bool,
    write_d_en: bool,
    write_busy: bool,
    write_busy_count: u32,
    pub busy: bool,
    /// Last FM register address and whether data may follow it.
    write_fm_address: bool,
    write_fm_data: bool,
    address: u16,
    data: u8,
    mode_address: u16,
}

impl Io {
    pub fn cpu_write(&mut self, port: u8, data: u8) {
        let port = port & 3;
        self.write_data = (((port as u16) << 7) & PART2) | data as u16;
        if port & 1 != 0 {
            self.write_d |= 1;
        } else {
            self.write_a |= 1;
        }
    }

    /// Start of a cycle: recognise strobes, update busy.
    pub fn detect_writes(&mut self) {
        self.write_a_en = self.write_a & 3 == 1;
        self.write_d_en = self.write_d & 3 == 1;
        self.write_a <<= 1;
        self.write_d <<= 1;
        self.busy = self.write_busy;
        self.write_busy_count += self.write_busy as u32;
        self.write_busy =
            (self.write_busy && self.write_busy_count >> BUSY_COUNT_BITS == 0) || self.write_d_en;
        self.write_busy_count &= (1 << BUSY_COUNT_BITS) - 1;
    }
}

impl Ym2612 {
    /// End of a cycle: slot/channel registers for this cycle, then the
    /// effects of a write recognised this cycle.
    pub(crate) fn apply_register_writes(&mut self) {
        if self.io.write_fm_data {
            self.write_slot_registers();
            self.write_channel_registers();
        }
        let (a_en, d_en) = (self.io.write_a_en, self.io.write_d_en);
        if a_en || d_en {
            if a_en {
                self.io.write_fm_data = false;
            }
            if self.io.write_fm_address && d_en {
                self.io.write_fm_data = true;
            }
            if a_en {
                // $00-$0F would address the (absent) SSG part.
                self.io.write_fm_address = self.io.write_data & 0xF0 != 0;
                if self.io.write_fm_address {
                    self.io.address = self.io.write_data;
                }
            }
            if d_en && self.io.write_data & PART2 == 0 {
                self.write_mode_register(self.io.mode_address, self.io.write_data as u8);
            }
            if a_en {
                self.io.mode_address = self.io.write_data & 0x1FF;
            }
        }
        if self.io.write_fm_data {
            self.io.data = self.io.write_data as u8;
        }
    }

    fn write_slot_registers(&mut self) {
        let mut slot = self.cycle % 12;
        let address = self.io.address;
        if OP_OFFSET[slot] != address & 0x107 {
            return;
        }
        if address & 0x08 != 0 {
            slot += 12; // OP2, OP4
        }
        let d = self.io.data;
        let op = &mut self.op;
        match address & 0xF0 {
            0x30 => {
                op.multi[slot] = if d & 0x0F == 0 { 1 } else { (d & 0x0F) << 1 };
                op.dt[slot] = (d >> 4) & 7;
            }
            0x40 => op.tl[slot] = d & 0x7F,
            0x50 => {
                op.ar[slot] = d & 0x1F;
                op.ks[slot] = (d >> 6) & 3;
            }
            0x60 => {
                op.dr[slot] = d & 0x1F;
                op.am[slot] = d & 0x80 != 0;
            }
            0x70 => op.sr[slot] = d & 0x1F,
            0x80 => {
                op.rr[slot] = d & 0x0F;
                let sl = (d >> 4) & 0x0F;
                // SL 15 means the bottom of the envelope (extended to 31).
                op.sl[slot] = sl | ((sl + 1) & 0x10);
            }
            0x90 => op.ssg_eg[slot] = d & 0x0F,
            _ => {}
        }
    }

    fn write_channel_registers(&mut self) {
        let channel = self.channel();
        let address = self.io.address;
        if CH_OFFSET[channel] != address & 0x103 {
            return;
        }
        let d = self.io.data;
        let ch = &mut self.ch;
        match address & 0xFC {
            0xA0 => {
                ch.fnum[channel] = d as u16 | ((ch.reg_a4 as u16 & 7) << 8);
                ch.block[channel] = (ch.reg_a4 >> 3) & 7;
                ch.kcode[channel] =
                    (ch.block[channel] << 2) | FN_NOTE[(ch.fnum[channel] >> 7) as usize];
            }
            0xA4 => ch.reg_a4 = d,
            0xA8 => {
                // Only channels 1-3 of part I address the channel 3 operators.
                if channel < 3 {
                    ch.fnum_3ch[channel] = d as u16 | ((ch.reg_ac as u16 & 7) << 8);
                    ch.block_3ch[channel] = (ch.reg_ac >> 3) & 7;
                    ch.kcode_3ch[channel] = (ch.block_3ch[channel] << 2)
                        | FN_NOTE[(ch.fnum_3ch[channel] >> 7) as usize];
                }
            }
            0xAC => ch.reg_ac = d,
            0xB0 => {
                ch.connect[channel] = d & 7;
                ch.fb[channel] = (d >> 3) & 7;
            }
            0xB4 => {
                ch.pms[channel] = d & 7;
                ch.ams[channel] = (d >> 4) & 3;
                ch.pan_l[channel] = d & 0x80 != 0;
                ch.pan_r[channel] = d & 0x40 != 0;
            }
            _ => {}
        }
    }

    /// Global registers $21-$2C (part I only).
    fn write_mode_register(&mut self, address: u16, d: u8) {
        match address {
            0x21 => self.test21 = d,
            0x22 => {
                self.lfo_enable = d & 0x08 != 0;
                self.lfo_freq = d & 7;
            }
            0x24 => self.timers.a_reg = (self.timers.a_reg & 0x003) | ((d as u16) << 2),
            0x25 => self.timers.a_reg = (self.timers.a_reg & 0x3FC) | (d as u16 & 3),
            0x26 => self.timers.b_reg = d,
            0x27 => {
                self.mode_ch3 = (d >> 6) & 3;
                self.timers.a_load = d & 0x01 != 0;
                self.timers.b_load = d & 0x02 != 0;
                self.timers.a_enable = d & 0x04 != 0;
                self.timers.b_enable = d & 0x08 != 0;
                self.timers.a_reset = d & 0x10 != 0;
                self.timers.b_reset = d & 0x20 != 0;
            }
            0x28 => {
                for (i, k) in self.kon_operators.iter_mut().enumerate() {
                    *k = d >> (4 + i) & 1 != 0;
                }
                self.kon_channel = if d & 3 == 3 {
                    0xFF // invalid channel: never matches
                } else {
                    (d & 3) + ((d >> 2) & 1) * 3
                };
            }
            0x2A => self.dac_data = (self.dac_data & 1) | (((d ^ 0x80) as u16) << 1),
            0x2B => self.dac_enable = d & 0x80 != 0,
            0x2C => {
                self.test2c = d;
                self.dac_data = (self.dac_data & 0x1FE) | ((d >> 3) & 1) as u16;
            }
            _ => {}
        }
    }
}

impl savestate::State for Io {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.write_data)?;
        v.field(&mut self.write_a)?;
        v.field(&mut self.write_d)?;
        v.field(&mut self.write_a_en)?;
        v.field(&mut self.write_d_en)?;
        v.field(&mut self.write_busy)?;
        v.field(&mut self.write_busy_count)?;
        v.field(&mut self.busy)?;
        v.field(&mut self.write_fm_address)?;
        v.field(&mut self.write_fm_data)?;
        v.field(&mut self.address)?;
        v.field(&mut self.data)?;
        v.field(&mut self.mode_address)
    }
}

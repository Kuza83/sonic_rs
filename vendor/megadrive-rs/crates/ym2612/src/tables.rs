//! ROM tables of the YM2612, generated from their defining formulas (the
//! values match the die ROM dumps exactly; checked once against the
//! Nuked-OPN2 oracle sources, and continuously through the oracle tests):
//! - log-sine: quarter wave, `-log2(sin((2i + 1)·π/1024)) × 256`;
//! - exponent: `(2^(i/256) − 1) × 1024`.

use std::sync::OnceLock;

pub(crate) struct Tables {
    pub log_sin: [u16; 256],
    pub exp: [u16; 256],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

pub(crate) fn tables() -> &'static Tables {
    TABLES.get_or_init(|| Tables {
        log_sin: std::array::from_fn(|i| {
            let x = (2 * i + 1) as f64 * std::f64::consts::PI / 1024.0;
            (-x.sin().log2() * 256.0).round() as u16
        }),
        exp: std::array::from_fn(|i| ((2f64.powf(i as f64 / 256.0) - 1.0) * 1024.0).round() as u16),
    })
}

/// Detune base steps (Nemesis' YM2612 notes; same values as the chip's table).
pub(crate) const DETUNE: [u32; 8] = [16, 17, 19, 20, 22, 24, 27, 29];

/// LFO period in sample-ticks for each frequency setting ($22 bits 2-0).
pub(crate) const LFO_CYCLES: [u32; 8] = [108, 77, 71, 67, 62, 44, 8, 5];

/// Phase modulation: two right shifts of fnum bits 10-4 per PMS and LFO
/// step, whose sum gives the frequency offset (7 = no contribution).
pub(crate) const LFO_PM_SHIFT1: [[u8; 8]; 8] = [
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 7, 1, 1],
    [7, 7, 7, 7, 1, 1, 1, 1],
    [7, 7, 7, 1, 1, 1, 1, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
];
pub(crate) const LFO_PM_SHIFT2: [[u8; 8]; 8] = [
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 2, 2, 2, 2],
    [7, 7, 7, 2, 2, 2, 7, 7],
    [7, 7, 2, 2, 7, 7, 2, 2],
    [7, 7, 2, 7, 7, 7, 2, 7],
    [7, 7, 7, 2, 7, 7, 2, 1],
    [7, 7, 7, 2, 7, 7, 2, 1],
    [7, 7, 7, 2, 7, 7, 2, 1],
];

/// AM depth: right shift of the LFO amplitude for AMS 0-3.
pub(crate) const AM_SHIFT: [u8; 4] = [7, 3, 1, 0];

/// Envelope increments of the fastest rates (≥ 48), by rate & 3 and the low
/// bits of the envelope timer.
pub(crate) const EG_STEP_HIGH: [[u8; 4]; 4] =
    [[0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 1, 0], [1, 1, 1, 0]];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_rom_values() {
        let t = tables();
        assert_eq!(t.log_sin[0], 0x859);
        assert_eq!(t.log_sin[255], 0x000);
        assert_eq!(t.exp[0], 0x000);
        assert_eq!(t.exp[255], 0x3FA);
    }
}

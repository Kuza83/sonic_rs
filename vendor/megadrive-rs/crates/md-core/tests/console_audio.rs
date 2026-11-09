//! End-to-end audio: the 68000 programs the PSG; the frequency of the
//! output buffer is measured (zero crossings), never listened to.

use md_core::audio::OUTPUT_RATE;
use md_core::testrom::{PSG_TONE_REGISTER, psg_tone};
use md_core::{Cartridge, Console, MdConfig};

/// PSG input clock (NTSC master / 15) and the tone frequency it gives.
const PSG_CLOCK: f64 = 53_693_175.0 / 15.0;

#[test]
fn psg_tone_has_the_programmed_frequency() {
    let mut c = Console::new(
        Cartridge::from_bytes(&psg_tone()).unwrap(),
        MdConfig::default(),
    );
    c.run_frame(); // set-up
    let mut left = Vec::new();
    for _ in 0..60 {
        c.run_frame();
        left.extend(c.audio_samples().iter().step_by(2).copied());
    }
    let seconds = left.len() as f64 / OUTPUT_RATE as f64;
    assert!(
        (seconds - 1.0).abs() < 0.01,
        "60 NTSC frames ≈ 1 s ({seconds})"
    );
    let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    let measured = crossings as f64 / 2.0 / seconds;
    let expected = PSG_CLOCK / (32.0 * PSG_TONE_REGISTER as f64);
    assert!(
        (measured - expected).abs() / expected < 0.01,
        "measured {measured:.1} Hz, expected {expected:.1} Hz"
    );
    // One PSG channel at full volume after the mix gain (32767 / 16 ≈ 2047),
    // plus the YM2612's idle DAC offset times the YM mix gain (72 × 11).
    let peak = left.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert_eq!(peak, 2047 + 72 * 11, "PSG level through the mixer");
}

#[test]
fn audio_buffer_size_matches_the_frame_duration() {
    let mut c = Console::new(
        Cartridge::from_bytes(&psg_tone()).unwrap(),
        MdConfig::default(),
    );
    c.run_frame();
    // NTSC frame: 262 × 3420 master cycles → ≈ 735.7 samples.
    let frames = c.audio_samples().len() / 2;
    assert!((735..=736).contains(&frames), "{frames} samples");
}

/// Master clock / (7 × 144): the YM2612 sample rate.
const YM_RATE: f64 = 53_693_175.0 / 1008.0;

/// The Z80 drives the YM2612 (busy polling included); the output carries the
/// programmed frequency.
#[test]
fn ym2612_tone_through_the_z80_has_the_programmed_frequency() {
    use md_core::testrom::{YM_TONE_BLOCK, YM_TONE_FNUM, ym_tone};
    let mut c = Console::new(
        Cartridge::from_bytes(&ym_tone()).unwrap(),
        MdConfig::default(),
    );
    for _ in 0..3 {
        c.run_frame(); // load the driver, write the registers
    }
    let mut left = Vec::new();
    for _ in 0..60 {
        c.run_frame();
        left.extend(c.audio_samples().iter().step_by(2).map(|&s| s as f64));
    }
    let mean = left.iter().sum::<f64>() / left.len() as f64;
    let seconds = left.len() as f64 / OUTPUT_RATE as f64;
    let crossings = left
        .windows(2)
        .filter(|w| (w[0] < mean) != (w[1] < mean))
        .count();
    let measured = crossings as f64 / 2.0 / seconds;
    let expected =
        YM_RATE * YM_TONE_FNUM as f64 * 2f64.powi(YM_TONE_BLOCK as i32 - 1) / (1u32 << 20) as f64;
    assert!(
        (measured - expected).abs() / expected < 0.01,
        "measured {measured:.1} Hz, expected {expected:.1} Hz"
    );
    // One channel, one operator at TL 0: the standalone chip fed the same
    // registers swings from -717 to 837 per native sample (one channel is
    // about 1/6 of the summed range); × 11 mix gain (GPGX sound.c). Box-filter
    // averaging keeps the extremes of a 440 Hz tone.
    let max = left.iter().cloned().fold(f64::MIN, f64::max);
    let min = left.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        (max - min - 11.0 * (837.0 + 717.0)).abs() < 100.0,
        "YM2612 level through the mixer: {min}..{max}"
    );
}

/// FNV-1a over the 16-bit samples (little-endian bytes).
fn audio_hash(samples: &[i16]) -> u64 {
    samples
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// Non-regression of the whole audio path (Z80 driver, YM2612, PSG idle,
/// resampling, mix). Pinned after the frequency and level checks above;
/// re-pinned when the YM mix gain went from 4 to 11 (GPGX level, A3).
#[test]
fn ym2612_tone_audio_hash() {
    let mut c = Console::new(
        Cartridge::from_bytes(&md_core::testrom::ym_tone()).unwrap(),
        MdConfig::default(),
    );
    let mut all = Vec::new();
    for _ in 0..10 {
        c.run_frame();
        all.extend_from_slice(c.audio_samples());
    }
    // 10 NTSC frames at 44.1 kHz: 7359 stereo pairs.
    assert_eq!(all.len(), 2 * 7359);
    assert_eq!(audio_hash(&all), 0x5e63_4213_c035_87fd);
}

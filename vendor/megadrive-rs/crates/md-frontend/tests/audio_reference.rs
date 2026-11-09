//! Whole-machine audio compared with Genesis Plus GX (scripts/gpgx_ref.py
//! --wav, Nuked YM2612, no filter) through robust measurements: pitch,
//! onset, envelope shape, balance (see audio_analysis).

mod audio_analysis;

use std::path::{Path, PathBuf};
use std::process::Command;

use audio_analysis::{Wav, ac_rms, balance, dominant_frequency, dominant_frequency_wide};

const BIN: &str = env!("CARGO_BIN_EXE_md-frontend");
const CORE: &str = "/usr/lib/x86_64-linux-gnu/libretro/genesis_plus_gx_libretro.so";

fn tmp_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn repo_file(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn generated_rom(name: &str) -> PathBuf {
    let path = tmp_file(&format!("audio_{name}.bin"));
    let ok = Command::new(BIN)
        .args(["--write-test-rom", name])
        .arg(&path)
        .status()
        .unwrap();
    assert!(ok.success());
    path
}

/// Audio of `frames` frames of `rom` (with `presses` script lines) from this
/// emulator and from GPGX, or `None` when the reference cannot be made.
fn both(rom: &Path, frames: u64, presses: &[&str], tag: &str) -> Option<(Wav, Wav)> {
    if !Path::new(CORE).exists() || !rom.exists() {
        eprintln!(
            "skipped: {CORE} or {} missing (see docs/decisions.md)",
            rom.display()
        );
        return None;
    }
    let script: Vec<String> = presses
        .iter()
        .flat_map(|p| ["--press".into(), p.to_string()])
        .collect();
    let (ours, gpgx) = (
        tmp_file(&format!("{tag}_ours.wav")),
        tmp_file(&format!("{tag}_gpgx.wav")),
    );
    let ok = Command::new(BIN)
        .arg(rom)
        .args(["--dump-audio", &frames.to_string(), "--wav"])
        .arg(&ours)
        .args(&script)
        .status()
        .unwrap();
    assert!(ok.success());
    let status = Command::new("python3")
        .arg("-I")
        .arg(repo_file("scripts/gpgx_ref.py"))
        .arg(rom)
        .arg(frames.to_string())
        .arg("--wav")
        .arg(&gpgx)
        .args(&script)
        .status();
    match status {
        Ok(s) => assert!(s.success(), "gpgx_ref.py failed"),
        Err(e) => {
            eprintln!("skipped: python3 unavailable ({e})");
            return None;
        }
    }
    Some((Wav::read(&ours), Wav::read(&gpgx)))
}

/// Measurements of one recording, over 1 s of steady tone starting
/// `start_s` seconds in. Absolute onsets are not compared: GPGX's output
/// starts about 15 ms later than ours on the same program (startup and
/// buffering), the same for PSG and YM2612.
#[derive(Debug)]
struct Measures {
    pitch: f64,
    level: f64,
    balance: f64,
}

fn measure(wav: &Wav, start_s: f64, lo: f64, hi: f64) -> Measures {
    let mono = wav.mono();
    let steady = Wav::slice(&mono, wav.rate, start_s, 1.0);
    Measures {
        pitch: dominant_frequency(steady, wav.rate, lo, hi),
        level: ac_rms(steady),
        balance: balance(wav),
    }
}

/// Same pitch (0.3 %), same balance, same level without DC (5 %).
fn assert_close(tag: &str, ours: &Measures, gpgx: &Measures) {
    eprintln!("{tag}: ours {ours:?}, GPGX {gpgx:?}");
    assert!(
        (ours.pitch - gpgx.pitch).abs() / gpgx.pitch < 0.003,
        "{tag}: pitch"
    );
    assert!((ours.balance - gpgx.balance).abs() < 0.01, "{tag}: balance");
    let ratio = ours.level / gpgx.level;
    assert!(
        (0.95..=1.05).contains(&ratio),
        "{tag}: level ratio {ratio:.3}"
    );
}

/// PSG channel 0 at full volume: checks the PSG mix level.
#[test]
fn psg_tone_matches_gpgx() {
    let Some((ours, gpgx)) = both(&generated_rom("psg_tone"), 120, &[], "psg_tone") else {
        return;
    };
    let (o, g) = (
        measure(&ours, 0.5, 400.0, 480.0),
        measure(&gpgx, 0.5, 400.0, 480.0),
    );
    assert_close("psg_tone", &o, &g);
}

/// One YM2612 operator driven by the Z80: checks the FM mix level (× 11).
#[test]
fn ym_tone_matches_gpgx() {
    let Some((ours, gpgx)) = both(&generated_rom("ym_tone"), 120, &[], "ym_tone") else {
        return;
    };
    let (o, g) = (
        measure(&ours, 0.5, 400.0, 480.0),
        measure(&gpgx, 0.5, 400.0, 480.0),
    );
    assert_close("ym_tone", &o, &g);
}

const SUITE_240P: &str = "roms/test/240pSuite-1.32.bin";
/// NTSC frame rate (53693175 / (3420 × 262)).
const NTSC_FPS: f64 = 59.922_743;

/// 240p Test Suite > Audio tests > Sound Test, entered at frame 400; the
/// `extra` script lines then select and play a sound (tests.c, SoundTest:
/// A plays the selection; DOWN moves FM → octave → pan → PSG; LEFT/RIGHT
/// move the selection). Returns the measurements of both emulators over
/// 0.3 s starting 10 frames after `play_frame`.
fn sound_test(extra: &[&str], play_frame: u64, tag: &str) -> Option<(Measures, Measures)> {
    let mut lines = vec!["300 down", "310 down", "320 a", "400 a"];
    lines.extend_from_slice(extra);
    let (ours, gpgx) = both(&repo_file(SUITE_240P), play_frame + 40, &lines, tag)?;
    let start = (play_frame + 10) as f64 / NTSC_FPS;
    let m = |wav: &Wav| {
        let mono = wav.mono();
        let window = |x: &[f64]| Wav::slice(x, wav.rate, start, 0.3).to_vec();
        let steady = window(&mono);
        Measures {
            pitch: dominant_frequency_wide(&steady, wav.rate, 50.0, 5000.0),
            level: ac_rms(&steady),
            balance: audio_analysis::balance_of(&window(&wav.left), &window(&wav.right)),
        }
    };
    Some((m(&ours), m(&gpgx)))
}

/// FM note, centred.
#[test]
fn suite_240p_fm_center() {
    if let Some((o, g)) = sound_test(&["480 a"], 480, "240p_fm_center") {
        assert_close("240p_fm_center", &o, &g);
    }
}

/// FM note panned left, then right (DOWN DOWN, LEFT or RIGHT, A, UP UP, A).
#[test]
fn suite_240p_fm_left_right() {
    let left = [
        "480 down", "490 down", "500 left", "510 a", "520 up", "530 up", "540 a",
    ];
    if let Some((o, g)) = sound_test(&left, 540, "240p_fm_left") {
        assert_close("240p_fm_left", &o, &g);
        assert!(o.balance > 0.99, "left only: {}", o.balance);
    }
    let right = [
        "480 down",
        "490 down",
        "500 right",
        "510 a",
        "520 up",
        "530 up",
        "540 a",
    ];
    if let Some((o, g)) = sound_test(&right, 540, "240p_fm_right") {
        assert_close("240p_fm_right", &o, &g);
        assert!(o.balance < 0.01, "right only: {}", o.balance);
    }
}

/// PSG channels 0 (200 Hz) and 1 (2000 Hz) (DOWN × 3, then A / RIGHT A).
#[test]
fn suite_240p_psg_channels() {
    let ch0 = ["480 down", "490 down", "500 down", "510 a"];
    if let Some((o, g)) = sound_test(&ch0, 510, "240p_psg0") {
        assert_close("240p_psg0", &o, &g);
    }
    let ch1 = ["480 down", "490 down", "500 down", "510 right", "520 a"];
    if let Some((o, g)) = sound_test(&ch1, 520, "240p_psg1") {
        assert_close("240p_psg1", &o, &g);
    }
}

/// MDFourier (240p Test Suite > Audio tests > MDFourier, mdfourier.c):
/// started by A at `MDF_START`; after a 20-frame sync pulse train and 20
/// frames of silence, `ExecuteFM(20)` plays octaves 0, 8, ... 56 (block 0-7)
/// of 12 notes, each on a left channel and a right channel at once, held 20
/// frames with key off at frame 16; then `ExecutePSG(20)` plays 500, 1000,
/// ... 20000 (SGDK PSG_setFrequency) for 20 frames each.
const MDF_PRESSES: [&str; 7] = [
    "300 down", "310 down", "320 a", "380 down", "390 down", "400 a", "480 a",
];
const MDF_START: u64 = 480;
const MDF_NOTE_FRAMES: u64 = 20;
const MDF_FM_NOTES: u64 = 96;
const MDF_PSG_TONES: u64 = 40;
/// Frames from the pulse train start to the first FM note.
const MDF_FM_OFFSET: u64 = 40;

/// One measured segment of the MDFourier sequence.
#[derive(Debug, Clone, Copy)]
struct Segment {
    pitch_left: f64,
    pitch_right: f64,
    level_left: f64,
    level_right: f64,
}

/// Measures `count` segments of `MDF_NOTE_FRAMES` starting `offset` frames
/// after the pulse train, frames 3 to 14 of each (before the FM key off);
/// `range(i)` is the pitch search range of segment `i`.
fn mdf_segments(
    wav: &Wav,
    offset: u64,
    count: u64,
    range: impl Fn(u64) -> (f64, f64),
) -> Vec<Segment> {
    let after = (MDF_START as f64 - 5.0) / NTSC_FPS;
    let sync =
        audio_analysis::pulse_train_start(&wav.mono(), wav.rate, after).expect("pulse train");
    (0..count)
        .map(|i| {
            let start = sync + (offset + i * MDF_NOTE_FRAMES + 3) as f64 / NTSC_FPS;
            let len = 12.0 / NTSC_FPS;
            let (lo, hi) = range(i);
            let l = Wav::slice(&wav.left, wav.rate, start, len);
            let r = Wav::slice(&wav.right, wav.rate, start, len);
            Segment {
                pitch_left: dominant_frequency_wide(l, wav.rate, lo, hi),
                pitch_right: dominant_frequency_wide(r, wav.rate, lo, hi),
                level_left: ac_rms(l),
                level_right: ac_rms(r),
            }
        })
        .collect()
}

/// F-numbers of the 12 notes (mdfourier.c PITCHES), played at blocks 0-7.
const MDF_FNUMS: [u32; 12] = [277, 293, 311, 329, 349, 369, 391, 415, 439, 465, 493, 522];
/// YM2612 sample rate (master / 1008) and the phase accumulator span.
const YM_RATE: f64 = 53_693_175.0 / 1008.0;

/// Fundamental of FM note `i`: F × 2^(block − 1) × rate / 2^20.
fn fm_fundamental(i: u64) -> f64 {
    let block = (i / 12) as i32;
    MDF_FNUMS[(i % 12) as usize] as f64 * 2f64.powi(block - 1) * YM_RATE / (1u64 << 20) as f64
}

/// Narrow search around the known fundamental (harmonics are left out).
fn fm_range(i: u64) -> (f64, f64) {
    let f = fm_fundamental(i);
    (f * 0.97, f * 1.03)
}

/// Pitch and level are compared from this fundamental up: below, 12 frames
/// (0.2 s) hold too few periods, and GPGX's output high-pass (blip_buf
/// `bass_shift = 9`: about 44100 / (2π × 512) ≈ 13.7 Hz) attenuates the
/// fundamental.
const MIN_PITCH_HZ: f64 = 50.0;
/// Levels are compared up to block 4: above, the piano's upper harmonics
/// meet GPGX's band-limited resampler (blip_buf), which attenuates them
/// while our box filter does not (docs/decisions.md, M2).
const LEVEL_MAX_NOTE: u64 = 60;

/// Compares the FM notes `0..notes` of MDFourier in both emulators.
fn mdfourier_fm(notes: u64, tag: &str) -> Option<(Wav, Wav)> {
    let frames = MDF_START + MDF_FM_OFFSET + notes * MDF_NOTE_FRAMES + 60;
    mdfourier_fm_in(
        both(&repo_file(SUITE_240P), frames, &MDF_PRESSES, tag)?,
        notes,
        tag,
    )
}

fn mdfourier_fm_in((ours, gpgx): (Wav, Wav), notes: u64, tag: &str) -> Option<(Wav, Wav)> {
    let o = mdf_segments(&ours, MDF_FM_OFFSET, notes, fm_range);
    let g = mdf_segments(&gpgx, MDF_FM_OFFSET, notes, fm_range);
    let mut failures = Vec::new();
    for (i, (o, g)) in o.iter().zip(&g).enumerate() {
        let i = i as u64;
        let f0 = fm_fundamental(i);
        let ratio = (o.level_left + o.level_right) / (g.level_left + g.level_right);
        eprintln!(
            "FM note {i:2} (f0 {f0:8.2} Hz): pitch L {:9.3} / {:9.3}, R {:9.3} / {:9.3}, level ratio {ratio:.3}",
            o.pitch_left, g.pitch_left, o.pitch_right, g.pitch_right
        );
        // Pitch on the right channel (grand piano, strong fundamental); the
        // left instrument's weak fundamental blurs its peak in both.
        if f0 >= MIN_PITCH_HZ && (o.pitch_right - g.pitch_right).abs() / g.pitch_right > 0.003 {
            failures.push(format!(
                "note {i}: pitch {:.3} vs {:.3}",
                o.pitch_right, g.pitch_right
            ));
        }
        if f0 >= MIN_PITCH_HZ && i < LEVEL_MAX_NOTE && !(0.95..=1.05).contains(&ratio) {
            failures.push(format!("note {i}: level ratio {ratio:.3}"));
        }
    }
    assert!(failures.is_empty(), "{tag}: {failures:#?}");
    Some((ours, gpgx))
}

/// SGDK PSG_setFrequency(f): tone register 3579545 / (32 f), so the tone
/// plays at 3579545 / (32 × register) Hz.
fn psg_actual(f: f64) -> f64 {
    const PSG_CLOCK: f64 = 53_693_175.0 / 15.0;
    PSG_CLOCK / (32.0 * (PSG_CLOCK / (32.0 * f)).floor().max(1.0))
}

/// The first four octaves of MDFourier's FM notes (kept short for the gate;
/// notes 36-47 are the measurable ones).
#[test]
fn mdfourier_fm_first_octaves() {
    mdfourier_fm(48, "mdf_fm48");
}

/// All of MDFourier's FM notes, then its PSG tones (500 to 20000 Hz): about
/// 2 minutes in a debug build, so it is run on demand (`cargo test -p
/// md-frontend --test audio_reference -- --ignored`); its results are
/// recorded in docs/decisions.md (M2).
#[test]
#[ignore = "long: run with --ignored (docs/decisions.md, M2)"]
fn mdfourier_full_sequence() {
    let frames = MDF_START + MDF_FM_OFFSET + (MDF_FM_NOTES + MDF_PSG_TONES) * MDF_NOTE_FRAMES + 60;
    let Some(pair) = both(&repo_file(SUITE_240P), frames, &MDF_PRESSES, "mdf_full") else {
        return;
    };
    let (ours, gpgx) = mdfourier_fm_in(pair, MDF_FM_NOTES, "mdf_full").unwrap();
    let offset = MDF_FM_OFFSET + MDF_FM_NOTES * MDF_NOTE_FRAMES;
    let range = |i: u64| {
        let f = psg_actual(500.0 * (i + 1) as f64);
        (f * 0.97, (f * 1.03).min(22_000.0))
    };
    let o = mdf_segments(&ours, offset, MDF_PSG_TONES, range);
    let g = mdf_segments(&gpgx, offset, MDF_PSG_TONES, range);
    let mut failures = Vec::new();
    for (i, (o, g)) in o.iter().zip(&g).enumerate() {
        let f = psg_actual(500.0 * (i + 1) as f64);
        let ratio = o.level_left / g.level_left;
        eprintln!(
            "PSG tone {i:2} ({f:8.1} Hz): pitch {:9.2} / {:9.2}, level ratio {ratio:.3}",
            o.pitch_left, g.pitch_left
        );
        if f <= 10_000.0 && (o.pitch_left - g.pitch_left).abs() / g.pitch_left > 0.003 {
            failures.push(format!(
                "tone {i}: pitch {} vs {}",
                o.pitch_left, g.pitch_left
            ));
        }
        if f <= 2_000.0 && !(0.95..=1.05).contains(&ratio) {
            failures.push(format!("tone {i}: level ratio {ratio:.3}"));
        }
    }
    assert!(failures.is_empty(), "PSG: {failures:#?}");
}

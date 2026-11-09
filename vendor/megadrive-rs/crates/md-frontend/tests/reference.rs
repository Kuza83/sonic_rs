//! J4.4: frames compared with Genesis Plus GX, the reference emulator.
//!
//! The reference frame is made on the fly by `scripts/gpgx_ref.py` (the GPGX
//! libretro core driven through the libretro API, same input script and
//! frame numbering). A test is skipped with a message when Python, the core
//! or a non-generated ROM is missing; see `docs/decisions.md` (J4.4).
//!
//! Emulators expand the VDP's 3-bit colour levels to 8 bits differently
//! (GPGX through RGB565: 0, 33, 66... 239; this emulator: n * 255 / 7), so
//! frames are equal when the reference colours map one-to-one onto ours at
//! every pixel, each pair within `LEVEL_TOLERANCE`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_md-frontend");
const CORE: &str = "/usr/lib/x86_64-linux-gnu/libretro/genesis_plus_gx_libretro.so";
/// Largest per-channel gap between matching colours (GPGX 239 vs 255).
const LEVEL_TOLERANCE: u8 = 20;

fn repo_file(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn tmp_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// RGB pixels and size of a PNG file (RGB or RGBA, 8 bits).
fn load_png(path: &Path) -> (u32, u32, Vec<[u8; 3]>) {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    assert!(channels >= 3, "{}: not an RGB image", path.display());
    let pixels = buf[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    (info.width, info.height, pixels)
}

/// Pixels whose colour breaks the one-to-one mapping or is too far from the
/// reference, as (x, y).
fn mismatches(width: u32, reference: &[[u8; 3]], ours: &[[u8; 3]]) -> Vec<(u32, u32)> {
    let (mut forward, mut backward) = (HashMap::new(), HashMap::new());
    let mut bad = Vec::new();
    for (i, (r, o)) in reference.iter().zip(ours).enumerate() {
        let ok_forward = *forward.entry(*r).or_insert(*o) == *o;
        let ok_backward = *backward.entry(*o).or_insert(*r) == *r;
        let close = r
            .iter()
            .zip(o)
            .all(|(a, b)| a.abs_diff(*b) <= LEVEL_TOLERANCE);
        if !(ok_forward && ok_backward && close) {
            bad.push((i as u32 % width, i as u32 / width));
        }
    }
    bad
}

/// Writes the generated test ROM `name` and returns its path.
fn generated_rom(name: &str) -> PathBuf {
    let path = tmp_file(&format!("ref_{name}.bin"));
    let status = Command::new(BIN)
        .args(["--write-test-rom", name])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

/// Runs `rom` for `frame` frames with the `presses` script lines, in this
/// emulator and in GPGX, and compares the two pictures.
fn compare(rom: &Path, frame: u64, presses: &[&str], tag: &str) {
    compare_in(rom, frame, presses, tag, false);
}

/// `compare` with both emulators forced to a PAL (Europe) console: ours
/// with `--region eu`, GPGX with its `genesis_plus_gx_region_detect=pal`
/// core option (libretro_core_options.h).
fn compare_pal(rom: &Path, frame: u64, presses: &[&str], tag: &str) {
    compare_in(rom, frame, presses, tag, true);
}

fn compare_in(rom: &Path, frame: u64, presses: &[&str], tag: &str, pal: bool) {
    let tool = repo_file("scripts/gpgx_ref.py");
    if !Path::new(CORE).exists() || !rom.exists() {
        eprintln!(
            "skipped: {CORE} or {} missing (see docs/decisions.md, J4.4)",
            rom.display()
        );
        return;
    }
    let script: Vec<String> = presses
        .iter()
        .flat_map(|p| ["--press".into(), p.to_string()])
        .collect();
    let (ours, reference) = (
        tmp_file(&format!("{tag}_ours.png")),
        tmp_file(&format!("{tag}_gpgx.png")),
    );
    let (our_region, gpgx_region): (&[&str], &[&str]) = if pal {
        (
            &["--region", "eu"],
            &["--option", "genesis_plus_gx_region_detect=pal"],
        )
    } else {
        (&[], &[])
    };
    let status = Command::new(BIN)
        .arg(rom)
        .args(["--dump-frame", &frame.to_string(), "--out"])
        .arg(&ours)
        .args(&script)
        .args(our_region)
        .status()
        .unwrap();
    assert!(status.success());
    let status = match Command::new("python3")
        .arg("-I")
        .arg(&tool)
        .arg(rom)
        .arg(frame.to_string())
        .arg("--out")
        .arg(&reference)
        .args(&script)
        .args(gpgx_region)
        .status()
    {
        Ok(status) => status,
        Err(e) => {
            eprintln!("skipped: python3 unavailable ({e})");
            return;
        }
    };
    assert!(status.success(), "gpgx_ref.py failed");

    let (rw, rh, reference_pixels) = load_png(&reference);
    let (ow, oh, our_pixels) = load_png(&ours);
    assert_eq!((ow, oh), (rw, rh), "{tag}: frame size");
    let bad = mismatches(rw, &reference_pixels, &our_pixels);
    assert!(
        bad.is_empty(),
        "{tag}: {} pixels differ from GPGX ({} vs {}), first at {:?}",
        bad.len(),
        reference.display(),
        ours.display(),
        &bad[..bad.len().min(8)]
    );
}

/// 240p Test Suite 1.32 (Artemio Urbina, GPL): main menu.
#[test]
fn suite_240p_main_menu() {
    compare(
        &repo_file("roms/test/240pSuite-1.32.bin"),
        300,
        &[],
        "240p_menu",
    );
}

/// Every button of a 3-button pad, pressed on frame 3 and shown on frame 4:
/// checks the button mapping and the frame alignment of the two emulators.
#[test]
fn pad_display_buttons() {
    let rom = generated_rom("pad_display");
    compare(&rom, 4, &[], "pad_idle");
    compare(&rom, 3, &["3 a"], "pad_not_yet");
    compare(&rom, 5, &["3 a"], "pad_released");
    for button in ["up", "down", "left", "right", "a", "b", "c", "start"] {
        compare(&rom, 4, &[&format!("3 {button}")], &format!("pad_{button}"));
    }
    compare(&rom, 4, &["3 up+a", "3 start"], "pad_combo");
}

/// Generated pictures, equal from frame 3: GPGX's first frame starts in
/// VBlank and its VDP accesses take FIFO time, so its picture is complete one
/// frame later than ours (docs/decisions.md, E3).
#[test]
fn generated_pictures() {
    for name in ["checkerboard", "checkerboard_dma", "showcase", "interrupts"] {
        let rom = generated_rom(name);
        for frame in [3, 10] {
            compare(&rom, frame, &[], &format!("{name}_{frame}"));
        }
    }
}

#[test]
fn mapping_must_be_one_to_one_and_close() {
    let (a, b, c) = ([0, 0, 0], [33, 33, 33], [36, 36, 36]);
    assert!(
        mismatches(2, &[a, b], &[a, c]).is_empty(),
        "levels may differ"
    );
    assert_eq!(
        mismatches(2, &[b, a], &[c, c]),
        [(1, 0)],
        "two colours merged"
    );
    assert_eq!(
        mismatches(2, &[a, a], &[a, c]),
        [(1, 0)],
        "one colour split"
    );
    let blue = [0, 0, 73];
    assert_eq!(
        mismatches(1, &[a], &[blue]),
        [(0, 0)],
        "one colour each, far apart"
    );
}

const SUITE_240P: &str = "roms/test/240pSuite-1.32.bin";

/// Script lines of a 240p Test Suite menu walk: from the main menu (ready
/// at frame 300), `down` presses 10 frames apart then `a`, per level.
/// Returns the lines and the frame of the last press.
fn menu_walk(levels: &[usize]) -> (Vec<String>, u64) {
    let mut lines = Vec::new();
    let mut frame = 300;
    for (i, &downs) in levels.iter().enumerate() {
        if i > 0 {
            frame += 60; // the menu is drawn
        }
        for _ in 0..downs {
            lines.push(format!("{frame} down"));
            frame += 10;
        }
        lines.push(format!("{frame} a"));
    }
    (lines, frame)
}

/// One 240p Test Suite screen, `after` frames past the last press.
fn suite_240p_screen(levels: &[usize], extra: &[&str], after: u64, tag: &str) {
    let (mut lines, last) = menu_walk(levels);
    lines.extend(
        extra
            .iter()
            .map(|l| l.replace("+", &(last + 60).to_string())),
    );
    let presses: Vec<&str> = lines.iter().map(String::as_str).collect();
    compare(&repo_file(SUITE_240P), last + after, &presses, tag);
}

/// Test Patterns > Color & Black Levels > PLUGE (its help page closed with
/// Start: `+` is replaced by the frame 60 after the last menu press).
#[test]
fn suite_240p_pluge() {
    suite_240p_screen(&[0, 0, 0], &["+ start"], 120, "240p_pluge");
}

/// Test Patterns > Color & Black Levels > SMPTE Color Bars.
#[test]
fn suite_240p_smpte_bars() {
    suite_240p_screen(&[0, 0, 3], &[], 90, "240p_smpte");
}

/// Test Patterns > Color & Black Levels > Sharpness.
#[test]
fn suite_240p_sharpness() {
    suite_240p_screen(&[0, 0, 9], &[], 90, "240p_sharpness");
}

/// Video tests > Striped Sprite Test (sprites over a background).
#[test]
fn suite_240p_striped_sprite() {
    suite_240p_screen(&[1, 1], &[], 240, "240p_striped_sprite");
}

// Video tests > Drop Shadow Test is left out: with some menu timings our
// picture keeps a sprite GPGX hides, most likely the VDP's internal sprite
// table cache, not emulated yet (docs/decisions.md, G2).

/// Video tests > Lag Test: a frame counter started when the screen is
/// ready (it was 3 frames ahead with instant DMA fills, 1 without FIFO waits).
#[test]
fn suite_240p_lag_counter() {
    suite_240p_screen(&[1, 2], &[], 240, "240p_lag");
}

/// Video tests > Timing & Reflex Test, in rhythmic mode (DOWN): the default
/// random mode depends on exact timing.
#[test]
fn suite_240p_timing_rhythmic() {
    suite_240p_screen(&[1, 3], &["+ down"], 240, "240p_timing");
}

/// Video tests > Alternate 240p/480i (frame counter).
#[test]
fn suite_240p_alternate_240p_480i() {
    suite_240p_screen(&[1, 10], &[], 240, "240p_alternate");
}

/// Video tests > Disappearing Logo (frame counter).
#[test]
fn suite_240p_disappearing_logo() {
    suite_240p_screen(&[1, 11], &[], 240, "240p_logo");
}

/// Video tests > Scroll Test (plane scrolling, animated).
#[test]
fn suite_240p_scroll() {
    suite_240p_screen(&[1, 4], &[], 240, "240p_scroll");
}

/// Video tests > Grid Scroll Test.
#[test]
fn suite_240p_grid_scroll() {
    suite_240p_screen(&[1, 5], &[], 240, "240p_grid_scroll");
}

/// PAL console: the 240p Test Suite switches to V30 (320 × 240, "PAL VDP
/// 320x240p"), so these check the 240-line picture, VBLANK at line 240 and
/// the 50 Hz frame timing seen by the program.
fn suite_240p_pal_screen(levels: &[usize], after: u64, tag: &str) {
    let (lines, last) = menu_walk(levels);
    let presses: Vec<&str> = lines.iter().map(String::as_str).collect();
    compare_pal(&repo_file(SUITE_240P), last + after, &presses, tag);
}

#[test]
fn suite_240p_pal_main_menu() {
    compare_pal(&repo_file(SUITE_240P), 300, &[], "240p_pal_menu");
    compare_pal(&repo_file(SUITE_240P), 301, &[], "240p_pal_menu_next");
}

/// Test Patterns > Color & Black Levels > SMPTE Color Bars, in V30.
#[test]
fn suite_240p_pal_smpte_bars() {
    suite_240p_pal_screen(&[0, 0, 3], 90, "240p_pal_smpte");
}

/// Video tests > Lag Test in PAL: its frame counter checks the V30 VBLANK
/// and the 50 Hz timing.
#[test]
fn suite_240p_pal_lag_counter() {
    suite_240p_pal_screen(&[1, 2], 240, "240p_pal_lag");
}

/// Video tests > Grid Scroll Test in PAL (240-line scrolling).
#[test]
fn suite_240p_pal_grid_scroll() {
    suite_240p_pal_screen(&[1, 5], 240, "240p_pal_grid_scroll");
}

/// Generated V30 picture on a PAL console (runs without the 240p ROM).
#[test]
fn v30_checkerboard_pal() {
    compare_pal(
        &generated_rom("v30_checkerboard"),
        3,
        &[],
        "v30_checkerboard",
    );
}

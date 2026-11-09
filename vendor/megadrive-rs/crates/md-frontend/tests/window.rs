//! The window loop run without a screen (SDL dummy video and audio drivers).
#![cfg(feature = "sdl")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_md-frontend");

/// A home folder of its own for the tests: the Library window would
/// otherwise scan the user's ~/roms and read their settings.
fn test_home() -> PathBuf {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("home");
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn run(args: &[&str]) -> String {
    let home = test_home();
    let out = Command::new(BIN)
        .args(args)
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .output()
        .expect("frontend runs");
    assert!(
        out.status.success(),
        "md-frontend {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Writes the generated ROM `name` to a file of its own: the tests run in
/// parallel, and a file shared between them could be rewritten (truncated)
/// by one while another reads it.
fn test_rom(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{n}.bin"));
    run(&["--write-test-rom", name, path.to_str().unwrap()]);
    path
}

/// The frame shown by the window loop is the headless one.
#[test]
fn window_loop_frame_matches_the_headless_dump() {
    let rom = test_rom("showcase");
    let rom = rom.to_str().unwrap();
    let headless = run(&[rom, "--dump-frame", "2", "--hash"]);
    let window = run(&[rom, "--screenshot-at", "2", "--hash", "--scale", "1"]);
    assert_eq!(headless.trim().len(), 16, "a 64-bit hash: {headless}");
    assert_eq!(window.trim(), headless.trim());
}

/// With audio, the queue paces the loop: 60 frames take about one second
/// (the first frames fill the queue without waiting).
#[test]
fn audio_queue_paces_the_loop() {
    let rom = test_rom("ym_tone");
    let start = Instant::now();
    run(&[rom.to_str().unwrap(), "--max-frames", "60", "--scale", "1"]);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(800) && elapsed < Duration::from_secs(10),
        "60 frames in {elapsed:?}"
    );
}

/// Muted, the clock paces the loop the same way.
#[test]
fn clock_paces_the_muted_loop() {
    let rom = test_rom("checkerboard");
    let start = Instant::now();
    run(&[
        rom.to_str().unwrap(),
        "--max-frames",
        "60",
        "--mute",
        "--scale",
        "1",
    ]);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(10),
        "60 frames in {elapsed:?}"
    );
}

/// SRAM is kept in `<rom>.srm` across runs: the boot counter goes 0, 1.
#[test]
fn sram_file_persists_between_runs() {
    let rom = test_rom("sram_counter");
    let srm = rom.with_extension("srm");
    let _ = std::fs::remove_file(&srm);
    let counter = || std::fs::read(&srm).expect(".srm written on quit")[1];
    let args = [
        rom.to_str().unwrap(),
        "--max-frames",
        "2",
        "--mute",
        "--scale",
        "1",
    ];
    run(&args);
    assert_eq!(counter(), 0x00, "first boot: $FF + 1");
    run(&args);
    assert_eq!(counter(), 0x01, "second boot read the saved value");
}

/// A cheat given on the command line changes the frame (headless dump).
#[test]
fn cheat_option_changes_the_picture() {
    let rom = test_rom("showcase");
    let rom = rom.to_str().unwrap();
    let plain = run(&[rom, "--dump-frame", "2", "--hash"]);
    // `move.w #$000E,(a0)` at $24E loads CRAM colour 2 (red): its
    // immediate at $250 patched to $0EEE turns the red band white.
    let cheated = run(&[rom, "--dump-frame", "2", "--hash", "--cheat", "000250:0EEE"]);
    assert_ne!(plain.trim(), cheated.trim());
    let bad = Command::new(BIN)
        .args([rom, "--dump-frame", "1", "--hash", "--cheat", "NOPE"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "invalid code rejected");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("NOPE"));
}

/// `--write-test-rom` creates missing directories.
#[test]
fn write_test_rom_creates_directories() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("new_dir/nested");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("showcase.bin");
    run(&["--write-test-rom", "showcase", path.to_str().unwrap()]);
    assert!(path.exists());
}

/// Copy of `rom` stored in a zip archive (deflated), next to it.
fn zipped(rom: &Path) -> PathBuf {
    use std::io::Write;
    let path = rom.with_extension("zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("readme.txt", options).unwrap();
    zip.write_all(b"homebrew test ROM").unwrap();
    zip.start_file(rom.file_name().unwrap().to_str().unwrap(), options)
        .unwrap();
    zip.write_all(&std::fs::read(rom).unwrap()).unwrap();
    zip.finish().unwrap();
    path
}

/// A zipped ROM runs exactly like the plain image.
#[test]
fn zipped_rom_matches_the_plain_image() {
    let rom = test_rom("showcase");
    let archive = zipped(&rom);
    let plain = run(&[rom.to_str().unwrap(), "--dump-frame", "2", "--hash"]);
    let unpacked = run(&[archive.to_str().unwrap(), "--dump-frame", "2", "--hash"]);
    assert_eq!(unpacked, plain);
}

/// The SRAM of a zipped ROM is kept next to the archive, under its name.
#[test]
fn zipped_rom_sram_is_named_after_the_archive() {
    let rom = Path::new(env!("CARGO_TARGET_TMPDIR")).join("zipped/sram_counter.bin");
    run(&["--write-test-rom", "sram_counter", rom.to_str().unwrap()]);
    let archive = zipped(&rom);
    std::fs::remove_file(&rom).unwrap();
    let srm = archive.with_extension("srm");
    let _ = std::fs::remove_file(&srm);
    let args = [
        archive.to_str().unwrap(),
        "--max-frames",
        "2",
        "--mute",
        "--scale",
        "1",
    ];
    run(&args);
    run(&args);
    assert_eq!(std::fs::read(&srm).expect(".srm written")[1], 0x01);
}

/// Scripted presses reach the game on the right frame, the same way with and
/// without a window: `pad_display` reads the pad during frame n and shows it
/// from frame n + 1.
#[test]
fn input_script_drives_the_pad() {
    let rom = test_rom("pad_display");
    let rom = rom.to_str().unwrap();
    let hash = |frames: &str, extra: &[&str]| {
        let mut args = vec![rom, "--dump-frame", frames, "--hash"];
        args.extend_from_slice(extra);
        run(&args)
    };
    let idle = hash("4", &[]);
    assert_ne!(hash("4", &["--press", "3 a"]), idle, "shown on frame 4");
    assert_eq!(
        hash("4", &["--press", "4 a"]),
        idle,
        "read on frame 4, not shown yet"
    );
    assert_eq!(
        hash("5", &["--press", "3 a"]),
        hash("5", &[]),
        "released on frame 4"
    );
    assert_ne!(
        hash("4", &["--press", "3 a"]),
        hash("4", &["--press", "3 start"]),
        "each button has its own cell"
    );

    let script = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pad.input");
    std::fs::write(&script, "# test\n2-3 up+c\n").unwrap();
    let script = script.to_str().unwrap();
    let from_file = hash("4", &["--input", script]);
    assert_eq!(from_file, hash("4", &["--press", "3 up", "--press", "3 c"]));
    let window = run(&[
        rom,
        "--screenshot-at",
        "4",
        "--hash",
        "--mute",
        "--scale",
        "1",
        "--input",
        script,
    ]);
    assert_eq!(window, from_file);

    let bad = Command::new(BIN)
        .args([rom, "--dump-frame", "1", "--hash", "--press", "3 jump"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown button 'jump'"));
}

/// The system-decorated window (`--decorated`) shows the same picture as the
/// default borderless one with our bar: captures come from the core, never
/// from the window.
#[test]
fn decorated_window_frame_matches_the_headless_dump() {
    let rom = test_rom("showcase");
    let rom = rom.to_str().unwrap();
    let headless = run(&[rom, "--dump-frame", "2", "--hash"]);
    let decorated = run(&[
        rom,
        "--screenshot-at",
        "2",
        "--hash",
        "--scale",
        "2",
        "--decorated",
    ]);
    assert_eq!(decorated.trim(), headless.trim());
}

/// The region comes from the header unless forced: a generated ROM (blank
/// country field) is USA/NTSC; `--region eu` runs a PAL console, 50 frames
/// of which last one second (887 stereo pairs per frame at 44.1 kHz).
#[test]
fn region_detected_or_forced() {
    let rom = test_rom("psg_tone");
    let rom = rom.to_str().unwrap();
    assert!(run(&[rom, "--info"]).contains("Console  : USA (NTSC 60 Hz) (detected)"));
    let samples = |region: &str| {
        let wav = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("region_{region}.wav"));
        let out = run(&[
            rom,
            "--region",
            region,
            "--dump-audio",
            "50",
            "--wav",
            wav.to_str().unwrap(),
        ]);
        // "wrote PATH (50 frames, N samples)"
        let n: u64 = out
            .rsplit(", ")
            .next()
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        n
    };
    // 50 × 44100 / 49.701 = 44364.5 pairs.
    assert!((44_360..=44_370).contains(&samples("eu")), "PAL: 49.70 fps");
    assert!(
        (36_790..=36_800).contains(&samples("us")),
        "NTSC: 59.92 fps"
    );
    let bad = Command::new(BIN)
        .args([rom, "--region", "mars", "--info"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

/// On-screen labels (fps here) are drawn over the window only: the capture
/// still equals the headless frame.
#[test]
fn on_screen_labels_stay_out_of_captures() {
    let rom = test_rom("showcase");
    let rom = rom.to_str().unwrap();
    let headless = run(&[rom, "--dump-frame", "90", "--hash"]);
    let window = run(&[
        rom,
        "--screenshot-at",
        "90",
        "--hash",
        "--scale",
        "1",
        "--mute",
        "--show-fps",
    ]);
    assert_eq!(window.trim(), headless.trim());
}

/// Without a ROM the window opens on the welcome screen (a ROM can be
/// dropped on it); --max-frames still ends it. Modes without a window need
/// a ROM.
#[test]
fn welcome_screen_without_a_rom() {
    run(&["--max-frames", "5", "--mute", "--scale", "1"]);
    let headless = Command::new(BIN)
        .args(["--dump-frame", "1", "--hash"])
        .output()
        .unwrap();
    assert!(!headless.status.success());
    assert!(String::from_utf8_lossy(&headless.stderr).contains("needs a ROM path"));
}

/// Two windows: Game and Library. The Library folder given with --library
/// is scanned (generated ROMs here) and remembered for the next runs.
#[test]
fn library_window_with_a_folder() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("library");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    run(&[
        "--write-test-rom",
        "showcase",
        dir.join("showcase.bin").to_str().unwrap(),
    ]);
    run(&[
        "--write-test-rom",
        "ym_tone",
        dir.join("sub/ym.bin").to_str().unwrap(),
    ]);
    // Own cover next to a ROM: a 2x3 PNG (no network in the tests, hence
    // --no-covers). The first game is selected, so its cover is shown.
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 2, 3);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[200; 18]).unwrap();
    }
    std::fs::write(dir.join("showcase.png"), png_bytes).unwrap();
    run(&[
        "--library",
        dir.to_str().unwrap(),
        "--no-covers",
        "--max-frames",
        "60",
        "--mute",
        "--scale",
        "1",
    ]);
    let config = std::fs::read_to_string(test_home().join(".config/megadrive-rs/config")).unwrap();
    assert!(
        config.contains(&format!("library = {}", dir.display())),
        "{config}"
    );
    // The cover grid view, remembered, starts as it was left; the own
    // cover is made into a thumbnail.
    let config_path = test_home().join(".config/megadrive-rs/config");
    std::fs::write(&config_path, format!("{config}library_view = covers\n")).unwrap();
    run(&[
        "--library",
        dir.to_str().unwrap(),
        "--no-covers",
        "--max-frames",
        "60",
        "--mute",
        "--scale",
        "1",
    ]);
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("library_view = covers"), "kept: {config}");
    // The windows' places are remembered at the close.
    assert!(config.contains("game_window = "), "{config}");
    assert!(config.contains("library_window = "), "{config}");
    // With a game too, the capture is still the core's picture.
    let rom = dir.join("showcase.bin");
    let rom = rom.to_str().unwrap();
    let headless = run(&[rom, "--dump-frame", "2", "--hash"]);
    let window = run(&[
        rom,
        "--screenshot-at",
        "2",
        "--hash",
        "--scale",
        "1",
        "--mute",
    ]);
    assert_eq!(window.trim(), headless.trim());
}

/// `--agent` is accepted; the AI's process only starts on the first F4,
/// so a window run without it never needs Python.
#[test]
fn agent_option_starts_nothing_until_asked() {
    let rom = test_rom("showcase");
    run(&[
        rom.to_str().unwrap(),
        "--agent",
        "/nonexistent/model.zip",
        "--max-frames",
        "10",
        "--mute",
        "--scale",
        "1",
    ]);
}

/// The AI playing in the window, end to end, with the stand-in server
/// (python/tests/fake_agent.py: always right+jump, no training packages
/// needed): it starts with --agent-on, decides every 4 frames, and its
/// episodes start over (the test ROM never moves Sonic's X: stuck after 300
/// steps, 1200 frames).
#[test]
fn the_ai_plays_pad_1() {
    let rom = test_rom("showcase");
    let home = test_home();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(BIN)
        .args([
            rom.to_str().unwrap(),
            "--agent",
            "ram",
            "--agent-on",
            "--max-frames",
            "1300",
            "--mute",
            "--scale",
            "1",
        ])
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("MDENV_PYTHON", "python3")
        .env(
            "MDENV_AGENT_SERVER",
            repo.join("python/tests/fake_agent.py"),
        )
        .output()
        .expect("frontend runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let summary = stderr
        .lines()
        .find(|l| l.starts_with("AI: "))
        .unwrap_or_else(|| panic!("{stderr}"));
    let numbers: Vec<u64> = summary
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|w| w.parse().ok())
        .collect();
    let (decisions, episodes) = (numbers[0], numbers[1]);
    assert!(
        decisions > 250,
        "about one decision per 4 frames: {summary}"
    );
    assert!(
        episodes >= 1,
        "stuck after 300 steps, started over: {summary}"
    );
}

//! Save states and resets.
//!
//! A state saved at frame N and loaded into a *new* console must replay the
//! following frames exactly (pictures and sound): any field missing from a
//! component's `visit` shows up here as a diverging hash.

use md_core::testrom::{SRAM_COUNTER_RAM, showcase, sram_counter, ym_tone};
use md_core::{Cartridge, Console, MdConfig, PadKind};
use savestate::StateError;

fn console(rom: &[u8]) -> Console {
    Console::new(Cartridge::from_bytes(rom).unwrap(), MdConfig::default())
}

/// Frame hashes and audio of the next `frames` frames.
fn record(c: &mut Console, frames: usize) -> (Vec<u64>, Vec<i16>) {
    let mut pictures = Vec::new();
    let mut audio = Vec::new();
    for _ in 0..frames {
        c.run_frame();
        pictures.push(c.frame_hash());
        audio.extend_from_slice(c.audio_samples());
    }
    (pictures, audio)
}

fn assert_replays(rom: &[u8], save_at: usize) {
    let mut original = console(rom);
    record(&mut original, save_at);
    let state = original.save_state();
    let expected = record(&mut original, 10);

    let mut restored = console(rom);
    restored.load_state(&state).unwrap();
    let replay = record(&mut restored, 10);
    assert_eq!(
        replay.0, expected.0,
        "pictures after loading at frame {save_at}"
    );
    assert!(
        replay.1 == expected.1,
        "audio after loading at frame {save_at}"
    );
}

#[test]
fn video_replays_identically_after_load() {
    assert_replays(&showcase(), 1);
    assert_replays(&showcase(), 5);
}

/// The Z80 driver is still writing YM2612 registers in the first frame;
/// later the tone plays: both moments must replay sample for sample.
#[test]
fn sound_replays_identically_after_load() {
    assert_replays(&ym_tone(), 1);
    assert_replays(&ym_tone(), 6);
}

#[test]
fn saving_is_stable() {
    let mut c = console(&ym_tone());
    record(&mut c, 3);
    let first = c.save_state();
    assert_eq!(c.save_state(), first, "saving twice");
    let mut other = console(&ym_tone());
    other.load_state(&first).unwrap();
    assert_eq!(other.save_state(), first, "load then save");
}

#[test]
fn bad_states_are_rejected_and_leave_the_console_untouched() {
    let mut c = console(&showcase());
    record(&mut c, 2);
    let good = c.save_state();
    let mut target = console(&showcase());
    record(&mut target, 1);
    let before = target.save_state();

    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x40;
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    let mut bad_version = good.clone();
    bad_version[7] = 99;
    let cases: [(&str, &[u8], StateError); 5] = [
        ("empty", &[], StateError::BadMagic),
        ("magic", &bad_magic, StateError::BadMagic),
        ("version", &bad_version, StateError::Version(99)),
        (
            "flipped bit",
            &flipped,
            StateError::InvalidValue("checksum"),
        ),
        (
            "truncated",
            &good[..good.len() - 1],
            StateError::InvalidValue("checksum"),
        ),
    ];
    for (what, bytes, error) in cases {
        assert_eq!(target.load_state(bytes), Err(error), "{what}");
    }
    let mut other_rom = console(&ym_tone());
    assert_eq!(other_rom.load_state(&good), Err(StateError::RomMismatch));
    assert_eq!(
        target.save_state(),
        before,
        "console unchanged by failed loads"
    );
}

/// Every possible truncation of the header is an error, never a panic.
#[test]
fn truncated_headers_are_errors() {
    let mut c = console(&showcase());
    let good = c.save_state();
    for len in 0..32 {
        assert!(c.load_state(&good[..len]).is_err(), "length {len}");
    }
}

/// Reset button: the program restarts and rebuilds its picture; SRAM keeps
/// the boot count, work RAM is not cleared.
#[test]
fn soft_reset_restarts_the_program() {
    let mut fresh = console(&showcase());
    let expected = record(&mut fresh, 2).0;
    let mut c = console(&showcase());
    record(&mut c, 3);
    c.reset_soft();
    assert_eq!(c.cpu().regs.pc, 0x200, "reset vector");
    assert_eq!(record(&mut c, 2).0, expected);

    let mut counter = console(&sram_counter());
    record(&mut counter, 1);
    counter.reset_soft();
    record(&mut counter, 1);
    assert_eq!(
        counter.ram_long(SRAM_COUNTER_RAM) >> 24,
        0x01,
        "second boot"
    );
}

#[test]
fn power_cycle_keeps_sram_and_pads() {
    let mut c = console(&sram_counter());
    c.connect_pad(1, Some(PadKind::SixButton));
    record(&mut c, 1);
    c.power_cycle();
    assert_eq!(c.frame_count(), 0);
    assert_eq!(c.bus().io().pad_kind(1), Some(PadKind::SixButton));
    record(&mut c, 1);
    assert_eq!(c.ram_long(SRAM_COUNTER_RAM) >> 24, 0x01, "SRAM kept");
}

/// Right after loading, the whole machine (every field, saved or not,
/// except per-line scratch buffers) equals the one that saved: a field left
/// out of a `visit` keeps the fresh console's value and shows up here even
/// when it would not change the next frames.
#[test]
fn loaded_machine_equals_the_saved_one() {
    for (rom, frames) in [(showcase(), 3), (ym_tone(), 2), (sram_counter(), 1)] {
        let mut original = console(&rom);
        record(&mut original, frames);
        let state = original.save_state();
        let mut restored = console(&rom);
        restored.load_state(&state).unwrap();
        let (a, b) = (format!("{original:?}"), format!("{restored:?}"));
        if a != b {
            let at = a
                .bytes()
                .zip(b.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            let context = |s: &str| s[at.saturating_sub(120)..(at + 60).min(s.len())].to_string();
            panic!(
                "machines differ after load:\n saved:  …{}\n loaded: …{}",
                context(&a),
                context(&b)
            );
        }
    }
}

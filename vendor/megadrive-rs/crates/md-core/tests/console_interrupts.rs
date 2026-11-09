//! End-to-end interrupts: a generated ROM counts V-ints and H-ints in RAM.

use md_core::testrom::{HINT_COUNTER, HINT_INTERVAL_LINES, VINT_COUNTER, interrupts};
use md_core::{Cartridge, Console, MdConfig};

fn console() -> Console {
    Console::new(
        Cartridge::from_bytes(&interrupts()).unwrap(),
        MdConfig::default(),
    )
}

#[test]
fn one_vint_per_frame() {
    let mut c = console();
    for frame in 1..=5 {
        c.run_frame();
        assert_eq!(c.ram_long(VINT_COUNTER), frame, "after frame {frame}");
    }
}

#[test]
fn hint_every_eight_lines() {
    let mut c = console();
    c.run_frame(); // set-up frame: counts are not steady yet
    let before = c.ram_long(HINT_COUNTER);
    c.run_frame();
    let per_frame = c.ram_long(HINT_COUNTER) - before;
    // Lines 7, 15, ..., 223 of the 224 active lines.
    assert_eq!(per_frame, 224 / HINT_INTERVAL_LINES);
}

#[test]
fn handlers_return_to_the_idle_loop() {
    let mut c = console();
    for _ in 0..3 {
        c.run_frame();
    }
    assert_eq!(c.cpu().state(), m68k::CpuState::Running);
    assert_eq!(c.cpu().regs.sr() & 0x0700, 0, "mask restored by RTE");
    assert_eq!(c.cpu().regs.a[7], 0x00FF_FE00, "stack balanced");
}

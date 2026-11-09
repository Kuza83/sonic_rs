//! End-to-end Z80: the 68000 loads a Z80 program into sound RAM; the Z80
//! runs it, takes its VBlank interrupt and reads the 68000 ROM through the
//! bank window. Results are read back from the shared Z80 RAM.

use md_core::testrom::{
    Z80_BANK_READ, Z80_LOOP_COUNTER, Z80_MARKER, Z80_VBLANK_COUNTER, z80_loader, z80_program,
};
use md_core::{Cartridge, Console, MdConfig};

fn console() -> Console {
    Console::new(
        Cartridge::from_bytes(&z80_loader()).unwrap(),
        MdConfig::default(),
    )
}

#[test]
fn program_is_copied_into_z80_ram() {
    let mut c = console();
    c.run_frame();
    let program = z80_program();
    assert_eq!(&c.bus().z80_space().ram()[..program.len()], &program[..]);
}

#[test]
fn z80_runs_reads_the_bank_window_and_takes_vblank_interrupts() {
    let mut c = console();
    c.run_frame();
    let ram = c.bus().z80_space().ram();
    assert_eq!(ram[Z80_BANK_READ], Z80_MARKER, "bank window read");
    let loops = ram[Z80_LOOP_COUNTER];
    let vblanks = ram[Z80_VBLANK_COUNTER];
    for _ in 0..4 {
        c.run_frame();
    }
    let ram = c.bus().z80_space().ram();
    assert_eq!(
        ram[Z80_VBLANK_COUNTER],
        vblanks.wrapping_add(4),
        "one interrupt per frame"
    );
    assert_ne!(ram[Z80_LOOP_COUNTER], loops, "main loop keeps running");
    assert_eq!(c.bus().unmapped_accesses(), 0);
}

/// The interrupt line is held for a whole line: a handler that re-enables
/// interrupts right away is entered again within the same line.
#[test]
fn int_line_lasts_one_scanline() {
    let mut cpu = z80::Cpu::new();
    let mut bus = z80::FlatBus::new();
    // IM 1 handler at $38: inc (hl) ; ei ; ret — main program: halt loop.
    bus.mem[0x38..0x3B].copy_from_slice(&[0x34, 0xFB, 0xC9]);
    bus.mem[0..2].copy_from_slice(&[0x76, 0x18]);
    cpu.regs.sp = 0x8000;
    cpu.regs.set_hl(0x4000);
    cpu.regs.im = 1;
    cpu.regs.iff1 = true;
    cpu.set_int(true);
    let mut tstates = 0;
    while tstates < 228 {
        tstates += cpu.step(&mut bus);
    }
    assert!(bus.mem[0x4000] > 1, "handler re-entered while /INT is held");
}

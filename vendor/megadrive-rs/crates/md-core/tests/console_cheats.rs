//! Cheat codes on a running console.

use md_core::cheats::{Cheat, CheatError, encode_game_genie, parse};
use md_core::testrom::{SRAM_COUNTER_RAM, sram_counter};
use md_core::{Cartridge, Console, MdConfig};

/// `addq.b #1,d0` of `sram_counter()` (entry $200 + 4 + 6).
const ADDQ_AT: u32 = 0x20A;
const ADDQ_2_D0: u16 = 0x5400;

fn run(cheats: &[Cheat], frames: usize) -> Console {
    let mut c = Console::new(
        Cartridge::from_bytes(&sram_counter()).unwrap(),
        MdConfig::default(),
    );
    c.set_cheats(cheats).unwrap();
    for _ in 0..frames {
        c.run_frame();
    }
    c
}

fn counter(c: &Console) -> u32 {
    c.ram_long(SRAM_COUNTER_RAM) >> 24
}

#[test]
fn rom_code_patches_the_program() {
    assert_eq!(counter(&run(&[], 1)), 0x00, "$FF + 1");
    let raw = parse("00020A:5400").unwrap();
    assert_eq!(counter(&run(&[raw], 1)), 0x01, "$FF + 2");
    // The same patch spelled as a Game Genie code.
    let genie = encode_game_genie(Cheat {
        address: ADDQ_AT,
        value: ADDQ_2_D0,
    });
    assert_eq!(parse(&genie).unwrap(), raw);
    assert_eq!(counter(&run(&[parse(&genie).unwrap()], 1)), 0x01, "{genie}");
}

#[test]
fn rom_patch_is_read_only_and_removable() {
    let mut c = run(&[parse("00020A:5400").unwrap()], 1);
    assert_eq!(
        c.bus().cartridge().rom()[ADDQ_AT as usize + 1],
        0x00,
        "image intact"
    );
    c.set_cheats(&[]).unwrap();
    assert_eq!(c.bus().cartridge().read8(ADDQ_AT), 0x52, "addq.b #1 again");
}

/// RAM codes are forced every frame: byte when the high byte is 0, else word.
#[test]
fn ram_codes_are_forced_every_frame() {
    let cheats = [parse("FF0000:0042").unwrap(), parse("FF0002:1234").unwrap()];
    let c = run(&cheats, 2);
    // Frame 1: the program writes its count (0) over $FF0000; frame 2
    // starts by forcing the codes again.
    assert_eq!(c.ram_long(0xFF_0000), 0x4200_1234);
}

#[test]
fn codes_outside_rom_and_ram_are_refused() {
    let mut c = run(&[], 0);
    let vdp = parse("C00000:0000").unwrap();
    assert_eq!(c.set_cheats(&[vdp]), Err(CheatError::Target(vdp)));
    let past_rom = parse("100000:0000").unwrap();
    assert_eq!(c.set_cheats(&[past_rom]), Err(CheatError::Target(past_rom)));
}

//! Cartridge hardware: battery-backed SRAM and the Sega bank mapper.

use m68k::Bus;
use md_core::testrom::{
    BANK_RESULTS_RAM, SRAM_COUNTER_ADDR, SRAM_COUNTER_RAM, bank_switcher, sram_counter,
};
use md_core::{Cartridge, Console, MdConfig};

fn boot(rom: &[u8], sram: Option<&[u8]>) -> Console {
    let mut c = Console::new(Cartridge::from_bytes(rom).unwrap(), MdConfig::default());
    if let Some(bytes) = sram {
        c.load_sram(bytes);
    }
    c.run_frame();
    c
}

/// The boot count survives a power cycle when the SRAM image is kept, as the
/// frontend does with its `.srm` file.
#[test]
fn sram_survives_a_power_cycle() {
    let rom = sram_counter();
    let mut first = boot(&rom, None);
    assert_eq!(first.ram_long(SRAM_COUNTER_RAM) >> 24, 0x00, "$FF + 1");
    assert!(first.take_sram_dirty(), "the game wrote its SRAM");
    assert!(!first.take_sram_dirty(), "flag cleared once taken");
    let image = first.sram().expect("header declares SRAM").to_vec();
    assert_eq!(image[(SRAM_COUNTER_ADDR & 0xFFFF) as usize], 0x00);

    let second = boot(&rom, Some(&image));
    assert_eq!(second.ram_long(SRAM_COUNTER_RAM) >> 24, 0x01);
    let fresh = boot(&rom, None);
    assert_eq!(
        fresh.ram_long(SRAM_COUNTER_RAM) >> 24,
        0x00,
        "no image: $FF again"
    );
}

#[test]
fn cartridges_without_sram_header_have_none() {
    let c = boot(&bank_switcher(), None);
    assert!(c.sram().is_none());
}

/// SRAM overlapping the ROM (here: a 4 MiB image) is hidden until $A130F1
/// bit 0 is set; bit 1 write-protects it (GPGX mapper_sega_w).
#[test]
fn a130f1_maps_and_protects_sram() {
    let mut rom = sram_counter();
    rom.resize(0x40_0000, 0);
    rom[0x20_0001] = 0x77; // ROM byte under the SRAM
    let mut cart = Cartridge::from_bytes(&rom).unwrap();
    let read = |cart: &mut Cartridge, write: Option<(u32, u8)>| {
        if let Some((addr, value)) = write {
            cart.write8(addr, value);
        }
        cart.read8(SRAM_COUNTER_ADDR)
    };
    assert_eq!(
        read(&mut cart, Some((SRAM_COUNTER_ADDR, 0x42))),
        0x77,
        "ROM, write ignored"
    );
    cart.write_time(0xA1_30F1, 0x01);
    assert_eq!(read(&mut cart, None), 0xFF, "SRAM visible, untouched");
    assert_eq!(read(&mut cart, Some((SRAM_COUNTER_ADDR, 0x42))), 0x42);
    cart.write_time(0xA1_30F1, 0x03);
    assert_eq!(
        read(&mut cart, Some((SRAM_COUNTER_ADDR, 0x99))),
        0x42,
        "write-protected"
    );
    cart.write_time(0xA1_30F1, 0x00);
    assert_eq!(read(&mut cart, None), 0x77, "ROM again");
}

/// Word writes from the 68000 reach the register through the low byte.
#[test]
fn a130f1_word_write_from_the_bus() {
    let mut rom = sram_counter();
    rom.resize(0x40_0000, 0);
    let mut c = Console::new(Cartridge::from_bytes(&rom).unwrap(), MdConfig::default());
    let bus = c.bus_mut();
    bus.write16(0xA1_30F0, 0x0001);
    bus.write8(SRAM_COUNTER_ADDR, 0x5A);
    assert_eq!(bus.read8(SRAM_COUNTER_ADDR), 0x5A);
}

/// The 68000 program switches pages through the mapper registers.
#[test]
fn sega_mapper_switches_512k_pages() {
    let c = boot(&bank_switcher(), None);
    let words = |at: u32| c.ram_long(at) >> 16;
    assert_eq!(
        words(BANK_RESULTS_RAM),
        0xB007,
        "window 7 holds page 7 at power on"
    );
    assert_eq!(
        words(BANK_RESULTS_RAM + 2),
        0xB009,
        "page 9 mapped into window 7"
    );
    assert_eq!(
        words(BANK_RESULTS_RAM + 4),
        0xB008,
        "page 8 mapped into window 1"
    );
    assert_eq!(c.bus().unmapped_accesses(), 0);
}

//! Region detected from the header drives the console timing.

use md_core::{Cartridge, Console, MdConfig, Region};

/// `psg_tone` with its header country field set to `country`.
fn rom_for(country: &[u8; 4]) -> Cartridge {
    let mut rom = md_core::testrom::psg_tone();
    rom[0x1F0..0x1F4].copy_from_slice(country);
    Cartridge::from_bytes(&rom).unwrap()
}

/// Stereo pairs of audio in one frame of a console set up for `cart`'s
/// detected region.
fn pairs_per_frame(cart: Cartridge) -> (Region, usize) {
    let region = Region::detect(cart.region_field());
    let mut console = Console::new(
        cart,
        MdConfig {
            region,
            ..MdConfig::default()
        },
    );
    console.run_frame();
    console.run_frame();
    (region, console.audio_samples().len() / 2)
}

#[test]
fn european_header_runs_a_pal_console() {
    // PAL: 313 lines × 3420 master cycles at 53.203424 MHz ≈ 49.70 fps,
    // so ≈ 887 pairs at 44.1 kHz; NTSC: 262 lines at 53.693175 MHz ≈ 736.
    let (region, pal) = pairs_per_frame(rom_for(b"E   "));
    assert_eq!(region, Region::Europe);
    assert!((886..=888).contains(&pal), "{pal} pairs per PAL frame");
    let (region, ntsc) = pairs_per_frame(rom_for(b"U   "));
    assert_eq!(region, Region::Usa);
    assert!((735..=736).contains(&ntsc), "{ntsc} pairs per NTSC frame");
}

#[test]
fn generated_test_roms_stay_usa() {
    // Blank country field: USA, as before region detection (pinned hashes).
    let cart = Cartridge::from_bytes(&md_core::testrom::showcase()).unwrap();
    assert_eq!(Region::detect(cart.region_field()), Region::Usa);
}

//! End-to-end video: a generated ROM programs the VDP through the 68000,
//! the console runs whole frames, and every pixel of the picture is checked.

use md_core::testrom::{checkerboard, checkerboard_index};
use md_core::{Cartridge, Console, MdConfig};
use vdp::FRAME_HEIGHT;

/// Colours of the checkerboard ROM's palette 0, as RGBA8888.
fn expected_color(index: usize) -> u32 {
    match index {
        1 => 0xFF00_00FF, // red
        2 => 0x00FF_00FF, // green
        _ => 0x0000_FFFF, // blue backdrop
    }
}

#[test]
fn checkerboard_rom_renders_expected_picture() {
    let cart = Cartridge::from_bytes(&checkerboard()).unwrap();
    let mut console = Console::new(cart, MdConfig::default());
    // Frame 1 sets everything up; frame 2 is drawn with the final state.
    console.run_frame();
    console.run_frame();
    assert_eq!(console.frame_count(), 2);
    assert_eq!(console.screen_width(), 320);

    let fb = console.framebuffer();
    for y in 0..FRAME_HEIGHT {
        for x in 0..console.screen_width() {
            let want = expected_color(checkerboard_index(x, y));
            assert_eq!(fb[y * vdp::FRAME_WIDTH + x], want, "pixel ({x}, {y})");
        }
    }
    // The program ends in `bra.s *`: no fault, PC parked in the ROM.
    assert_eq!(console.cpu().state(), m68k::CpuState::Running);
    assert!(console.cpu().regs.pc < 0x1000);
    assert_eq!(console.bus().unmapped_accesses(), 0);
}

#[test]
fn frame_timing_is_stable() {
    let cart = Cartridge::from_bytes(&checkerboard()).unwrap();
    let mut a = Console::new(cart, MdConfig::default());
    let cart = Cartridge::from_bytes(&checkerboard()).unwrap();
    let mut b = Console::new(cart, MdConfig::default());
    for _ in 0..3 {
        a.run_frame();
        b.run_frame();
    }
    assert_eq!(
        a.framebuffer(),
        b.framebuffer(),
        "emulation is deterministic"
    );
}

#[test]
fn frame_rgba_and_hash_cover_the_visible_picture() {
    let cart = Cartridge::from_bytes(&checkerboard()).unwrap();
    let mut console = Console::new(cart, MdConfig::default());
    console.run_frame();
    console.run_frame();
    let rgba = console.frame_rgba();
    assert_eq!(rgba.len(), 320 * FRAME_HEIGHT * 4);
    assert_eq!(&rgba[0..4], &[0xFF, 0x00, 0x00, 0xFF], "top-left is red");
    assert_eq!(&rgba[32..36], &[0x00, 0xFF, 0x00, 0xFF], "x = 8 is green");
    let hash = console.frame_hash();
    // Pinned value: any change to rendering or colour conversion shows up here.
    assert_eq!(hash, 0x80e8_09f2_19f2_9b25);
    console.run_frame();
    assert_eq!(console.frame_hash(), hash, "static picture, stable hash");
}

/// Same picture, but palette and tiles uploaded by 68000 → VDP DMA from ROM
/// and plane B cleared by a DMA fill: the frame hash must be identical.
#[test]
fn dma_uploaded_checkerboard_matches_the_reference() {
    let cart = Cartridge::from_bytes(&md_core::testrom::checkerboard_dma()).unwrap();
    let mut console = Console::new(cart, MdConfig::default());
    console.run_frame();
    console.run_frame();
    assert_eq!(console.frame_hash(), 0x80e8_09f2_19f2_9b25);
    assert_eq!(console.bus().unmapped_accesses(), 0);
}

/// V30 (register 1 bit 3): on a PAL console the picture has 240 lines,
/// latched from the frame after the program sets the bit, and the
/// checkerboard covers lines 224-239 too.
#[test]
fn v30_checkerboard_has_240_lines_on_pal() {
    use md_core::{MdConfig, Region};
    let cart = Cartridge::from_bytes(&md_core::testrom::v30_checkerboard()).unwrap();
    let mut c = Console::new(
        cart,
        MdConfig {
            region: Region::Europe,
            ..MdConfig::default()
        },
    );
    for _ in 0..3 {
        c.run_frame();
    }
    assert_eq!(c.screen_height(), 240);
    assert_eq!(c.frame_rgba().len(), 320 * 240 * 4);
    let fb = c.framebuffer();
    let at = |x: usize, y: usize| fb[y * vdp::FRAME_WIDTH + x];
    // Cell (0, 29) holds tile 1 + ((0 ^ 29) & 1) = tile 2 (green); cell
    // (1, 29) tile 1 (red), as in the top rows.
    assert_eq!(
        at(4, 235),
        vdp::cram_to_rgba(0x00E0),
        "green cell in line 235"
    );
    assert_eq!(
        at(12, 235),
        vdp::cram_to_rgba(0x000E),
        "red cell in line 235"
    );
    assert_eq!(c.frame_hash(), V30_HASH, "pinned after checking the PNG");
}

/// Hash of `v30_checkerboard` on a PAL console at frame 3.
const V30_HASH: u64 = 0x11a0_7e49_5501_7f25;

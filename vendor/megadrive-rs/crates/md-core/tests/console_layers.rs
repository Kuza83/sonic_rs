//! End-to-end multi-layer picture: plane B, scrolled high-priority plane A
//! band, window and sprites, set up by the CPU in a generated ROM. Expected
//! colours are derived by hand from the layout, independently of the renderer.

use md_core::testrom::showcase_layout::*;
use md_core::testrom::{SHOWCASE_PALETTE, showcase};
use md_core::{Cartridge, Console, MdConfig};

const RED: usize = 2;
const GREEN: usize = 3;
const WHITE: usize = 4;
const YELLOW: usize = 5;

fn rgba(index: usize) -> u32 {
    vdp::cram_to_rgba(SHOWCASE_PALETTE[index])
}

fn frame() -> Console {
    let mut c = Console::new(
        Cartridge::from_bytes(&showcase()).unwrap(),
        MdConfig::default(),
    );
    c.run_frame();
    c.run_frame();
    c
}

#[test]
fn layers_compose_as_designed() {
    let c = frame();
    let px = |x: usize, y: usize| c.framebuffer()[y * vdp::FRAME_WIDTH + x];
    let band_start = BAND_HSCROLL;
    let band_end = BAND_HSCROLL + 8 * BAND_CELLS; // exclusive
    let checks = [
        ((0, 0), YELLOW, "window, top-left"),
        ((319, WINDOW_LINES - 1), YELLOW, "window, last line"),
        ((0, WINDOW_LINES), GREEN, "plane B under the window"),
        (
            (band_start - 1, BAND_TOP),
            GREEN,
            "left of the scrolled band",
        ),
        ((band_start, BAND_TOP), RED, "band start (hscroll)"),
        ((band_end - 1, BAND_TOP + 7), RED, "band end"),
        ((band_end, BAND_TOP), GREEN, "right of the band"),
        // Low sprite from tile 4: only its top-left cell (tile 4, white) is
        // opaque; tiles 5-7 are empty.
        (
            (LOW_SPRITE.0 + 2, LOW_SPRITE.1 + 2),
            WHITE,
            "low sprite over plane B",
        ),
        (
            (LOW_SPRITE.0 + 2, BAND_TOP + 2),
            RED,
            "high band over low sprite",
        ),
        (
            (LOW_SPRITE.0 + 2, LOW_SPRITE.1 + 14),
            GREEN,
            "transparent cell (tile 5)",
        ),
        (
            (LOW_SPRITE.0 + 10, LOW_SPRITE.1 + 2),
            GREEN,
            "transparent cell (tile 6)",
        ),
        // High sprite from tile 1: cells column by column = tiles 1, 2 / 3, 4.
        ((HIGH_SPRITE.0, HIGH_SPRITE.1), RED, "high sprite, tile 1"),
        (
            (HIGH_SPRITE.0, HIGH_SPRITE.1 + 8),
            GREEN,
            "high sprite, tile 2",
        ),
        (
            (HIGH_SPRITE.0 + 8, HIGH_SPRITE.1),
            YELLOW,
            "high sprite, tile 3",
        ),
        (
            (HIGH_SPRITE.0 + 15, HIGH_SPRITE.1 + 15),
            WHITE,
            "high sprite, tile 4",
        ),
        (
            (HIGH_SPRITE.0 + 16, HIGH_SPRITE.1),
            GREEN,
            "right of the high sprite",
        ),
        ((200, 200), GREEN, "plane B"),
    ];
    for ((x, y), index, what) in checks {
        assert_eq!(px(x, y), rgba(index), "{what} at ({x}, {y})");
    }
    assert_eq!(c.bus().unmapped_accesses(), 0);
    // Pinned after inspecting the PNG (docs/decisions.md, J7.5).
    assert_eq!(c.frame_hash(), 0xc68b_b1ab_26bf_1925);
}

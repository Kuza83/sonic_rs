//! Renderer tests: layers written through the ports, pixels checked.

use super::*;

const RED: u16 = 0x000E;
const GREEN: u16 = 0x00E0;
const BLUE: u16 = 0x0E00;

fn command(v: &mut Vdp, long: u32) {
    v.write_control((long >> 16) as u16);
    v.write_control(long as u16);
}

fn vram_write(addr: u32) -> u32 {
    ((addr & 0x3FFF) << 16) | ((addr & 0xC000) >> 14) | 0x4000_0000
}

/// Display on, H40, plane A at $C000 and plane B at $E000 (64×32, both
/// empty), backdrop = palette 0 entry 1.
fn setup() -> Vdp {
    let mut v = Vdp::new(false);
    for w in [
        0x8004, 0x8144, 0x8F02, 0x8230, 0x8407, 0x8C81, 0x9001, 0x8701,
    ] {
        v.write_control(w);
    }
    command(&mut v, 0xC000_0000); // CRAM write at 0
    for c in [0, BLUE, RED, GREEN] {
        v.write_data(c);
    }
    command(&mut v, 0xC020_0000); // palette 1, entry 0
    for c in [0, 0, GREEN] {
        v.write_data(c);
    }
    // Tile 1: left half colour 2, right half colour 3, except row 0 = colour 0.
    command(&mut v, vram_write(32));
    for row in 0..8 {
        let word = if row == 0 { 0x0000 } else { 0x2222 };
        v.write_data(word);
        v.write_data(if row == 0 { 0x0000 } else { 0x3333 });
    }
    v
}

fn pixel(v: &Vdp, x: usize, y: usize) -> u32 {
    v.framebuffer()[y * FRAME_WIDTH + x]
}

#[test]
fn color_conversion_is_linear_ramp() {
    assert_eq!(cram_to_rgba(0x0000), 0x0000_00FF);
    assert_eq!(cram_to_rgba(0x0EEE), 0xFFFF_FFFF);
    assert_eq!(cram_to_rgba(RED), 0xFF00_00FF);
    assert_eq!(cram_to_rgba(0x0002), 0x2400_00FF);
}

/// Register 0 bit 2 clear: only the low bit of each component is used.
#[test]
fn palette_select_clear_keeps_low_bits() {
    let mut v = setup();
    v.write_control(0x8000);
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE & 0x0222));
    assert_eq!(
        pixel(&v, 0, 0),
        0x0000_24FF,
        "blue level 7 shown as level 1"
    );
    v.write_control(0x8004);
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE), "CRAM itself unchanged");
}

#[test]
fn empty_plane_shows_backdrop() {
    let mut v = setup();
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE));
    assert_eq!(pixel(&v, 319, 0), cram_to_rgba(BLUE));
}

#[test]
fn tile_pixels_palette_and_transparency() {
    let mut v = setup();
    // Plane A cell (1, 0) = tile 1, palette 0; cell (2, 0) = tile 1, palette 1.
    command(&mut v, vram_write(0xC002));
    v.write_data(0x0001);
    v.write_data(0x2001);
    v.render_line(0);
    v.render_line(1);
    // Row 0 of the tile is transparent.
    assert_eq!(pixel(&v, 8, 0), cram_to_rgba(BLUE));
    assert_eq!(pixel(&v, 8, 1), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 12, 1), cram_to_rgba(GREEN));
    // Palette 1 entry 2 is green.
    assert_eq!(pixel(&v, 16, 1), cram_to_rgba(GREEN));
}

#[test]
fn flips() {
    let mut v = setup();
    command(&mut v, vram_write(0xC000));
    v.write_data(0x0801); // hflip
    v.write_data(0x1001); // vflip
    v.render_line(1);
    v.render_line(7);
    // hflip: right half (colour 3) now on the left.
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(GREEN));
    assert_eq!(pixel(&v, 7, 1), cram_to_rgba(RED));
    // vflip: the transparent row 0 is now row 7.
    assert_eq!(pixel(&v, 8, 7), cram_to_rgba(BLUE));
    assert_eq!(pixel(&v, 8, 1), cram_to_rgba(RED));
}

#[test]
fn h32_and_display_off_use_backdrop() {
    let mut v = setup();
    command(&mut v, vram_write(0xC000 + 2 * 33));
    v.write_data(0x0001); // cell 33 is beyond H32's 32 columns
    v.write_control(0x8C00); // H32
    v.render_line(1);
    assert_eq!(v.registers().screen_width(), 256);
    assert_eq!(pixel(&v, 264, 1), cram_to_rgba(BLUE));
    v.write_control(0x8104); // display off
    command(&mut v, vram_write(0xC000));
    v.write_data(0x0001);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(BLUE));
}

/// Writes name table entry `entry` at cell (`cx`, `cy`) of a 64-wide plane
/// whose table starts at `base`.
fn set_cell(v: &mut Vdp, base: u32, cx: u32, cy: u32, entry: u16) {
    command(v, vram_write(base + 2 * (cy * 64 + cx)));
    v.write_data(entry);
}

const PLANE_A: u32 = 0xC000;
const PLANE_B: u32 = 0xE000;
/// Tile 1, palette 0 (colours 2/3) and palette 1 (colour 2 = green).
const TILE1_PAL0: u16 = 0x0001;
const TILE1_PAL1: u16 = 0x2001;
const PRIORITY: u16 = 0x8000;

#[test]
fn plane_b_shows_through_transparent_plane_a() {
    let mut v = setup();
    set_cell(&mut v, PLANE_B, 0, 0, TILE1_PAL0);
    v.render_line(0);
    v.render_line(1);
    // Row 0 of tile 1 is transparent in both planes: backdrop.
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE));
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED));
}

#[test]
fn plane_a_wins_at_equal_priority() {
    let mut v = setup();
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL1); // left half: palette 1 colour 2 = green
    set_cell(&mut v, PLANE_B, 0, 0, TILE1_PAL0); // left half: red
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(GREEN));
    // Both high: still A.
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL1 | PRIORITY);
    set_cell(&mut v, PLANE_B, 0, 0, TILE1_PAL0 | PRIORITY);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(GREEN));
}

#[test]
fn high_priority_plane_b_covers_low_plane_a() {
    let mut v = setup();
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL1);
    set_cell(&mut v, PLANE_B, 0, 0, TILE1_PAL0 | PRIORITY);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED));
}

/// Hscroll table at $FC00 (register 13 = $3F).
const HSCROLL_TABLE: u32 = 0xFC00;

fn write_vsram(v: &mut Vdp, index: u32, value: u16) {
    command(v, 0x4000_0010 | ((2 * index) << 16));
    v.write_data(value);
}

/// Plane A cell (2, 2) = tile 1 (red left half, green right half, row 0 transparent).
fn scroll_setup() -> Vdp {
    let mut v = setup();
    v.write_control(0x8D3F); // hscroll table at $FC00
    set_cell(&mut v, PLANE_A, 2, 2, TILE1_PAL0);
    v
}

#[test]
fn full_screen_hscroll_moves_plane_right() {
    let mut v = scroll_setup();
    command(&mut v, vram_write(HSCROLL_TABLE));
    v.write_data(3); // plane A +3 pixels
    v.render_line(17);
    assert_eq!(pixel(&v, 16, 17), cram_to_rgba(BLUE), "shifted out");
    assert_eq!(pixel(&v, 19, 17), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 23, 17), cram_to_rgba(GREEN));
}

#[test]
fn negative_hscroll_wraps_around_the_plane() {
    let mut v = scroll_setup();
    command(&mut v, vram_write(HSCROLL_TABLE));
    v.write_data((-16i16) as u16); // plane A 16 pixels left
    v.render_line(17);
    assert_eq!(pixel(&v, 0, 17), cram_to_rgba(RED));
}

#[test]
fn per_line_hscroll_uses_each_line_entry() {
    let mut v = scroll_setup();
    v.write_control(0x8B03); // per-line hscroll
    command(&mut v, vram_write(HSCROLL_TABLE + 4 * 18));
    v.write_data(8); // only line 18 moves
    v.render_line(17);
    v.render_line(18);
    assert_eq!(pixel(&v, 16, 17), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 16, 18), cram_to_rgba(BLUE));
    assert_eq!(pixel(&v, 24, 18), cram_to_rgba(RED));
}

#[test]
fn per_cell_hscroll_shares_an_entry_per_8_lines() {
    let mut v = scroll_setup();
    v.write_control(0x8B02); // per-cell hscroll
    command(&mut v, vram_write(HSCROLL_TABLE + 4 * 16));
    v.write_data(8); // lines 16-23
    v.render_line(21);
    assert_eq!(pixel(&v, 24, 21), cram_to_rgba(RED));
}

#[test]
fn plane_b_uses_second_hscroll_word() {
    let mut v = scroll_setup();
    set_cell(&mut v, PLANE_B, 0, 2, TILE1_PAL0);
    command(&mut v, vram_write(HSCROLL_TABLE + 2));
    v.write_data(40);
    v.render_line(17);
    assert_eq!(pixel(&v, 40, 17), cram_to_rgba(RED));
}

#[test]
fn full_screen_vscroll_moves_plane_up() {
    let mut v = scroll_setup();
    write_vsram(&mut v, 0, 8); // plane A 8 pixels up
    v.render_line(9);
    assert_eq!(
        pixel(&v, 16, 9),
        cram_to_rgba(RED),
        "cell row 2 now at line 8"
    );
}

#[test]
fn two_cell_vscroll_columns() {
    let mut v = scroll_setup();
    set_cell(&mut v, PLANE_A, 4, 2, TILE1_PAL0);
    v.write_control(0x8B04); // 2-cell vertical scroll
    write_vsram(&mut v, 2, 8); // column 1 (x 16-31), plane A
    v.render_line(9);
    assert_eq!(pixel(&v, 16, 9), cram_to_rgba(RED), "column 1 scrolled");
    assert_eq!(
        pixel(&v, 32, 9),
        cram_to_rgba(BLUE),
        "column 2 not scrolled"
    );
}

/// Window table at $B000 (register 3 = $2C), 64 cells per row in H40.
const WINDOW: u32 = 0xB000;

fn window_setup() -> Vdp {
    let mut v = setup();
    v.write_control(0x832C);
    // Plane A cell (0, 0) green-ish (palette 1), window cell (0, 0) red/green (palette 0).
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL1);
    command(&mut v, vram_write(WINDOW));
    v.write_data(TILE1_PAL0);
    v
}

#[test]
fn window_disabled_by_default() {
    let mut v = window_setup();
    assert_eq!(v.registers().window_base(), 0xB000);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(GREEN), "plane A");
}

#[test]
fn window_on_top_lines() {
    let mut v = window_setup();
    v.write_control(0x9201); // DOWN = 0, split at 8: lines 0-7 are window
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED), "window, not scrolled");
    // Hscroll does not move the window.
    v.write_control(0x8D3F);
    command(&mut v, vram_write(0xFC00));
    v.write_data(5);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED));
}

#[test]
fn window_left_columns() {
    let mut v = window_setup();
    set_cell(&mut v, PLANE_A, 2, 0, TILE1_PAL1);
    command(&mut v, vram_write(WINDOW + 2 * 2));
    v.write_data(TILE1_PAL0);
    v.write_control(0x9101); // RIGT = 0, split at 16 px: x < 16 is window
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED), "window");
    assert_eq!(pixel(&v, 16, 1), cram_to_rgba(GREEN), "plane A");
}

#[test]
fn window_right_side_from_zero_covers_the_line() {
    let mut v = window_setup();
    v.write_control(0x9180); // RIGT = 1, split at 0: whole line
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), cram_to_rgba(RED));
}

/// Sprite attribute table at $F000 (register 5 = $78).
const SAT: u32 = 0xF000;

/// Writes sprite `index`: screen position, size in cells, link, attributes.
fn set_sprite(v: &mut Vdp, index: u32, x: i32, y: i32, size: (u16, u16), link: u16, attr: u16) {
    command(v, vram_write(SAT + 8 * index));
    v.write_data((y + 128) as u16);
    v.write_data(((size.0 - 1) << 10) | ((size.1 - 1) << 8) | link);
    v.write_data(attr);
    v.write_data((x + 128) as u16);
}

/// Solid tiles: tile 4 = colour 1 (blue), tile 5 = colour 2 (red),
/// tile 6 = colour 3 (green), tile 7 = colour 2 in the left half only.
fn sprite_setup() -> Vdp {
    let mut v = setup();
    v.write_control(0x8578);
    for (tile, row) in [
        (4, 0x1111_1111u32),
        (5, 0x2222_2222),
        (6, 0x3333_3333),
        (7, 0x2222_0000),
    ] {
        command(&mut v, vram_write(32 * tile));
        for _ in 0..8 {
            v.write_data((row >> 16) as u16);
            v.write_data(row as u16);
        }
    }
    v
}

#[test]
fn single_sprite_position() {
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 16, 8, (1, 1), 0, 5);
    v.render_line(7);
    v.render_line(8);
    v.render_line(15);
    v.render_line(16);
    assert_eq!(pixel(&v, 16, 7), cram_to_rgba(BLUE), "above");
    assert_eq!(pixel(&v, 16, 8), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 23, 15), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 24, 8), cram_to_rgba(BLUE), "right of it");
    assert_eq!(pixel(&v, 16, 16), cram_to_rgba(BLUE), "below");
}

#[test]
fn multi_cell_sprite_tiles_are_column_major() {
    let mut v = sprite_setup();
    // 2×2 cells from tile 4: column 0 = tiles 4, 5; column 1 = tiles 6, 7.
    set_sprite(&mut v, 0, 0, 0, (2, 2), 0, 4);
    v.render_line(0);
    v.render_line(8);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE), "tile 4");
    assert_eq!(pixel(&v, 0, 8), cram_to_rgba(RED), "tile 5");
    assert_eq!(pixel(&v, 8, 0), cram_to_rgba(GREEN), "tile 6");
    assert_eq!(pixel(&v, 8, 8), cram_to_rgba(RED), "tile 7 left half");
    assert_eq!(
        pixel(&v, 12, 8),
        cram_to_rgba(BLUE),
        "tile 7 right half transparent"
    );
}

#[test]
fn sprite_flips_reorder_cells_and_pixels() {
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 0, 0, (2, 2), 0, 0x1800 | 4); // hflip + vflip
    v.render_line(0);
    v.render_line(8);
    // Bottom-right cell (tile 7) now drawn top-left, mirrored: colour on the right half.
    assert_eq!(
        pixel(&v, 0, 0),
        cram_to_rgba(BLUE),
        "tile 7 transparent half"
    );
    assert_eq!(pixel(&v, 4, 0), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 8, 0), cram_to_rgba(RED), "tile 5");
    assert_eq!(pixel(&v, 0, 8), cram_to_rgba(GREEN), "tile 6");
    assert_eq!(
        pixel(&v, 8, 8),
        cram_to_rgba(BLUE),
        "tile 4 (colour 1 = blue)"
    );
}

#[test]
fn earlier_sprite_is_in_front_and_link_chain_is_followed() {
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 0, 0, (1, 1), 2, 5); // red, links to 2
    set_sprite(&mut v, 1, 0, 0, (1, 1), 0, 4); // skipped by the chain
    set_sprite(&mut v, 2, 4, 0, (1, 1), 0, 6); // green, behind sprite 0
    v.render_line(0);
    assert_eq!(pixel(&v, 4, 0), cram_to_rgba(RED));
    assert_eq!(pixel(&v, 10, 0), cram_to_rgba(GREEN));
    assert_ne!(v.read_status() & crate::status::SPRITE_COLLISION, 0);
    assert_eq!(
        v.read_status() & crate::status::SPRITE_COLLISION,
        0,
        "cleared by read"
    );
}

#[test]
fn sprite_priority_against_planes() {
    let mut v = sprite_setup();
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL1 | PRIORITY); // high plane A: green left half
    set_sprite(&mut v, 0, 0, 0, (1, 1), 0, 5); // low sprite
    v.render_line(1);
    assert_eq!(
        pixel(&v, 0, 1),
        cram_to_rgba(GREEN),
        "high plane over low sprite"
    );
    set_sprite(&mut v, 0, 0, 0, (1, 1), 0, 0x8000 | 5); // high sprite
    v.render_line(1);
    assert_eq!(
        pixel(&v, 0, 1),
        cram_to_rgba(RED),
        "high sprite over high plane"
    );
}

#[test]
fn twenty_sprites_per_line_in_h40() {
    let mut v = sprite_setup();
    for i in 0..21u16 {
        let link = if i < 20 { i + 1 } else { 0 };
        set_sprite(&mut v, i as u32, 8 * i as i32, 0, (1, 1), link, 5);
    }
    v.render_line(0);
    assert_eq!(pixel(&v, 8 * 19, 0), cram_to_rgba(RED), "20th sprite drawn");
    assert_eq!(pixel(&v, 8 * 20, 0), cram_to_rgba(BLUE), "21st dropped");
    assert_ne!(v.read_status() & crate::status::SPRITE_OVERFLOW, 0);
}

#[test]
fn pixel_budget_truncates_the_last_sprite() {
    let mut v = sprite_setup();
    // Ten 4-cell-wide sprites = 320 pixels: the 10th fits exactly; an 11th is cut.
    for i in 0..11u16 {
        let link = if i < 10 { i + 1 } else { 0 };
        let x = if i < 10 { -40 } else { 200 };
        set_sprite(&mut v, i as u32, x, 0, (4, 1), link, 5);
    }
    v.render_line(0);
    assert_eq!(
        pixel(&v, 200, 0),
        cram_to_rgba(BLUE),
        "11th sprite over budget"
    );
}

#[test]
fn sprite_at_x0_masks_following_sprites_after_a_visible_one() {
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 100, 0, (1, 1), 1, 4);
    set_sprite(&mut v, 1, -128, 0, (1, 1), 2, 4); // raw X = 0: mask
    set_sprite(&mut v, 2, 0, 0, (1, 1), 0, 5);
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(BLUE), "masked");
    // Without a visible sprite before it, X = 0 does not mask.
    set_sprite(&mut v, 0, -128, 0, (1, 1), 1, 4);
    set_sprite(&mut v, 1, 0, 0, (1, 1), 0, 5);
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), cram_to_rgba(RED));
}

use super::Intensity;

/// Shadow/highlight on; palette 3 colours 14/15 and palette 0 colour 14 set.
fn ste_setup() -> Vdp {
    let mut v = sprite_setup();
    v.write_control(0x8C89); // H40 + shadow/highlight
    command(&mut v, 0xC07C_0000); // CRAM 62, 63 (palette 3 colours 14, 15)
    v.write_data(0x0EEE);
    v.write_data(0x0EEE);
    command(&mut v, 0xC01C_0000); // CRAM 14 (palette 0 colour 14)
    v.write_data(RED);
    // Tile 8: solid colour 14; tile 9: solid colour 15.
    for (tile, row) in [(8, 0xEEEE_EEEEu32), (9, 0xFFFF_FFFF)] {
        command(&mut v, vram_write(32 * tile));
        for _ in 0..8 {
            v.write_data((row >> 16) as u16);
            v.write_data(row as u16);
        }
    }
    v
}

fn rgba(color: u16, intensity: Intensity) -> u32 {
    super::cram_to_rgba_with(color, intensity)
}

#[test]
fn intensity_levels() {
    // Channel value 7: shadow 7/14, normal 14/14, highlight 14/14.
    assert_eq!(rgba(RED, Intensity::Shadow), 0x8000_00FF);
    assert_eq!(rgba(RED, Intensity::Normal), 0xFF00_00FF);
    // Channel value 2: shadow 2/14 = 36, highlight 9/14 = 164.
    assert_eq!(rgba(0x0004, Intensity::Shadow) >> 24, 36);
    assert_eq!(rgba(0x0004, Intensity::Highlight) >> 24, 164);
}

#[test]
fn low_priority_planes_are_shadowed() {
    let mut v = ste_setup();
    set_cell(&mut v, PLANE_A, 0, 0, TILE1_PAL0);
    v.render_line(1);
    assert_eq!(pixel(&v, 0, 1), rgba(RED, Intensity::Shadow), "low plane A");
    assert_eq!(pixel(&v, 100, 1), rgba(BLUE, Intensity::Shadow), "backdrop");
}

#[test]
fn a_priority_bit_lifts_the_shadow_even_on_transparent_pixels() {
    let mut v = ste_setup();
    set_cell(&mut v, PLANE_B, 0, 0, TILE1_PAL0 | PRIORITY);
    v.render_line(0); // row 0 of tile 1 is transparent
    assert_eq!(
        pixel(&v, 0, 0),
        rgba(BLUE, Intensity::Normal),
        "backdrop, normal"
    );
}

#[test]
fn sprites_in_shadow_highlight_mode() {
    let mut v = ste_setup();
    set_sprite(&mut v, 0, 0, 0, (1, 1), 1, 5); // low red sprite over shadowed planes
    set_sprite(&mut v, 1, 8, 0, (1, 1), 2, 0x8000 | 5); // high red sprite
    set_sprite(&mut v, 2, 16, 0, (1, 1), 0, 8); // palette 0 colour 14: never shadowed
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), rgba(RED, Intensity::Shadow));
    assert_eq!(pixel(&v, 8, 0), rgba(RED, Intensity::Normal));
    assert_eq!(pixel(&v, 16, 0), rgba(RED, Intensity::Normal));
}

#[test]
fn palette3_operator_sprites() {
    let mut v = ste_setup();
    // Normal planes (priority bit) under both operators.
    for cx in 0..4 {
        set_cell(&mut v, PLANE_B, cx, 0, PRIORITY);
    }
    set_sprite(&mut v, 0, 0, 0, (1, 1), 1, 0x6000 | 9); // palette 3 colour 15: shadow
    set_sprite(&mut v, 1, 8, 0, (1, 1), 0, 0x6000 | 8); // palette 3 colour 14: highlight
    v.render_line(0);
    assert_eq!(pixel(&v, 0, 0), rgba(BLUE, Intensity::Shadow));
    assert_eq!(pixel(&v, 8, 0), rgba(BLUE, Intensity::Highlight));
    // Over shadowed planes, the highlight operator restores normal intensity.
    for cx in 0..4 {
        set_cell(&mut v, PLANE_B, cx, 0, 0);
    }
    v.render_line(0);
    assert_eq!(pixel(&v, 8, 0), rgba(BLUE, Intensity::Normal));
}

/// Sprite table base used to move the table in the cache tests: $D800 is
/// neither plane A ($C000) nor plane B ($E000).
const OTHER_SAT: u32 = 0xD800;
const OTHER_SAT_REG: u16 = 0x8500 | (OTHER_SAT >> 9) as u16;

/// Writes sprite `index` at `table` (not necessarily the current table).
fn set_sprite_at(v: &mut Vdp, table: u32, index: u32, x: i32, y: i32, attr: u16) {
    command(v, vram_write(table + 8 * index));
    for w in [(y + 128) as u16, 0, attr, (x + 128) as u16] {
        v.write_data(w);
    }
}

/// GPGX parse_satb_m5: Y, size and link from the internal copy, name,
/// attributes and X from VRAM; moving the table does not reload the copy.
#[test]
fn moving_the_sprite_table_keeps_the_cached_y() {
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 16, 8, (1, 1), 0, 5); // red at (16, 8), cached
    set_sprite_at(&mut v, OTHER_SAT, 0, 40, 200, 6); // green, Y 200: not cached
    v.write_control(OTHER_SAT_REG);
    v.render_line(8);
    assert_eq!(
        pixel(&v, 40, 8),
        cram_to_rgba(GREEN),
        "cached Y, X and tile from VRAM"
    );
    assert_eq!(pixel(&v, 16, 8), cram_to_rgba(BLUE));
    v.render_line(200);
    assert_eq!(pixel(&v, 40, 200), cram_to_rgba(BLUE), "VRAM Y ignored");
}

#[test]
fn a_table_written_before_it_is_selected_is_not_cached() {
    let mut v = sprite_setup();
    set_sprite_at(&mut v, OTHER_SAT, 0, 16, 8, 5);
    v.write_control(OTHER_SAT_REG);
    v.render_line(8);
    assert_eq!(pixel(&v, 16, 8), cram_to_rgba(BLUE), "cache still empty");
    // Rewriting it once selected fills the cache.
    set_sprite_at(&mut v, OTHER_SAT, 0, 16, 8, 5);
    v.render_line(8);
    assert_eq!(pixel(&v, 16, 8), cram_to_rgba(RED));
}

#[test]
fn dma_copy_into_the_table_updates_the_cache() {
    let mut v = sprite_setup();
    set_sprite_at(&mut v, OTHER_SAT, 0, 16, 8, 5); // source, outside the table
    v.write_control(0x8154); // DMA enabled
    v.write_control(0x8F01);
    for w in [
        0x9308,
        0x9400,
        0x9500 | (OTHER_SAT & 0xFF) as u16,
        0x9600 | (OTHER_SAT >> 8) as u16,
        0x97C0,
    ] {
        v.write_control(w); // copy 8 bytes from $D800
    }
    command(
        &mut v,
        ((SAT & 0x3FFF) << 16) | ((SAT & 0xC000) >> 14) | 0x0000_00C0,
    );
    for line in 0..4 {
        v.start_line(line); // the copy runs with the beam
    }
    v.write_control(0x8F02);
    v.render_line(8);
    assert_eq!(pixel(&v, 16, 8), cram_to_rgba(RED));
}

#[test]
fn h32_table_region_is_512_bytes() {
    let mut v = sprite_setup();
    v.write_control(0x8C00); // H32: table $F000-$F1FF
    command(&mut v, vram_write(SAT + 0x200));
    v.write_data(0xABCD);
    assert_eq!(
        &v.sat_cache[0..2],
        &[0, 0],
        "$F200 is outside the H32 table"
    );
    command(&mut v, vram_write(SAT + 0x1FE));
    v.write_data(0xABCD);
    assert_eq!(&v.sat_cache[0x1FE..0x200], &[0xAB, 0xCD]);
}

#[test]
fn save_state_keeps_the_sprite_cache() {
    use savestate::{Reader, State, Writer};
    let mut v = sprite_setup();
    set_sprite(&mut v, 0, 16, 8, (1, 1), 0, 5);
    v.write_control(OTHER_SAT_REG);
    let mut w = Writer::new();
    v.visit(&mut w).unwrap();
    let bytes = w.into_bytes();
    let mut restored = Vdp::new(false);
    restored.visit(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(restored.sat_cache[..], v.sat_cache[..]);
}

/// `render_planes` (one decoded tile row reused per cell) gives exactly the
/// per-pixel reference on random VRAM, VSRAM and register settings: plane
/// sizes, bases, scroll modes, window positions, H32/H40, every line.
#[test]
fn tile_row_rendering_matches_the_per_pixel_reference() {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for case in 0..40 {
        let mut v = Vdp::new(false);
        for byte in v.vram.iter_mut() {
            *byte = next() as u8;
        }
        for word in v.vsram.iter_mut() {
            *word = next() as u16 & 0x3FF;
        }
        let h40 = case % 2 == 0;
        // Valid plane sizes (register 16): 32/64/128 cells, at most 4096 cells.
        let size = [0x00, 0x01, 0x03, 0x10, 0x11, 0x30][case % 6];
        for w in [
            0x8004,
            0x8144,
            0x8200 | (next() as u16 & 0x38), // plane A base
            0x8300 | (next() as u16 & 0x3E), // window base
            0x8400 | (next() as u16 & 0x07), // plane B base
            0x8B00 | (next() as u16 & 0x07), // vscroll / hscroll modes
            if h40 { 0x8C81 } else { 0x8C00 },
            0x8D00 | (next() as u16 & 0x3F), // hscroll base
            0x9000 | size,
            0x9100 | (next() as u16 & 0x9F), // window H
            0x9200 | (next() as u16 & 0x9F), // window V
        ] {
            v.write_control(w);
        }
        let width = v.regs.screen_width();
        for line in 0..FRAME_HEIGHT {
            v.render_planes(line, width);
            let (a, b) = v.render_planes_reference(line, width);
            assert_eq!(
                v.lines.plane_a[..width],
                a[..width],
                "case {case} line {line}: plane A"
            );
            assert_eq!(
                v.lines.plane_b[..width],
                b[..width],
                "case {case} line {line}: plane B"
            );
        }
    }
}

/// `tile_row` is `tile_pixel` for the 8 pixels of a row, for every flip,
/// palette and priority combination.
#[test]
fn tile_row_matches_tile_pixel() {
    let mut v = Vdp::new(false);
    for (i, byte) in v.vram.iter_mut().enumerate().take(64 * 32) {
        *byte = (i * 37 % 251) as u8;
    }
    for entry in (0..0x10000u32).step_by(97).map(|e| e as u16 & 0xF83F) {
        for py in 0..TILE_SIZE {
            let row = v.tile_row(entry, py);
            for (px, &pixel) in row.iter().enumerate() {
                assert_eq!(
                    pixel,
                    v.tile_pixel(entry, px, py),
                    "entry {entry:04X} ({px}, {py})"
                );
            }
        }
    }
}

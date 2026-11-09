//! Sprites (Plutiedev "Sprites"; Genesis Plus GX `parse_satb_m5` and
//! `render_obj_m5` for the per-line rules).
//!
//! Attribute table entry (8 bytes): Y (9 bits, screen + 128); size (width
//! and height in cells, 1-4 each) and link; name table style attributes
//! (priority, palette, flips, tile); X (9 bits, screen + 128). Multi-cell
//! sprites store their tiles column by column.
//!
//! Per line: at most 20 sprites (16 in H32), beyond which the overflow flag
//! is set; at most 320 (256) sprite pixels, off-screen ones included, the
//! last sprite being truncated. A sprite at X = 0 masks the following ones
//! on its line, provided a sprite with X ≠ 0 came before it on that line
//! (or the previous line reached the pixel budget).

use super::{FRAME_WIDTH, PIXEL_TRANSPARENT, TILE_SIZE, is_opaque};
use crate::Vdp;

const SPRITE_ENTRY_BYTES: usize = 8;
const SPRITE_POSITION_MASK: u16 = 0x1FF;
const SPRITE_OFFSET: usize = 128;
const SPRITE_LINK_MASK: u16 = 0x7F;
const NAME_TILE_MASK: u16 = 0x07FF;
const NAME_HFLIP: u16 = 1 << 11;
const NAME_VFLIP: u16 = 1 << 12;
/// Line limits (sprites, pixels) and table length, H40 then H32.
const H40_LIMITS: (usize, usize, usize) = (20, 320, 80);
const H32_LIMITS: (usize, usize, usize) = (16, 256, 64);
/// Upper bound of sprites kept for one line.
const MAX_LINE_SPRITES: usize = 20;

/// A sprite intersecting the current line.
#[derive(Debug, Clone, Copy, Default)]
struct LineSprite {
    x: u16,
    attr: u16,
    width_cells: usize,
    height_cells: usize,
    /// Row of the sprite on this line (0 = top).
    row: usize,
}

/// Sprite state carried across lines and reported in the status register.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpriteState {
    /// Masking armed by the previous line reaching the pixel budget.
    pub mask_carry: bool,
    pub overflow: bool,
    pub collision: bool,
}

impl Vdp {
    fn sprite_limits(&self) -> (usize, usize, usize) {
        if self.regs.h40() {
            H40_LIMITS
        } else {
            H32_LIMITS
        }
    }

    fn sat_cache_word(&self, offset: usize) -> u16 {
        u16::from_be_bytes([self.sat_cache[offset], self.sat_cache[offset + 1]])
    }

    /// Walks the attribute table, keeping the sprites present on `line`.
    fn parse_sprites(&mut self, line: usize, list: &mut [LineSprite; MAX_LINE_SPRITES]) -> usize {
        let (max_sprites, _, table_len) = self.sprite_limits();
        let base = self.regs.sprite_table_base() as usize;
        let target = line + SPRITE_OFFSET;
        let mut count = 0;
        let mut index = 0;
        let (_, cache_mask) = self.sat_masks();
        for _ in 0..table_len {
            let entry = base + SPRITE_ENTRY_BYTES * index;
            // Y, size and link come from the internal table copy, name,
            // attributes and X from VRAM (GPGX parse_satb_m5).
            let cached = (SPRITE_ENTRY_BYTES * index) & cache_mask as usize;
            let y = (self.sat_cache_word(cached) & SPRITE_POSITION_MASK) as usize;
            let size_link = self.sat_cache_word(cached + 2);
            let width_cells = 1 + ((size_link >> 10) & 3) as usize;
            let height_cells = 1 + ((size_link >> 8) & 3) as usize;
            if target >= y && target - y < height_cells * TILE_SIZE {
                if count == max_sprites {
                    self.sprite_state.overflow = true;
                    break;
                }
                list[count] = LineSprite {
                    x: self.vram_word(entry + 6) & SPRITE_POSITION_MASK,
                    attr: self.vram_word(entry + 4),
                    width_cells,
                    height_cells,
                    row: target - y,
                };
                count += 1;
            }
            let link = (size_link & SPRITE_LINK_MASK) as usize;
            if link == 0 || link >= table_len {
                break;
            }
            index = link;
        }
        count
    }

    /// Fills the sprite layer for `line`. Earlier sprites are in front.
    pub(crate) fn render_sprites(&mut self, line: usize, width: usize) {
        // Fresh transparent line (a stack array: no allocation per line).
        let mut buffer = [PIXEL_TRANSPARENT; FRAME_WIDTH];
        let mut src = [0u32; FRAME_WIDTH];
        let mut list = [LineSprite::default(); MAX_LINE_SPRITES];
        let count = self.parse_sprites(line, &mut list);
        let (_, max_pixels, _) = self.sprite_limits();

        let mut mask_armed = self.sprite_state.mask_carry;
        let mut masked = false;
        let mut pixel_count = 0;
        self.sprite_state.mask_carry = false;
        for sprite in &list[..count] {
            if sprite.x != 0 {
                mask_armed = true;
            } else if mask_armed {
                masked = true;
            }
            let sprite_width = sprite.width_cells * TILE_SIZE;
            pixel_count += sprite_width;
            if !masked {
                let truncated = pixel_count.saturating_sub(max_pixels);
                let columns = (sprite_width - truncated.min(sprite_width)) / TILE_SIZE;
                self.draw_sprite(sprite, columns, width, &mut buffer, &mut src);
            }
            if pixel_count >= max_pixels {
                self.sprite_state.mask_carry = pixel_count >= width;
                break;
            }
        }
        self.lines.sprites = buffer;
        self.lines.src_s = src;
    }

    fn draw_sprite(
        &mut self,
        s: &LineSprite,
        columns: usize,
        width: usize,
        buffer: &mut [u8; FRAME_WIDTH],
        src: &mut [u32; FRAME_WIDTH],
    ) {
        let hflip = s.attr & NAME_HFLIP != 0;
        let vflip = s.attr & NAME_VFLIP != 0;
        let cell_row = s.row / TILE_SIZE;
        let cell_row = if vflip {
            s.height_cells - 1 - cell_row
        } else {
            cell_row
        };
        let screen_x = s.x as isize - SPRITE_OFFSET as isize;
        for column in 0..columns {
            let cell_col = if hflip {
                s.width_cells - 1 - column
            } else {
                column
            };
            let tile = (s.attr & NAME_TILE_MASK) as usize + cell_col * s.height_cells + cell_row;
            let entry = (s.attr & !NAME_TILE_MASK) | (tile as u16 & NAME_TILE_MASK);
            for px in 0..TILE_SIZE {
                let x = screen_x + (column * TILE_SIZE + px) as isize;
                if x < 0 || x as usize >= width {
                    continue;
                }
                let pixel = self.tile_pixel(entry, px, s.row % TILE_SIZE);
                if !is_opaque(pixel) {
                    continue;
                }
                let slot = &mut buffer[x as usize];
                if is_opaque(*slot) {
                    self.sprite_state.collision = true;
                } else {
                    *slot = pixel;
                    if self.prov_enabled { src[x as usize] = super::src_word(entry, px, s.row % TILE_SIZE); }
                }
            }
        }
    }
}

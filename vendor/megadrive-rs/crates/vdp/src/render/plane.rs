//! Scroll planes A and B: name table lookup and tile pixel fetch.

use super::{
    PALETTE_SIZE, PIXEL_PRIORITY, PIXEL_TRANSPARENT, TILE_BYTES, TILE_ROW_BYTES, TILE_SIZE,
};
use crate::Vdp;

const NAME_TILE_MASK: u16 = 0x07FF;
const NAME_HFLIP: u16 = 1 << 11;
const NAME_VFLIP: u16 = 1 << 12;
const NAME_PALETTE_SHIFT: u16 = 13;
const NAME_PRIORITY: u16 = 1 << 15;
/// Each hscroll table line holds two words: plane A, then plane B.
const HSCROLL_ENTRY_BYTES: usize = 4;
/// 2-cell vertical scroll columns are 16 pixels wide; VSRAM holds A then B
/// for each column.
const VSCROLL_COLUMN_WIDTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plane {
    A,
    B,
}

impl Vdp {
    /// Layer pixel of a tile whose name table entry is `entry`, at pixel
    /// (`px`, `py`) inside the 8×8 cell.
    pub(crate) fn tile_pixel(&self, entry: u16, px: usize, py: usize) -> u8 {
        let px = if entry & NAME_HFLIP != 0 {
            TILE_SIZE - 1 - px
        } else {
            px
        };
        let py = if entry & NAME_VFLIP != 0 {
            TILE_SIZE - 1 - py
        } else {
            py
        };
        let tile = (entry & NAME_TILE_MASK) as usize;
        let byte = self.vram[(tile * TILE_BYTES + py * TILE_ROW_BYTES + px / 2) & 0xFFFF];
        let color = if px.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0F
        };
        let priority = if entry & NAME_PRIORITY != 0 {
            PIXEL_PRIORITY
        } else {
            0
        };
        if color == 0 {
            // Transparent, but the priority bit still matters for
            // shadow/highlight (GPGX make_lut_bg_ste).
            return PIXEL_TRANSPARENT | priority;
        }
        let palette = ((entry >> NAME_PALETTE_SHIFT) & 3) as u8;
        priority | (palette * PALETTE_SIZE + color)
    }

    /// The 8 layer pixels of row `py` of the tile whose name table entry is
    /// `entry`, left to right on screen: `tile_pixel` for a whole row, with
    /// the entry decoded and the 4 bytes of the row read once.
    pub(crate) fn tile_row(&self, entry: u16, py: usize) -> [u8; TILE_SIZE] {
        let py = if entry & NAME_VFLIP != 0 {
            TILE_SIZE - 1 - py
        } else {
            py
        };
        let tile = (entry & NAME_TILE_MASK) as usize;
        let at = tile * TILE_BYTES + py * TILE_ROW_BYTES;
        let bytes = [0, 1, 2, 3].map(|i| self.vram[(at + i) & 0xFFFF]);
        let priority = if entry & NAME_PRIORITY != 0 {
            PIXEL_PRIORITY
        } else {
            0
        };
        let palette = ((entry >> NAME_PALETTE_SHIFT) & 3) as u8 * PALETTE_SIZE;
        let hflip = entry & NAME_HFLIP != 0;
        std::array::from_fn(|x| {
            let px = if hflip { TILE_SIZE - 1 - x } else { x };
            let byte = bytes[px / 2];
            let color = if px.is_multiple_of(2) {
                byte >> 4
            } else {
                byte & 0x0F
            };
            if color == 0 {
                PIXEL_TRANSPARENT | priority
            } else {
                priority | (palette + color)
            }
        })
    }

    /// Pixel of `plane` at plane coordinates (`x`, `y`), wrapped to the
    /// plane size (reference for `render_planes`).
    #[cfg(test)]
    pub(crate) fn plane_pixel(&self, plane: Plane, x: usize, y: usize) -> u8 {
        let (plane_w, plane_h) = self.regs.plane_size();
        let base = match plane {
            Plane::A => self.regs.plane_a_base(),
            Plane::B => self.regs.plane_b_base(),
        } as usize;
        let cell_x = (x / TILE_SIZE) & (plane_w - 1);
        let cell_y = (y / TILE_SIZE) & (plane_h - 1);
        let entry = self.vram_word(base + 2 * (cell_y * plane_w + cell_x));
        self.tile_pixel(entry, x % TILE_SIZE, y % TILE_SIZE)
    }

    /// Horizontal scroll of `plane` for screen line `line` (Plutiedev
    /// "Scrolling"; GPGX render_bg_m5: first word plane A, second plane B).
    fn hscroll(&self, plane: Plane, line: usize) -> usize {
        let entry = self.regs.hscroll_base() as usize
            + HSCROLL_ENTRY_BYTES * self.regs.hscroll_line(line)
            + if plane == Plane::B { 2 } else { 0 };
        self.vram_word(entry) as usize
    }

    /// Vertical scroll of `plane` at screen column `x`: one value for the
    /// whole screen, or one per 16-pixel column in 2-cell mode (columns
    /// counted on the screen; the partial-column quirk with fine horizontal
    /// scrolling is not reproduced).
    fn vscroll(&self, plane: Plane, x: usize) -> usize {
        let column = if self.regs.vscroll_2cell() {
            x / VSCROLL_COLUMN_WIDTH
        } else {
            0
        };
        let index = 2 * column + if plane == Plane::B { 1 } else { 0 };
        self.vsram[index & (self.vsram.len() - 1)] as usize
    }

    /// Window pixel at screen (`x`, `line`): fixed, unscrolled name table
    /// (reference for `render_planes`).
    #[cfg(test)]
    fn window_pixel(&self, x: usize, line: usize) -> u8 {
        let cell = (line / TILE_SIZE) * self.regs.window_row_cells() + x / TILE_SIZE;
        let entry = self.vram_word(self.regs.window_base() as usize + 2 * cell);
        self.tile_pixel(entry, x % TILE_SIZE, line % TILE_SIZE)
    }

    /// Columns `[start, end)` of screen line `line` where the window
    /// replaces plane A (`Registers::window_at` for the whole line).
    fn window_span(&self, line: usize, width: usize) -> (usize, usize) {
        match (
            self.regs.window_at(0, line),
            self.regs.window_at(width - 1, line),
        ) {
            (true, true) => (0, width),
            (false, false) => (0, 0),
            // Left part (split > 0) or right part: find the boundary.
            (true, false) => (
                0,
                (0..width)
                    .find(|&x| !self.regs.window_at(x, line))
                    .unwrap_or(width),
            ),
            (false, true) => (
                (0..width)
                    .find(|&x| self.regs.window_at(x, line))
                    .unwrap_or(width),
                width,
            ),
        }
    }

    /// Fills the plane layer buffers for `line`, applying both scrolls; the
    /// window replaces plane A where it is active. Works span by span: a
    /// span shares one tile row (decoded once by `tile_row`) and ends at the
    /// tile edge, at a 16-pixel vertical scroll column (2-cell mode) or at
    /// the window edge; per-pixel lookups were 40 % of the rendering time.
    /// Same result as `render_planes_reference`.
    pub(crate) fn render_planes(&mut self, line: usize, width: usize) {
        let (plane_w, plane_h) = self.regs.plane_size();
        let (wrap_x, wrap_y) = (plane_w * TILE_SIZE - 1, plane_h * TILE_SIZE - 1);
        let (window_start, window_end) = self.window_span(line, width);
        let window_base = self.regs.window_base() as usize;
        let window_row = (line / TILE_SIZE) * self.regs.window_row_cells();
        let two_cell = self.regs.vscroll_2cell();
        let mut buffers = std::mem::take(&mut self.lines);
        for (plane, buffer) in [
            (Plane::A, &mut buffers.plane_a),
            (Plane::B, &mut buffers.plane_b),
        ] {
            let base = match plane {
                Plane::A => self.regs.plane_a_base(),
                Plane::B => self.regs.plane_b_base(),
            } as usize;
            let h = self.hscroll(plane, line);
            let window = if plane == Plane::A {
                window_start..window_end
            } else {
                0..0
            };
            let mut x = 0;
            while x < width {
                let (addr, fine_y, fine_x, end) = if window.contains(&x) {
                    // Window cells are fixed on the screen.
                    let end = window.end.min(x - x % TILE_SIZE + TILE_SIZE);
                    let addr = window_base + 2 * (window_row + x / TILE_SIZE);
                    (addr, line % TILE_SIZE, x % TILE_SIZE, end)
                } else {
                    let mut limit = if x < window.start {
                        window.start
                    } else {
                        width
                    };
                    if two_cell {
                        limit = limit.min(x - x % VSCROLL_COLUMN_WIDTH + VSCROLL_COLUMN_WIDTH);
                    }
                    let px = x.wrapping_sub(h) & wrap_x;
                    let py = (line + self.vscroll(plane, x)) & wrap_y;
                    let cell_x = (px / TILE_SIZE) & (plane_w - 1);
                    let cell_y = (py / TILE_SIZE) & (plane_h - 1);
                    let fine_x = px % TILE_SIZE;
                    let end = limit.min(x + TILE_SIZE - fine_x);
                    (
                        base + 2 * (cell_y * plane_w + cell_x),
                        py % TILE_SIZE,
                        fine_x,
                        end,
                    )
                };
                let entry = self.vram_word(addr);
                let row = self.tile_row(entry, fine_y);
                let len = end - x;
                buffer[x..end].copy_from_slice(&row[fine_x..fine_x + len]);
                if self.prov_enabled {
                    let src = if plane == Plane::A { &mut buffers.src_a } else { &mut buffers.src_b };
                    for i in 0..len { src[x + i] = super::src_word(entry, fine_x + i, fine_y); }
                }
                x = end;
            }
        }
        self.lines = buffers;
    }

    /// The per-pixel version of `render_planes`, kept as its test reference.
    #[cfg(test)]
    pub(crate) fn render_planes_reference(
        &self,
        line: usize,
        width: usize,
    ) -> ([u8; super::FRAME_WIDTH], [u8; super::FRAME_WIDTH]) {
        let (plane_w, plane_h) = self.regs.plane_size();
        let (wrap_x, wrap_y) = (plane_w * TILE_SIZE - 1, plane_h * TILE_SIZE - 1);
        let mut out = (
            [PIXEL_TRANSPARENT; super::FRAME_WIDTH],
            [PIXEL_TRANSPARENT; super::FRAME_WIDTH],
        );
        for (plane, buffer) in [(Plane::A, &mut out.0), (Plane::B, &mut out.1)] {
            let h = self.hscroll(plane, line);
            for (x, pixel) in buffer.iter_mut().enumerate().take(width) {
                if plane == Plane::A && self.regs.window_at(x, line) {
                    *pixel = self.window_pixel(x, line);
                    continue;
                }
                let px = x.wrapping_sub(h) & wrap_x;
                let py = (line + self.vscroll(plane, x)) & wrap_y;
                *pixel = self.plane_pixel(plane, px, py);
            }
        }
        out
    }
}

//! Scanline renderer.
//!
//! Each line is built in layer buffers (plane B, plane A/window, sprites),
//! then composed by priority (`compose.rs`). A layer pixel is one byte:
//! bits 5-0 = CRAM index (palette × 16 + colour), bit 6 = priority; colour 0
//! of a palette is transparent.
//!
//! Layouts (Plutiedev "Tiles", "Planes", "Palettes"):
//! - name table entry `PCCV HAAA AAAA AAAA`: priority, palette, vflip, hflip,
//!   tile index (×32 bytes);
//! - tile: 8×8 pixels, 4 bits per pixel, 4 bytes per row, left pixel in the
//!   high nibble.

mod compose;
mod plane;
mod sprites;

pub(crate) use sprites::SpriteState;

/// Format du mot d'origine d'un pixel (`Vdp::provenance`), un par pixel du framebuffer :
/// bits 0-10 indice de tuile ; 11-13 colonne (0-7) et 14-16 ligne (0-7) du pixel DANS la cellule, côté écran
/// (avant retournement) ; 17 retournement horizontal ; 18 vertical ; 19-24 indice CRAM (palette × 16 + couleur) ;
/// 25-26 intensité (0 normale, 1 ombre, 2 surbrillance) ; 27-28 couche (0 fond, 1 plan A ou fenêtre, 2 plan B, 3 sprite).
pub mod provenance {
    pub const TILE_MASK: u32 = 0x7FF;
    pub const COL_SHIFT: u32 = 11;
    pub const ROW_SHIFT: u32 = 14;
    pub const HFLIP: u32 = 1 << 17;
    pub const VFLIP: u32 = 1 << 18;
    pub const COLOR_SHIFT: u32 = 19;
    pub const INTENSITY_SHIFT: u32 = 25;
    pub const LAYER_SHIFT: u32 = 27;
    pub const LAYER_BACKDROP: u32 = 0;
    pub const LAYER_A: u32 = 1;
    pub const LAYER_B: u32 = 2;
    pub const LAYER_SPRITE: u32 = 3;
}

/// Mot d'origine d'un pixel de tuile (sans couleur, intensité ni couche, ajoutées à la composition).
pub(crate) fn src_word(entry: u16, px: usize, py: usize) -> u32 {
    (entry as u32 & provenance::TILE_MASK)
        | ((px as u32) << provenance::COL_SHIFT)
        | ((py as u32) << provenance::ROW_SHIFT)
        | if entry & (1 << 11) != 0 { provenance::HFLIP } else { 0 }
        | if entry & (1 << 12) != 0 { provenance::VFLIP } else { 0 }
}

use crate::Vdp;
use crate::memory::CRAM_ENTRIES;

pub const FRAME_WIDTH: usize = 320;
pub const FRAME_HEIGHT: usize = 224;
/// Rows of the framebuffer: the V30 picture height.
pub const MAX_FRAME_HEIGHT: usize = crate::MAX_ACTIVE_LINES as usize;

pub(crate) const TILE_SIZE: usize = 8;
pub(crate) const TILE_BYTES: usize = 32;
pub(crate) const TILE_ROW_BYTES: usize = 4;
pub(crate) const PALETTE_SIZE: u8 = 16;
/// Layer pixel priority flag (bits 5-0 hold the CRAM index).
pub(crate) const PIXEL_PRIORITY: u8 = 0x40;
/// A pixel whose colour within its palette is 0 is transparent.
pub(crate) const PIXEL_TRANSPARENT: u8 = 0x00;
/// Opaque alpha for RGBA8888 pixels.
const ALPHA: u32 = 0xFF;

pub(crate) fn is_opaque(pixel: u8) -> bool {
    pixel & 0x0F != 0
}

pub(crate) fn is_high(pixel: u8) -> bool {
    pixel & PIXEL_PRIORITY != 0
}

/// Per-line layer buffers, reused for every line (no allocation per line).
#[derive(Clone)]
pub(crate) struct LineBuffers {
    pub plane_a: [u8; FRAME_WIDTH],
    pub plane_b: [u8; FRAME_WIDTH],
    pub sprites: [u8; FRAME_WIDTH],
    pub src_a: [u32; FRAME_WIDTH],
    pub src_b: [u32; FRAME_WIDTH],
    pub src_s: [u32; FRAME_WIDTH],
}

impl Default for LineBuffers {
    fn default() -> Self {
        Self {
            plane_a: [PIXEL_TRANSPARENT; FRAME_WIDTH],
            plane_b: [PIXEL_TRANSPARENT; FRAME_WIDTH],
            sprites: [PIXEL_TRANSPARENT; FRAME_WIDTH],
            src_a: [0; FRAME_WIDTH],
            src_b: [0; FRAME_WIDTH],
            src_s: [0; FRAME_WIDTH],
        }
    }
}

/// Brightness of a pixel in shadow/highlight mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intensity {
    Normal,
    Shadow,
    Highlight,
}

/// Intensities, the rows of the palette cache.
const INTENSITIES: usize = 3;

/// CRAM converted to RGBA per intensity, rebuilt when `dirty`. Derived
/// data: not saved in states, and left out of `Debug` so that a loaded
/// machine compares equal to the saved one.
#[derive(Clone)]
pub(crate) struct PaletteCache {
    colors: [[u32; CRAM_ENTRIES]; INTENSITIES],
    pub dirty: bool,
}

impl Default for PaletteCache {
    fn default() -> Self {
        Self {
            colors: [[0; CRAM_ENTRIES]; INTENSITIES],
            dirty: true,
        }
    }
}

impl std::fmt::Debug for PaletteCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaletteCache(derived from CRAM)")
    }
}

/// Each channel has 15 output levels (GPGX palette_init): shadow = v,
/// normal = 2v, highlight = v + 7, for a 3-bit CRAM value v.
const MAX_LEVEL: u32 = 14;

/// Level 0-14 to 8 bits, rounded. For normal intensity this is the linear
/// ramp 0, 36, 73, 109, 146, 182, 219, 255 (docs/decisions.md, J4.1).
fn level_to_8bit(level: u32) -> u32 {
    (level * 255 + MAX_LEVEL / 2) / MAX_LEVEL
}

fn channel(v: u16, intensity: Intensity) -> u32 {
    let v = (v & 7) as u32;
    level_to_8bit(match intensity {
        Intensity::Shadow => v,
        Intensity::Normal => 2 * v,
        Intensity::Highlight => v + 7,
    })
}

/// Low bit of each component of a CRAM word: what remains with register 0
/// bit 2 clear (`MODE_1_FULL_PALETTE`).
const CRAM_LOW_BITS: u16 = 0x0222;

/// CRAM word `----BBB-GGG-RRR-` to RGBA8888 (0xRRGGBBAA) at `intensity`.
pub fn cram_to_rgba_with(color: u16, intensity: Intensity) -> u32 {
    let r = channel(color >> 1, intensity);
    let g = channel(color >> 5, intensity);
    let b = channel(color >> 9, intensity);
    (r << 24) | (g << 16) | (b << 8) | ALPHA
}

/// CRAM word to RGBA8888 at normal intensity.
pub fn cram_to_rgba(color: u16) -> u32 {
    cram_to_rgba_with(color, Intensity::Normal)
}

impl Vdp {
    pub(crate) fn color(&self, index: u8) -> u32 {
        self.color_with(index, Intensity::Normal)
    }

    /// RGBA of CRAM entry `index` at `intensity`, from the palette cache
    /// (rebuilt by `refresh_palette` at the start of each rendered line).
    pub(crate) fn color_with(&self, index: u8, intensity: Intensity) -> u32 {
        self.palette.colors[intensity as usize][(index & 0x3F) as usize & (CRAM_ENTRIES - 1)]
    }

    /// Rebuilds the RGBA palette cache after a CRAM or register 0 write:
    /// converting colours per pixel cost a third of the rendering time.
    pub(crate) fn refresh_palette(&mut self) {
        if !std::mem::take(&mut self.palette.dirty) {
            return;
        }
        let full = self.regs.full_palette();
        for intensity in [Intensity::Normal, Intensity::Shadow, Intensity::Highlight] {
            for (entry, out) in self.palette.colors[intensity as usize]
                .iter_mut()
                .enumerate()
            {
                let color = if full {
                    self.cram[entry]
                } else {
                    self.cram[entry] & CRAM_LOW_BITS
                };
                *out = cram_to_rgba_with(color, intensity);
            }
        }
    }

    pub(crate) fn vram_word(&self, addr: usize) -> u16 {
        let a = addr & 0xFFFE;
        u16::from_be_bytes([self.vram[a], self.vram[a + 1]])
    }

    /// Renders active display line `line` into the framebuffer. Columns past
    /// the screen width (H32) and a disabled display show the backdrop.
    pub fn render_line(&mut self, line: usize) {
        if line >= self.active_lines as usize {
            return;
        }
        self.refresh_palette();
        if self.prov_enabled && line + 1 == self.active_lines as usize {
            self.prov_snapshot = (self.vram.to_vec(), self.cram.to_vec());
        }
        let width = self.regs.screen_width();
        if self.regs.display_enabled() {
            self.render_planes(line, width);
            self.render_sprites(line, width);
            self.compose_line(line, width);
        } else {
            let backdrop = self.color(self.regs.background_index() as u8);
            self.framebuffer[line * FRAME_WIDTH..(line + 1) * FRAME_WIDTH].fill(backdrop);
            if self.prov_enabled { self.prov[line * FRAME_WIDTH..(line + 1) * FRAME_WIDTH].fill((self.regs.background_index() as u32) << provenance::COLOR_SHIFT); }
            return;
        }
        let backdrop = self.color(self.regs.background_index() as u8);
        self.framebuffer[line * FRAME_WIDTH + width..(line + 1) * FRAME_WIDTH].fill(backdrop);
        if self.prov_enabled { self.prov[line * FRAME_WIDTH + width..(line + 1) * FRAME_WIDTH].fill((self.regs.background_index() as u32) << provenance::COLOR_SHIFT); }
    }
}

#[cfg(test)]
mod tests;

/// Scratch buffers rewritten before every use: left out of `Debug` (and of
/// save states) so whole-machine comparisons ignore them.
impl std::fmt::Debug for LineBuffers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LineBuffers")
    }
}

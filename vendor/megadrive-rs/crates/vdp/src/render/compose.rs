//! Layer composition by priority.
//!
//! From front to back (Plutiedev "Planes", priority section):
//! high sprites, high plane A, high plane B, low sprites, low plane A,
//! low plane B, backdrop. The first opaque pixel in that order wins.
//!
//! Shadow/highlight (register 12 bit 3), after Genesis Plus GX
//! `make_lut_bg_ste` / `make_lut_bgobj_ste`:
//! - the planes are at normal intensity where plane A or B has its priority
//!   bit set (even on a transparent pixel), shadowed elsewhere;
//! - a sprite pixel in front of the planes is drawn normal if the sprite has
//!   priority or the planes are normal, shadowed otherwise; colour 14 of
//!   palettes 0-2 is never shadowed;
//! - palette 3 colour 15 sprites are not drawn: they shadow what is under
//!   them; palette 3 colour 14 sprites highlight it (shadow → normal,
//!   normal → highlight).

use super::{FRAME_WIDTH, Intensity, is_high, is_opaque, provenance as pv};
use crate::Vdp;

/// Sprite colours with a special meaning in shadow/highlight mode.
const SPRITE_HIGHLIGHT_OPERATOR: u8 = 0x3E;
const SPRITE_SHADOW_OPERATOR: u8 = 0x3F;
const NEVER_SHADOWED_COLOR: u8 = 0x0E;
const PIXEL_COLOR_INDEX: u8 = 0x3F;

impl Vdp {
    pub(crate) fn compose_line(&mut self, line: usize, width: usize) {
        if self.regs.shadow_highlight() {
            self.compose_line_shadow_highlight(line, width);
        } else {
            self.compose_line_normal(line, width);
        }
    }

    fn compose_line_normal(&mut self, line: usize, width: usize) {
        let backdrop = self.color(self.regs.background_index() as u8);
        let colors: [u32; 64] = std::array::from_fn(|i| self.color(i as u8));
        let lines = &self.lines;
        let enabled = self.prov_enabled;
        let backdrop_index = self.regs.background_index();
        let prov = &mut self.prov[line * FRAME_WIDTH..line * FRAME_WIDTH + width];
        let out = &mut self.framebuffer[line * FRAME_WIDTH..line * FRAME_WIDTH + width];
        for (x, pixel) in out.iter_mut().enumerate() {
            let (s, a, b) = (lines.sprites[x], lines.plane_a[x], lines.plane_b[x]);
            let front = |p: u8| is_opaque(p) && is_high(p);
            // High layers first, then low ones: an opaque pixel left for the
            // second pass is necessarily low.
            let (winner, layer, src) = if front(s) {
                (s, pv::LAYER_SPRITE, lines.src_s[x])
            } else if front(a) {
                (a, pv::LAYER_A, lines.src_a[x])
            } else if front(b) {
                (b, pv::LAYER_B, lines.src_b[x])
            } else if is_opaque(s) {
                (s, pv::LAYER_SPRITE, lines.src_s[x])
            } else if is_opaque(a) {
                (a, pv::LAYER_A, lines.src_a[x])
            } else if is_opaque(b) {
                (b, pv::LAYER_B, lines.src_b[x])
            } else {
                *pixel = backdrop;
                if enabled { prov[x] = (backdrop_index as u32) << pv::COLOR_SHIFT; }
                continue;
            };
            *pixel = colors[(winner & PIXEL_COLOR_INDEX) as usize];
            if enabled { prov[x] = src | (((winner & PIXEL_COLOR_INDEX) as u32) << pv::COLOR_SHIFT) | (layer << pv::LAYER_SHIFT); }
        }
    }

    fn compose_line_shadow_highlight(&mut self, line: usize, width: usize) {
        let backdrop = self.regs.background_index() as u8;
        for x in 0..width {
            let (s, a, b) = (
                self.lines.sprites[x],
                self.lines.plane_a[x],
                self.lines.plane_b[x],
            );
            // Planes alone (same order as without shadow/highlight).
            let plane_found = [(a, true, true), (b, true, false), (a, false, true), (b, false, false)]
                .into_iter()
                .find(|&(p, high, _)| is_opaque(p) && is_high(p) == high);
            let plane = plane_found.map(|(p, _, _)| p);
            let (plane_layer, plane_src) = match plane_found {
                Some((_, _, true)) => (pv::LAYER_A, self.lines.src_a[x]),
                Some((_, _, false)) => (pv::LAYER_B, self.lines.src_b[x]),
                None => (pv::LAYER_BACKDROP, 0),
            };
            let plane_normal = is_high(a) || is_high(b);
            let plane_intensity = if plane_normal {
                Intensity::Normal
            } else {
                Intensity::Shadow
            };
            let under = plane.unwrap_or(backdrop);
            let plane_in_front = plane.is_some_and(is_high) && !is_high(s);
            let sprite_shown = is_opaque(s) && !plane_in_front;
            let (index, intensity) = if is_opaque(s) && !plane_in_front {
                match s & PIXEL_COLOR_INDEX {
                    SPRITE_SHADOW_OPERATOR => (under, Intensity::Shadow),
                    SPRITE_HIGHLIGHT_OPERATOR => (
                        under,
                        if plane_normal {
                            Intensity::Highlight
                        } else {
                            Intensity::Normal
                        },
                    ),
                    c if c & 0x0F == NEVER_SHADOWED_COLOR && c < 0x30 => (s, Intensity::Normal),
                    _ if is_high(s) => (s, Intensity::Normal),
                    _ => (s, plane_intensity),
                }
            } else {
                (under, plane_intensity)
            };
            self.framebuffer[line * FRAME_WIDTH + x] = self.color_with(index, intensity);
            if self.prov_enabled {
                // Un sprite « opérateur » (couleur 14/15 de la palette 3) ne dessine rien : on garde l'origine du dessous.
                let drawn_sprite = sprite_shown && index == (s & PIXEL_COLOR_INDEX);
                let (layer, src) = if drawn_sprite { (pv::LAYER_SPRITE, self.lines.src_s[x]) } else { (plane_layer, plane_src) };
                let inten = match intensity { Intensity::Normal => 0, Intensity::Shadow => 1, Intensity::Highlight => 2 };
                self.prov[line * FRAME_WIDTH + x] = src | ((index as u32 & 0x3F) << pv::COLOR_SHIFT) | (inten << pv::INTENSITY_SHIFT) | (layer << pv::LAYER_SHIFT);
            }
        }
    }
}

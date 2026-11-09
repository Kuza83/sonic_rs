//! Our own window chrome on SDL: hit testing for the borderless window,
//! drawing of the title bar, and the window icon. Geometry and behaviour
//! come from `crate::chrome` (unit-tested without SDL).

use std::ffi::c_void;
use std::ptr;

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, Canvas};
use sdl2::surface::Surface;
use sdl2::sys;
use sdl2::video::Window;

use crate::chrome::{self, Button, Edge, Hit, Layout};

pub(super) const BAR_COLOR: Color = Color::RGBA(16, 16, 24, 190);
pub(super) const TEXT_COLOR: Color = Color::RGBA(235, 235, 240, 255);
pub(super) const HOVER_COLOR: Color = Color::RGBA(255, 255, 255, 40);
const CLOSE_HOVER_COLOR: Color = Color::RGBA(200, 40, 40, 230);
const ICON_SIZE: u32 = 32;

/// Which hit-test rules a borderless window follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Our bar drags, its buttons click (`Layout::hit`).
    Library,
    /// The whole picture drags (`Layout::game_hit`).
    Game,
}

/// Lets the window manager move the window and resize it by its edges
/// (SDL_SetWindowHitTest, not wrapped by the sdl2 crate). Returns the SDL
/// error when the platform does not support hit testing.
pub fn install_hit_test(window: &Window, kind: WindowKind) -> Result<(), String> {
    let callback: sys::SDL_HitTest = match kind {
        WindowKind::Library => Some(hit_test),
        WindowKind::Game => Some(game_hit_test),
    };
    // SAFETY: `window.raw()` is a valid SDL window for as long as `window`
    // lives, and SDL drops the callback with the window. The callbacks read
    // nothing through `data` (null) and only call SDL getters on the window
    // they are given.
    let status = unsafe { sys::SDL_SetWindowHitTest(window.raw(), callback, ptr::null_mut()) };
    if status == 0 {
        Ok(())
    } else {
        Err(sdl2::get_error())
    }
}

/// The layout of `window` and the hit point, for the hit-test callbacks.
///
/// # Safety
/// `window` and `area` must be the live window and point SDL passes.
unsafe fn hit_context(
    window: *mut sys::SDL_Window,
    area: *const sys::SDL_Point,
) -> (Layout, i32, i32) {
    let (mut w, mut h) = (0, 0);
    // SAFETY: guaranteed by the caller (SDL's own arguments).
    let (point, flags) = unsafe {
        sys::SDL_GetWindowSize(window, &mut w, &mut h);
        (*area, sys::SDL_GetWindowFlags(window))
    };
    let fullscreen = flags & sys::SDL_WindowFlags::SDL_WINDOW_FULLSCREEN as u32 != 0;
    (
        Layout::new(w.max(1) as u32, h.max(1) as u32, !fullscreen),
        point.x,
        point.y,
    )
}

/// Library window: the bar drags, the edges resize, the buttons and the
/// contents stay normal (their clicks reach the event loop).
unsafe extern "C" fn hit_test(
    window: *mut sys::SDL_Window,
    area: *const sys::SDL_Point,
    _data: *mut c_void,
) -> sys::SDL_HitTestResult {
    // SAFETY: SDL calls this with its live window and a valid point.
    let (layout, x, y) = unsafe { hit_context(window, area) };
    sdl_hit(layout.hit(x, y))
}

/// Game window: the picture drags, the edges resize.
unsafe extern "C" fn game_hit_test(
    window: *mut sys::SDL_Window,
    area: *const sys::SDL_Point,
    _data: *mut c_void,
) -> sys::SDL_HitTestResult {
    // SAFETY: SDL calls this with its live window and a valid point.
    let (layout, x, y) = unsafe { hit_context(window, area) };
    sdl_hit(layout.game_hit(x, y))
}

fn sdl_hit(hit: Hit) -> sys::SDL_HitTestResult {
    use sys::SDL_HitTestResult::*;
    match hit {
        Hit::Drag => SDL_HITTEST_DRAGGABLE,
        Hit::Resize(edge) => match edge {
            Edge::Top => SDL_HITTEST_RESIZE_TOP,
            Edge::Bottom => SDL_HITTEST_RESIZE_BOTTOM,
            Edge::Left => SDL_HITTEST_RESIZE_LEFT,
            Edge::Right => SDL_HITTEST_RESIZE_RIGHT,
            Edge::TopLeft => SDL_HITTEST_RESIZE_TOPLEFT,
            Edge::TopRight => SDL_HITTEST_RESIZE_TOPRIGHT,
            Edge::BottomLeft => SDL_HITTEST_RESIZE_BOTTOMLEFT,
            Edge::BottomRight => SDL_HITTEST_RESIZE_BOTTOMRIGHT,
        },
        Hit::Normal | Hit::Button(_) => SDL_HITTEST_NORMAL,
    }
}

fn sdl_rect(r: chrome::Rect) -> SdlRect {
    SdlRect::new(r.x, r.y, r.w.max(1), r.h.max(1))
}

/// `draw_text` in the current text colour, for the welcome screen.
pub fn draw_text_at(
    canvas: &mut Canvas<Window>,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
) -> Result<(), String> {
    canvas.set_draw_color(TEXT_COLOR);
    draw_text(canvas, text, x, y, scale)
}

/// `draw_text` in the colour already set on the canvas.
pub fn draw_text_colour(
    canvas: &mut Canvas<Window>,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
) -> Result<(), String> {
    draw_text(canvas, text, x, y, scale)
}

/// Draws `text` at (`x`, `y`) in the bar font, each font pixel `scale` wide.
fn draw_text(
    canvas: &mut Canvas<Window>,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
) -> Result<(), String> {
    let s = scale as i32;
    for (rx, ry, len) in chrome::text_runs(text) {
        canvas.fill_rect(SdlRect::new(x + rx * s, y + ry * s, len * scale, scale))?;
    }
    Ok(())
}

/// The bar: translucent background, game title, then the minimize,
/// fullscreen and close buttons; `hover` lights the button under the mouse.
pub fn draw_bar(
    canvas: &mut Canvas<Window>,
    layout: &Layout,
    title: &str,
    hover: Hit,
) -> Result<(), String> {
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(BAR_COLOR);
    canvas.fill_rect(sdl_rect(layout.bar))?;

    let scale = layout.scale;
    let buttons_x = layout.buttons[0].1.x;
    let (tx, ty) = layout.title_origin();
    // Keep the title clear of the buttons (one glyph of margin).
    let room = (buttons_x - tx).max(0) as u32 / scale;
    let mut shown: String = title.into();
    while !shown.is_empty() && chrome::text_width(&shown) + chrome::GLYPH > room {
        shown.pop();
    }
    canvas.set_draw_color(TEXT_COLOR);
    draw_text(canvas, &shown, tx, ty, scale)?;

    for (button, rect) in layout.buttons {
        if hover == Hit::Button(button) {
            canvas.set_draw_color(if button == Button::Close {
                CLOSE_HOVER_COLOR
            } else {
                HOVER_COLOR
            });
            canvas.fill_rect(sdl_rect(rect))?;
        }
        canvas.set_draw_color(TEXT_COLOR);
        // Symbols: a 4-pixel unit square box centred in the button.
        let unit = scale as i32 * 2;
        let (cx, cy) = (rect.x + rect.w as i32 / 2, rect.y + rect.h as i32 / 2);
        let s = scale as i32;
        match button {
            Button::Minimize => canvas.fill_rect(SdlRect::new(
                cx - 2 * unit,
                cy + unit,
                (4 * unit) as u32,
                s as u32,
            ))?,
            Button::Fullscreen => canvas.draw_rect(SdlRect::new(
                cx - 2 * unit,
                cy - 2 * unit,
                (4 * unit) as u32,
                (4 * unit) as u32,
            ))?,
            Button::Close => {
                // Two diagonals, `s` pixels thick.
                let (x0, y0) = (cx - 2 * unit, cy - 2 * unit);
                let (x1, y1) = (cx + 2 * unit - s, cy + 2 * unit - 1);
                for k in 0..s {
                    canvas.draw_line((x0 + k, y0), (x1 + k, y1))?;
                    canvas.draw_line((x1 + k, y0), (x0 + k, y1))?;
                }
            }
        }
    }
    canvas.set_blend_mode(BlendMode::None);
    Ok(())
}

/// Window icon: "MD" in the bar font, white on a dark blue square.
pub fn set_icon(window: &mut Window) -> Result<(), String> {
    const BACKGROUND: [u8; 4] = [20, 30, 90, 255];
    const FOREGROUND: [u8; 4] = [240, 240, 255, 255];
    let size = ICON_SIZE as usize;
    let mut pixels = [BACKGROUND; ICON_SIZE as usize * ICON_SIZE as usize].concat();
    // Two glyphs scaled ×2 fill the width; centred vertically.
    let (scale, top) = (2, (size - 2 * chrome::GLYPH as usize) / 2);
    for (x, y, len) in chrome::text_runs("MD") {
        for dy in 0..scale {
            for dx in 0..len as usize * scale {
                let (px, py) = (x as usize * scale + dx, top + y as usize * scale + dy);
                let i = 4 * (py * size + px);
                pixels[i..i + 4].copy_from_slice(&FOREGROUND);
            }
        }
    }
    let surface = Surface::from_data(
        &mut pixels,
        ICON_SIZE,
        ICON_SIZE,
        4 * ICON_SIZE,
        PixelFormatEnum::RGBA32,
    )?;
    window.set_icon(surface);
    Ok(())
}

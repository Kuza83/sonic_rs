//! The Game window: renderer and the streaming texture holding the
//! picture. Nothing else is drawn there (the bar, menu and messages are in
//! the Library window, `sdl::library`).

use md_core::Console;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::video::{FullscreenType, Window, WindowContext};
use vdp::{FRAME_WIDTH, MAX_FRAME_HEIGHT};

use super::chrome::WindowKind;
use super::sdl_error;
use crate::chrome::{Layout, Rect as Area};

/// Picture of a save state, for the previews of the Library window.
pub struct Preview {
    pub pixels: Vec<u32>,
    pub width: usize,
    pub height: usize,
}

/// A window at `area` (position and size), titled `title`: borderless with
/// the hit-test rules of `kind`, or with the system decorations if
/// `decorated`; presentation synchronised with the display if `vsync`.
pub fn window_canvas(
    context: &sdl2::Sdl,
    title: &str,
    area: Area,
    vsync: bool,
    decorated: bool,
    kind: WindowKind,
) -> anyhow::Result<Canvas<Window>> {
    let video = context.video().map_err(sdl_error)?;
    let mut builder = video.window(title, area.w, area.h);
    builder.position(area.x, area.y).resizable();
    if !decorated {
        builder.borderless();
    }
    let mut window = builder.build()?;
    if let Err(e) = super::chrome::set_icon(&mut window) {
        eprintln!("window icon: {e}");
    }
    if !decorated && let Err(e) = super::chrome::install_hit_test(&window, kind) {
        eprintln!("window moving/resizing unavailable ({e}); use --decorated for the system bar");
    }
    let mut builder = window.into_canvas();
    if vsync {
        builder = builder.present_vsync();
    }
    Ok(builder.build()?)
}

/// Layout of a window as it is now (fullscreen: no resize bands) for a
/// picture of `lines` active lines.
pub fn layout(canvas: &Canvas<Window>, lines: usize) -> Layout {
    let window = canvas.window();
    let (w, h) = window.size();
    Layout::with_lines(
        w.max(1),
        h.max(1),
        window.fullscreen_state() == FullscreenType::Off,
        lines as u32,
    )
}

/// The window's position and size on screen, as it is now.
pub fn window_rect(canvas: &Canvas<Window>) -> Area {
    let window = canvas.window();
    let (x, y) = window.position();
    let (w, h) = window.size();
    Area { x, y, w, h }
}

pub fn is_fullscreen(canvas: &Canvas<Window>) -> bool {
    canvas.window().fullscreen_state() != FullscreenType::Off
}

/// The window's position and size, when it is not fullscreen (and not
/// minimized: its size then means nothing).
pub fn windowed_area(canvas: &Canvas<Window>) -> Option<Area> {
    let window = canvas.window();
    if is_fullscreen(canvas) || window.is_minimized() {
        return None;
    }
    let (x, y) = window.position();
    let (w, h) = window.size();
    (w > 0 && h > 0).then_some(Area { x, y, w, h })
}

pub fn toggle_fullscreen(canvas: &mut Canvas<Window>) -> anyhow::Result<()> {
    let window = canvas.window_mut();
    let next = match window.fullscreen_state() {
        FullscreenType::Off => FullscreenType::Desktop,
        _ => FullscreenType::Off,
    };
    window.set_fullscreen(next).map_err(sdl_error)
}

/// The Game window's picture.
pub struct Screen<'a> {
    texture: Texture<'a>,
}

impl<'a> Screen<'a> {
    pub fn new(creator: &'a TextureCreator<WindowContext>) -> anyhow::Result<Self> {
        // RGBA8888 is a packed 32-bit format: a native-endian u32 0xRRGGBBAA,
        // exactly the core's framebuffer pixels.
        let texture = creator.create_texture_streaming(
            PixelFormatEnum::RGBA8888,
            FRAME_WIDTH as u32,
            MAX_FRAME_HEIGHT as u32,
        )?;
        Ok(Self { texture })
    }

    /// The picture of `console` at the 320:224 (or 240) aspect over black,
    /// or a black window without a game.
    pub fn present(
        &mut self,
        canvas: &mut Canvas<Window>,
        console: Option<&Console>,
    ) -> anyhow::Result<()> {
        canvas.set_draw_color(sdl2::pixels::Color::BLACK);
        canvas.clear();
        if let Some(console) = console {
            let width = console.screen_width();
            self.texture
                .with_lock(None, |pixels, pitch| {
                    copy_frame(console.framebuffer(), width, pixels, pitch)
                })
                .map_err(sdl_error)?;
            let source = Rect::new(0, 0, width as u32, console.screen_height() as u32);
            let p = layout(canvas, console.screen_height()).picture;
            canvas
                .copy(
                    &self.texture,
                    source,
                    Rect::new(p.x, p.y, p.w.max(1), p.h.max(1)),
                )
                .map_err(sdl_error)?;
        }
        canvas.present();
        Ok(())
    }
}

/// Copies the `width` visible pixels of each framebuffer row (stride
/// `FRAME_WIDTH`) into texture memory with `pitch` bytes per row.
pub fn copy_frame(framebuffer: &[u32], width: usize, pixels: &mut [u8], pitch: usize) {
    for (row, line) in framebuffer
        .as_chunks::<FRAME_WIDTH>()
        .0
        .iter()
        .zip(pixels.chunks_mut(pitch))
    {
        for (pixel, out) in row[..width].iter().zip(line.as_chunks_mut::<4>().0) {
            out.copy_from_slice(&pixel.to_ne_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vdp::FRAME_HEIGHT;

    #[test]
    fn copy_frame_keeps_pixels_rows_and_padding() {
        let mut framebuffer = vec![0u32; FRAME_WIDTH * FRAME_HEIGHT];
        framebuffer[0] = 0x1122_33FF;
        framebuffer[FRAME_WIDTH + 255] = 0xAABB_CCFF;
        framebuffer[FRAME_WIDTH + 256] = 0xDEAD_BEEF; // outside H32
        let pitch = FRAME_WIDTH * 4 + 16; // drivers may pad rows
        let mut pixels = vec![0x55u8; pitch * FRAME_HEIGHT];
        copy_frame(&framebuffer, 256, &mut pixels, pitch);
        let at = |offset: usize| u32::from_ne_bytes(pixels[offset..offset + 4].try_into().unwrap());
        assert_eq!(at(0), 0x1122_33FF);
        assert_eq!(at(pitch + 255 * 4), 0xAABB_CCFF);
        assert_eq!(at(pitch + 256 * 4), 0x5555_5555, "beyond the visible width");
        assert_eq!(at(FRAME_WIDTH * 4), 0x5555_5555, "row padding untouched");
    }
}

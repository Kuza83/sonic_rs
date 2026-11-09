//! Window chrome of the borderless window mode, independent of SDL: layout
//! of the picture and of our own title bar, hit testing (drag, resize,
//! buttons), auto-hiding, the bitmap font of the bar and the game title.

use std::path::Path;
use std::time::Duration;

/// Picture aspect: 320 × 224, or 320 × 240 in V30 (H32 pictures are
/// stretched to the same width).
const ASPECT_W: u32 = 320;
const ASPECT_H: u32 = 224;
/// Bar height and button width per text scale step, in window pixels.
const BAR_UNIT: u32 = 12;
const BUTTON_UNIT: u32 = 18;
/// Width of the resize band along the window edges.
pub const RESIZE_BAND: i32 = 6;
/// Glyph size of the bar font.
pub const GLYPH: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w as i32 && y < self.y + self.h as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Fullscreen,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The picture: no window action.
    Normal,
    /// The bar outside its buttons: moves the window.
    Drag,
    Button(Button),
    Resize(Edge),
}

/// Geometry of the window contents for a window of `width` × `height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// The picture, at the 320:224 aspect, centred (black bands around).
    pub picture: Rect,
    /// Our title bar, over the top of the window.
    pub bar: Rect,
    /// Buttons at the right of the bar, left to right.
    pub buttons: [(Button, Rect); 3],
    /// Text scale of the bar (1-3, from the window height).
    pub scale: u32,
    /// Edges resize the window (not in fullscreen).
    pub resizable: bool,
}

impl Layout {
    pub fn new(width: u32, height: u32, resizable: bool) -> Self {
        Self::with_lines(width, height, resizable, ASPECT_H)
    }

    /// Layout for a picture of `lines` active lines (224, or 240 in V30).
    pub fn with_lines(width: u32, height: u32, resizable: bool, lines: u32) -> Self {
        let (pw, ph) = if width as u64 * lines as u64 >= height as u64 * ASPECT_W as u64 {
            (height * ASPECT_W / lines, height)
        } else {
            (width, width * lines / ASPECT_W)
        };
        let picture = Rect {
            x: ((width - pw) / 2) as i32,
            y: ((height - ph) / 2) as i32,
            w: pw,
            h: ph,
        };
        let scale = (height / 300).clamp(1, 3);
        let bar = Rect {
            x: 0,
            y: 0,
            w: width,
            h: BAR_UNIT * scale,
        };
        let bw = BUTTON_UNIT * scale;
        let button = |i: u32| Rect {
            x: width as i32 - ((3 - i) * bw) as i32,
            y: 0,
            w: bw,
            h: bar.h,
        };
        Self {
            width,
            height,
            picture,
            bar,
            buttons: [
                (Button::Minimize, button(0)),
                (Button::Fullscreen, button(1)),
                (Button::Close, button(2)),
            ],
            scale,
            resizable,
        }
    }

    /// What is at window pixel (`x`, `y`): resize bands first (corners
    /// before sides), then the buttons, then the rest of the bar.
    pub fn hit(&self, x: i32, y: i32) -> Hit {
        if let Some(edge) = self.edge(x, y) {
            return Hit::Resize(edge);
        }
        if let Some((button, _)) = self.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            return Hit::Button(*button);
        }
        if self.bar.contains(x, y) {
            return Hit::Drag;
        }
        Hit::Normal
    }

    /// The game window has no bar: the whole picture drags it, the edges
    /// resize it.
    pub fn game_hit(&self, x: i32, y: i32) -> Hit {
        self.edge(x, y).map_or(Hit::Drag, Hit::Resize)
    }

    /// Resize band under (`x`, `y`), corners first.
    fn edge(&self, x: i32, y: i32) -> Option<Edge> {
        if self.resizable {
            let (w, h) = (self.width as i32, self.height as i32);
            let (left, right) = (x < RESIZE_BAND, x >= w - RESIZE_BAND);
            let (top, bottom) = (y < RESIZE_BAND, y >= h - RESIZE_BAND);
            let edge = match (top, bottom, left, right) {
                (true, _, true, _) => Some(Edge::TopLeft),
                (true, _, _, true) => Some(Edge::TopRight),
                (_, true, true, _) => Some(Edge::BottomLeft),
                (_, true, _, true) => Some(Edge::BottomRight),
                (true, ..) => Some(Edge::Top),
                (_, true, ..) => Some(Edge::Bottom),
                (_, _, true, _) => Some(Edge::Left),
                (_, _, _, true) => Some(Edge::Right),
                _ => None,
            };
            return edge;
        }
        None
    }

    /// Top-left of the title text inside the bar.
    pub fn title_origin(&self) -> (i32, i32) {
        let pad = ((self.bar.h - GLYPH * self.scale) / 2) as i32;
        (pad * 2, pad)
    }
}

/// Whether a mouse release at (`x`, `y`) in the coordinates of the window
/// at `from` (on screen) falls on the window at `target`. While a button is
/// held SDL keeps sending the mouse to the window where it was pressed,
/// with coordinates that may lie outside it, hence the conversion to screen
/// coordinates.
pub fn released_over(from: Rect, x: i32, y: i32, target: Rect) -> bool {
    target.contains(from.x + x, from.y + y)
}

/// A window's remembered area if it is still usable: its centre on one of
/// `displays` (a display unplugged since moves it back to the automatic
/// placement), its size cut down to that display.
pub fn restore_area(saved: Rect, displays: &[Rect]) -> Option<Rect> {
    let cx = i64::from(saved.x) + i64::from(saved.w) / 2;
    let cy = i64::from(saved.y) + i64::from(saved.h) / 2;
    let display = displays.iter().find(|d| {
        (i64::from(d.x)..i64::from(d.x) + i64::from(d.w)).contains(&cx)
            && (i64::from(d.y)..i64::from(d.y) + i64::from(d.h)).contains(&cy)
    })?;
    Some(Rect {
        w: saved.w.min(display.w),
        h: saved.h.min(display.h),
        ..saved
    })
}

/// Where the Game and Library windows open, given the displays' bounds
/// (first = primary): one display each when there are two or more, else
/// side by side on the only one (overlapping if it is too narrow). Sizes
/// are kept; positions are centred.
pub fn place_windows(displays: &[Rect], game: (u32, u32), library: (u32, u32)) -> (Rect, Rect) {
    let centred = |d: &Rect, (w, h): (u32, u32)| Rect {
        x: d.x + (d.w as i32 - w as i32) / 2,
        y: d.y + (d.h as i32 - h as i32) / 2,
        w,
        h,
    };
    match displays {
        [] => (
            Rect {
                x: 0,
                y: 0,
                w: game.0,
                h: game.1,
            },
            Rect {
                x: game.0 as i32,
                y: 0,
                w: library.0,
                h: library.1,
            },
        ),
        [only] => {
            let pair = Rect {
                w: game.0 + library.0,
                h: game.1.max(library.1),
                ..*only
            };
            let start = centred(only, (pair.w, pair.h));
            (
                Rect {
                    x: start.x,
                    y: start.y + (pair.h - game.1) as i32 / 2,
                    w: game.0,
                    h: game.1,
                },
                Rect {
                    x: start.x + game.0 as i32,
                    y: start.y + (pair.h - library.1) as i32 / 2,
                    w: library.0,
                    h: library.1,
                },
            )
        }
        [first, second, ..] => (centred(first, game), centred(second, library)),
    }
}

/// On-screen message ("State 3 saved"): the latest one stays visible for
/// `MESSAGE_DURATION`.
pub const MESSAGE_DURATION: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Default)]
pub struct Messages {
    latest: Option<(String, Duration)>,
}

impl Messages {
    /// Shows `text` from time `now` (replacing the previous message).
    pub fn show(&mut self, text: impl Into<String>, now: Duration) {
        self.latest = Some((text.into(), now));
    }

    /// Removes the message shown (an action given up).
    pub fn clear(&mut self) {
        self.latest = None;
    }

    /// The message to draw at time `now`, if any.
    pub fn current(&self, now: Duration) -> Option<&str> {
        self.latest
            .as_ref()
            .filter(|(_, since)| now.saturating_sub(*since) < MESSAGE_DURATION)
            .map(|(text, _)| text.as_str())
    }
}

/// Frames per second actually shown, measured over windows of one second.
#[derive(Debug, Clone, Default)]
pub struct FpsMeter {
    window_start: Option<Duration>,
    frames: u32,
    fps: Option<f64>,
}

impl FpsMeter {
    const WINDOW: Duration = Duration::from_secs(1);

    /// A frame was presented at time `now`.
    pub fn frame(&mut self, now: Duration) {
        let start = *self.window_start.get_or_insert(now);
        self.frames += 1;
        let elapsed = now.saturating_sub(start);
        if elapsed >= Self::WINDOW {
            self.fps = Some(self.frames as f64 / elapsed.as_secs_f64());
            self.window_start = Some(now);
            self.frames = 0;
        }
    }

    /// Rate over the last complete window (none during the first second).
    pub fn fps(&self) -> Option<f64> {
        self.fps
    }
}

/// Horizontal runs of lit pixels of `text` in the bar font, unscaled:
/// (x, y, length), x and y in font pixels from the top-left of the text.
/// Characters outside ASCII 32-126 are drawn as '?'.
pub fn text_runs(text: &str) -> Vec<(i32, i32, u32)> {
    let mut runs = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let glyph = glyph(c);
        for (y, row) in glyph.iter().enumerate() {
            let mut x = 0;
            while x < GLYPH {
                if row >> x & 1 == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < GLYPH && row >> x & 1 != 0 {
                    x += 1;
                }
                runs.push(((i as u32 * GLYPH + start) as i32, y as i32, x - start));
            }
        }
    }
    runs
}

/// Width of `text` in font pixels.
pub fn text_width(text: &str) -> u32 {
    text.chars().count() as u32 * GLYPH
}

fn glyph(c: char) -> &'static [u8; 8] {
    let index = match c {
        ' '..='~' => c as usize - ' ' as usize,
        _ => '?' as usize - ' ' as usize,
    };
    &GLYPHS[index]
}

/// Title shown in the bar and given to the window: the overseas title of
/// the header, else the domestic one, else the file name; runs of spaces
/// (headers pad with them) collapse to one.
pub fn game_title(overseas: &str, domestic: &str, rom: &Path) -> String {
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    [clean(overseas), clean(domestic)]
        .into_iter()
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| {
            rom.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "megadrive-rs".into())
        })
}

// Bar font: font8x8_basic by Daniel Hepper (github.com/dhepper/font8x8),
// public domain, from the IBM public domain VGA fonts.
/// ASCII 32-126, 8 rows each, bit 0 = leftmost pixel (font8x8_basic.h).
const GLYPHS: [[u8; 8]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // ' '
    [0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00], // '!'
    [0x36, 0x36, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // '"'
    [0x36, 0x36, 0x7F, 0x36, 0x7F, 0x36, 0x36, 0x00], // '#'
    [0x0C, 0x3E, 0x03, 0x1E, 0x30, 0x1F, 0x0C, 0x00], // '$'
    [0x00, 0x63, 0x33, 0x18, 0x0C, 0x66, 0x63, 0x00], // '%'
    [0x1C, 0x36, 0x1C, 0x6E, 0x3B, 0x33, 0x6E, 0x00], // '&'
    [0x06, 0x06, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00], // "'"
    [0x18, 0x0C, 0x06, 0x06, 0x06, 0x0C, 0x18, 0x00], // '('
    [0x06, 0x0C, 0x18, 0x18, 0x18, 0x0C, 0x06, 0x00], // ')'
    [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00], // '*'
    [0x00, 0x0C, 0x0C, 0x3F, 0x0C, 0x0C, 0x00, 0x00], // '+'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C, 0x06], // ','
    [0x00, 0x00, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00], // '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C, 0x00], // '.'
    [0x60, 0x30, 0x18, 0x0C, 0x06, 0x03, 0x01, 0x00], // '/'
    [0x3E, 0x63, 0x73, 0x7B, 0x6F, 0x67, 0x3E, 0x00], // '0'
    [0x0C, 0x0E, 0x0C, 0x0C, 0x0C, 0x0C, 0x3F, 0x00], // '1'
    [0x1E, 0x33, 0x30, 0x1C, 0x06, 0x33, 0x3F, 0x00], // '2'
    [0x1E, 0x33, 0x30, 0x1C, 0x30, 0x33, 0x1E, 0x00], // '3'
    [0x38, 0x3C, 0x36, 0x33, 0x7F, 0x30, 0x78, 0x00], // '4'
    [0x3F, 0x03, 0x1F, 0x30, 0x30, 0x33, 0x1E, 0x00], // '5'
    [0x1C, 0x06, 0x03, 0x1F, 0x33, 0x33, 0x1E, 0x00], // '6'
    [0x3F, 0x33, 0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x00], // '7'
    [0x1E, 0x33, 0x33, 0x1E, 0x33, 0x33, 0x1E, 0x00], // '8'
    [0x1E, 0x33, 0x33, 0x3E, 0x30, 0x18, 0x0E, 0x00], // '9'
    [0x00, 0x0C, 0x0C, 0x00, 0x00, 0x0C, 0x0C, 0x00], // ':'
    [0x00, 0x0C, 0x0C, 0x00, 0x00, 0x0C, 0x0C, 0x06], // ';'
    [0x18, 0x0C, 0x06, 0x03, 0x06, 0x0C, 0x18, 0x00], // '<'
    [0x00, 0x00, 0x3F, 0x00, 0x00, 0x3F, 0x00, 0x00], // '='
    [0x06, 0x0C, 0x18, 0x30, 0x18, 0x0C, 0x06, 0x00], // '>'
    [0x1E, 0x33, 0x30, 0x18, 0x0C, 0x00, 0x0C, 0x00], // '?'
    [0x3E, 0x63, 0x7B, 0x7B, 0x7B, 0x03, 0x1E, 0x00], // '@'
    [0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00], // 'A'
    [0x3F, 0x66, 0x66, 0x3E, 0x66, 0x66, 0x3F, 0x00], // 'B'
    [0x3C, 0x66, 0x03, 0x03, 0x03, 0x66, 0x3C, 0x00], // 'C'
    [0x1F, 0x36, 0x66, 0x66, 0x66, 0x36, 0x1F, 0x00], // 'D'
    [0x7F, 0x46, 0x16, 0x1E, 0x16, 0x46, 0x7F, 0x00], // 'E'
    [0x7F, 0x46, 0x16, 0x1E, 0x16, 0x06, 0x0F, 0x00], // 'F'
    [0x3C, 0x66, 0x03, 0x03, 0x73, 0x66, 0x7C, 0x00], // 'G'
    [0x33, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x33, 0x00], // 'H'
    [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // 'I'
    [0x78, 0x30, 0x30, 0x30, 0x33, 0x33, 0x1E, 0x00], // 'J'
    [0x67, 0x66, 0x36, 0x1E, 0x36, 0x66, 0x67, 0x00], // 'K'
    [0x0F, 0x06, 0x06, 0x06, 0x46, 0x66, 0x7F, 0x00], // 'L'
    [0x63, 0x77, 0x7F, 0x7F, 0x6B, 0x63, 0x63, 0x00], // 'M'
    [0x63, 0x67, 0x6F, 0x7B, 0x73, 0x63, 0x63, 0x00], // 'N'
    [0x1C, 0x36, 0x63, 0x63, 0x63, 0x36, 0x1C, 0x00], // 'O'
    [0x3F, 0x66, 0x66, 0x3E, 0x06, 0x06, 0x0F, 0x00], // 'P'
    [0x1E, 0x33, 0x33, 0x33, 0x3B, 0x1E, 0x38, 0x00], // 'Q'
    [0x3F, 0x66, 0x66, 0x3E, 0x36, 0x66, 0x67, 0x00], // 'R'
    [0x1E, 0x33, 0x07, 0x0E, 0x38, 0x33, 0x1E, 0x00], // 'S'
    [0x3F, 0x2D, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // 'T'
    [0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x3F, 0x00], // 'U'
    [0x33, 0x33, 0x33, 0x33, 0x33, 0x1E, 0x0C, 0x00], // 'V'
    [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00], // 'W'
    [0x63, 0x63, 0x36, 0x1C, 0x1C, 0x36, 0x63, 0x00], // 'X'
    [0x33, 0x33, 0x33, 0x1E, 0x0C, 0x0C, 0x1E, 0x00], // 'Y'
    [0x7F, 0x63, 0x31, 0x18, 0x4C, 0x66, 0x7F, 0x00], // 'Z'
    [0x1E, 0x06, 0x06, 0x06, 0x06, 0x06, 0x1E, 0x00], // '['
    [0x03, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x40, 0x00], // '\\'
    [0x1E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x1E, 0x00], // ']'
    [0x08, 0x1C, 0x36, 0x63, 0x00, 0x00, 0x00, 0x00], // '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF], // '_'
    [0x0C, 0x0C, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00], // '`'
    [0x00, 0x00, 0x1E, 0x30, 0x3E, 0x33, 0x6E, 0x00], // 'a'
    [0x07, 0x06, 0x06, 0x3E, 0x66, 0x66, 0x3B, 0x00], // 'b'
    [0x00, 0x00, 0x1E, 0x33, 0x03, 0x33, 0x1E, 0x00], // 'c'
    [0x38, 0x30, 0x30, 0x3E, 0x33, 0x33, 0x6E, 0x00], // 'd'
    [0x00, 0x00, 0x1E, 0x33, 0x3F, 0x03, 0x1E, 0x00], // 'e'
    [0x1C, 0x36, 0x06, 0x0F, 0x06, 0x06, 0x0F, 0x00], // 'f'
    [0x00, 0x00, 0x6E, 0x33, 0x33, 0x3E, 0x30, 0x1F], // 'g'
    [0x07, 0x06, 0x36, 0x6E, 0x66, 0x66, 0x67, 0x00], // 'h'
    [0x0C, 0x00, 0x0E, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // 'i'
    [0x30, 0x00, 0x30, 0x30, 0x30, 0x33, 0x33, 0x1E], // 'j'
    [0x07, 0x06, 0x66, 0x36, 0x1E, 0x36, 0x67, 0x00], // 'k'
    [0x0E, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x1E, 0x00], // 'l'
    [0x00, 0x00, 0x33, 0x7F, 0x7F, 0x6B, 0x63, 0x00], // 'm'
    [0x00, 0x00, 0x1F, 0x33, 0x33, 0x33, 0x33, 0x00], // 'n'
    [0x00, 0x00, 0x1E, 0x33, 0x33, 0x33, 0x1E, 0x00], // 'o'
    [0x00, 0x00, 0x3B, 0x66, 0x66, 0x3E, 0x06, 0x0F], // 'p'
    [0x00, 0x00, 0x6E, 0x33, 0x33, 0x3E, 0x30, 0x78], // 'q'
    [0x00, 0x00, 0x3B, 0x6E, 0x66, 0x06, 0x0F, 0x00], // 'r'
    [0x00, 0x00, 0x3E, 0x03, 0x1E, 0x30, 0x1F, 0x00], // 's'
    [0x08, 0x0C, 0x3E, 0x0C, 0x0C, 0x2C, 0x18, 0x00], // 't'
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x33, 0x6E, 0x00], // 'u'
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x1E, 0x0C, 0x00], // 'v'
    [0x00, 0x00, 0x63, 0x6B, 0x7F, 0x7F, 0x36, 0x00], // 'w'
    [0x00, 0x00, 0x63, 0x36, 0x1C, 0x36, 0x63, 0x00], // 'x'
    [0x00, 0x00, 0x33, 0x33, 0x33, 0x3E, 0x30, 0x1F], // 'y'
    [0x00, 0x00, 0x3F, 0x19, 0x0C, 0x26, 0x3F, 0x00], // 'z'
    [0x38, 0x0C, 0x0C, 0x07, 0x0C, 0x0C, 0x38, 0x00], // '{'
    [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // '|'
    [0x07, 0x0C, 0x0C, 0x38, 0x0C, 0x0C, 0x07, 0x00], // '}'
    [0x6E, 0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // '~'
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_onto_the_other_window() {
        let library = Rect {
            x: 0,
            y: 0,
            w: 900,
            h: 640,
        };
        let game = Rect {
            x: 1920,
            y: 100,
            w: 960,
            h: 720,
        };
        assert!(
            released_over(library, 2000, 300, game),
            "dragged to the right display"
        );
        assert!(
            !released_over(library, 700, 300, game),
            "released in the Library"
        );
        assert!(
            !released_over(library, 2000, 900, game),
            "below the game window"
        );
        let moved = Rect { x: 1000, ..library };
        assert!(
            released_over(moved, 1000, 200, game),
            "the Library's own position counts"
        );
    }

    #[test]
    fn remembered_window_areas() {
        let left = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let right = Rect {
            x: 1920,
            y: 0,
            w: 2560,
            h: 1440,
        };
        let saved = Rect {
            x: 2000,
            y: 100,
            w: 960,
            h: 720,
        };
        assert_eq!(
            restore_area(saved, &[left, right]),
            Some(saved),
            "on the right display"
        );
        assert_eq!(
            restore_area(saved, &[left]),
            None,
            "right display unplugged"
        );
        let big = Rect {
            x: 10,
            y: 10,
            w: 3000,
            h: 2000,
        };
        assert_eq!(
            restore_area(big, &[left, right]),
            Some(Rect {
                x: 10,
                y: 10,
                w: 1920,
                h: 1080
            }),
            "centre at (1510, 1010) is on the left display, size cut to it"
        );
    }

    #[test]
    fn picture_keeps_the_aspect_and_is_centred() {
        // Exact 3× window: no bands.
        let l = Layout::new(960, 672, true);
        assert_eq!(
            l.picture,
            Rect {
                x: 0,
                y: 0,
                w: 960,
                h: 672
            }
        );
        // Wider window: bands left and right.
        let l = Layout::new(1920, 1080, false);
        assert_eq!(l.picture.h, 1080);
        assert_eq!(l.picture.w, 1080 * 320 / 224);
        assert_eq!(l.picture.x, ((1920 - l.picture.w) / 2) as i32);
        // Taller window: bands above and below.
        let l = Layout::new(640, 1000, true);
        assert_eq!((l.picture.w, l.picture.h), (640, 448));
        assert_eq!(l.picture.y, (1000 - 448) / 2);
    }

    #[test]
    fn v30_picture_is_taller() {
        // 960 × 720 window: a 240-line picture fills it, a 224-line one is
        // letterboxed above and below.
        let v30 = Layout::with_lines(960, 720, true, 240);
        assert_eq!(
            v30.picture,
            Rect {
                x: 0,
                y: 0,
                w: 960,
                h: 720
            }
        );
        let v28 = Layout::with_lines(960, 720, true, 224);
        assert_eq!((v28.picture.w, v28.picture.h), (960, 672));
        assert_eq!(v28.picture.y, 24);
    }

    #[test]
    fn bar_buttons_drag_and_resize_zones() {
        let l = Layout::new(960, 672, true);
        assert_eq!(l.scale, 2);
        assert_eq!(l.bar.h, 24);
        let (_, close) = l.buttons[2];
        assert_eq!(close.x + close.w as i32, 960, "close button at the right");
        assert_eq!(l.hit(close.x + 5, 10), Hit::Button(Button::Close));
        let expected = [Button::Minimize, Button::Fullscreen];
        for (i, button) in expected.into_iter().enumerate() {
            assert_eq!(l.hit(l.buttons[i].1.x + 5, 10), Hit::Button(button));
        }
        assert_eq!(l.hit(300, 10), Hit::Drag);
        assert_eq!(l.hit(300, 300), Hit::Normal);
        assert_eq!(l.hit(2, 2), Hit::Resize(Edge::TopLeft), "corners first");
        assert_eq!(l.hit(958, 670), Hit::Resize(Edge::BottomRight));
        assert_eq!(l.hit(300, 2), Hit::Resize(Edge::Top));
        assert_eq!(
            l.hit(959, 10),
            Hit::Resize(Edge::Right),
            "edge before the close button"
        );
        assert_eq!(l.hit(1, 300), Hit::Resize(Edge::Left));
        // Fullscreen: no resize bands.
        let l = Layout::new(960, 672, false);
        assert_eq!(l.hit(2, 2), Hit::Drag);
        assert_eq!(l.hit(1, 300), Hit::Normal);
    }

    #[test]
    fn game_window_drags_anywhere_and_resizes_by_its_edges() {
        let l = Layout::new(960, 672, true);
        assert_eq!(
            l.game_hit(480, 336),
            Hit::Drag,
            "the picture moves the window"
        );
        assert_eq!(l.game_hit(959, 10), Hit::Resize(Edge::Right));
        assert_eq!(l.game_hit(2, 670), Hit::Resize(Edge::BottomLeft));
        let full = Layout::new(960, 672, false);
        assert_eq!(
            full.game_hit(0, 0),
            Hit::Drag,
            "no resize bands in fullscreen"
        );
    }

    #[test]
    fn windows_go_one_per_display_or_side_by_side() {
        let left = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let right = Rect {
            x: 1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let (game, library) = place_windows(&[left, right], (960, 672), (900, 640));
        assert!(left.contains(game.x, game.y) && left.contains(game.x + 959, game.y + 671));
        assert!(right.contains(library.x, library.y));
        assert_eq!(
            (game.x, game.y),
            ((1920 - 960) / 2, (1080 - 672) / 2),
            "centred"
        );
        let (game, library) = place_windows(&[left], (960, 672), (900, 640));
        assert_eq!(library.x, game.x + 960, "side by side");
        assert!(left.contains(game.x, game.y) && left.contains(library.x + 899, library.y));
    }

    #[test]
    fn bar_scales_with_the_window() {
        assert_eq!(Layout::new(320, 224, true).scale, 1);
        assert_eq!(Layout::new(1600, 1200, true).scale, 3);
        assert_eq!(Layout::new(3840, 2160, true).scale, 3, "capped");
        let l = Layout::new(320, 224, true);
        let (x, y) = l.title_origin();
        assert_eq!((x, y), (4, 2), "text centred vertically in a 12-pixel bar");
    }

    #[test]
    fn messages_expire() {
        let t = Duration::from_millis;
        let mut m = Messages::default();
        assert_eq!(m.current(t(0)), None);
        m.show("State 3 saved", t(1000));
        assert_eq!(m.current(t(1000)), Some("State 3 saved"));
        assert_eq!(m.current(t(3499)), Some("State 3 saved"));
        assert_eq!(m.current(t(3500)), None);
        m.show("Slot 4", t(2000));
        m.show("Reset", t(2100));
        assert_eq!(m.current(t(2200)), Some("Reset"), "the latest replaces");
    }

    #[test]
    fn fps_over_one_second_windows() {
        let mut meter = FpsMeter::default();
        let frame = Duration::from_micros(16_688); // NTSC
        for i in 0..=60 {
            meter.frame(frame * i);
            if i < 60 {
                assert_eq!(meter.fps(), None, "no full second yet ({i})");
            }
        }
        let fps = meter.fps().unwrap();
        assert!((fps - 61.0 / (60.0 * 0.016_688)).abs() < 0.01, "{fps}");
    }

    #[test]
    fn text_runs_follow_the_font() {
        // 'A' row 4 is 0x3F: pixels 0-5 lit, one run of 6.
        let runs = text_runs("A");
        assert!(runs.contains(&(0, 4, 6)));
        let lit: u32 = runs.iter().map(|r| r.2).sum();
        let expected: u32 = GLYPHS['A' as usize - 32]
            .iter()
            .map(|r| r.count_ones())
            .sum();
        assert_eq!(lit, expected);
        // Second character starts 8 pixels further.
        assert!(text_runs("AA").iter().any(|r| r.0 >= 8));
        assert!(text_runs(" ").is_empty());
        assert_eq!(text_runs("é"), text_runs("?"), "unknown characters as '?'");
        assert_eq!(text_width("SONIC"), 40);
    }

    #[test]
    fn title_from_the_header() {
        let rom = Path::new("roms/test/game.bin");
        assert_eq!(
            game_title("SONIC THE      HEDGEHOG  ", "", rom),
            "SONIC THE HEDGEHOG"
        );
        assert_eq!(game_title("   ", "DOMESTIC NAME", rom), "DOMESTIC NAME");
        assert_eq!(game_title("", " ", rom), "game");
    }
}

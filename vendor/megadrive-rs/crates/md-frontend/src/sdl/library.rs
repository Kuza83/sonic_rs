//! The Library window: our bar, the ROM list with its filter, the preview of
//! the selected game, the status line (messages, fps) and the menu.
//! Geometry and behaviour come from `crate::library`, `crate::chrome` and
//! `crate::menu` (unit-tested without SDL).

use std::collections::HashMap;
use std::path::PathBuf;

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, Texture, TextureCreator};
use sdl2::video::{Window, WindowContext};
use vdp::{FRAME_WIDTH, MAX_FRAME_HEIGHT};

use super::chrome::{BAR_COLOR, HOVER_COLOR, draw_bar, draw_text_at};
use super::sdl_error;
use super::video::{Preview, copy_frame};
use crate::chrome::{GLYPH, Hit, Layout, Rect as Area, text_width};
use crate::library::{Library, LibraryLayout, fit_picture};

const BACKGROUND: Color = Color::RGB(24, 24, 32);
const FIELD_COLOR: Color = Color::RGB(40, 40, 52);
const DIM_TEXT: Color = Color::RGBA(150, 150, 165, 255);
/// Menu items that do not apply now.
const GREYED_TEXT: Color = Color::RGBA(90, 90, 100, 255);

/// The open menu, ready to draw.
pub struct MenuView<'t> {
    pub layout: crate::menu::MenuLayout,
    /// What the tab's items apply to (game running, game selected).
    pub subtitle: String,
    pub rows: Vec<crate::menu::RowView>,
    /// The slot page, when open.
    pub slots: Option<SlotsView>,
    /// Picture of the selected slot's state (none when the slot is empty).
    pub preview: Option<&'t Preview>,
}

/// The save state slot page.
pub struct SlotsView {
    pub action: crate::menu::SlotAction,
    pub used: [bool; crate::menu::SLOTS as usize],
    pub selected: u8,
}

/// The cover chooser, drawn over the list.
pub struct PickerView<'t> {
    pub picker: &'t crate::coverpick::CoverPicker,
    /// Shown instead of the picture: "Loading picture...", "No picture".
    pub picture_text: &'t str,
    /// The user's own picture next to the ROM wins over the choice.
    pub own_cover: bool,
}

/// The cover grid (in place of the list, picture and info panel).
pub struct GridView<'t> {
    pub grid: crate::library::Grid,
    /// ROM -> (version, thumbnail or None without a cover); a game not in
    /// it yet is still being prepared.
    pub thumbs: &'t HashMap<PathBuf, (u64, Option<Preview>)>,
}

/// Everything the Library window shows this frame.
pub struct LibraryView<'t> {
    pub chrome: Layout,
    pub layout: LibraryLayout,
    /// Draw our bar (not with `--decorated`).
    pub bar: bool,
    pub title: &'t str,
    pub hover: Hit,
    pub library: &'t Library,
    pub scanning: bool,
    /// Picture of the selected game: its cover, else its latest state.
    pub preview: Option<&'t Preview>,
    /// Changes when `preview` changes (the picture is uploaded only then).
    pub preview_generation: u64,
    /// Status line: message (or the default text) and fps.
    pub status: String,
    pub fps: Option<f64>,
    pub grid: Option<GridView<'t>>,
    pub view_mode: crate::library::ViewMode,
    /// The menu tabs, always shown: their cells and states.
    pub tab_cells: Vec<Area>,
    pub tabs: Vec<crate::menu::TabView>,
    /// The open menu, dropped below its tab.
    pub menu: Option<MenuView<'t>>,
    pub picker: Option<PickerView<'t>>,
}

pub struct LibraryScreen<'a> {
    creator: &'a TextureCreator<WindowContext>,
    /// The selected game's picture at its own size, and the generation
    /// it was uploaded for.
    picture: Option<(Texture<'a>, u32, u32)>,
    picture_generation: Option<u64>,
    menu_preview: Texture<'a>,
    /// Grid thumbnails uploaded, with the version they were made from.
    thumb_textures: HashMap<PathBuf, (u64, Texture<'a>)>,
}

/// Thumbnail textures kept before the cache is emptied.
const MAX_THUMB_TEXTURES: usize = 400;
const SELECTED_FRAME: Color = Color::RGB(240, 240, 250);

fn rect(r: Area) -> Rect {
    Rect::new(r.x, r.y, r.w.max(1), r.h.max(1))
}

/// `text` in lines of at most `max_px` pixels at `scale`, broken between
/// words (a word too long is cut by `fit`).
fn wrap(text: &str, max_px: u32, scale: u32) -> Vec<String> {
    let max_chars = (max_px / (GLYPH * scale)).max(1) as usize;
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max_chars {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(|l| fit(&l, max_px, scale)).collect()
}

/// `text` cut to `max_px` pixels wide at `scale` (whole characters).
fn fit(text: &str, max_px: u32, scale: u32) -> String {
    let max_chars = (max_px / (GLYPH * scale)) as usize;
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('~');
        cut
    }
}

impl<'a> LibraryScreen<'a> {
    pub fn new(creator: &'a TextureCreator<WindowContext>) -> anyhow::Result<Self> {
        Ok(Self {
            creator,
            picture: None,
            picture_generation: None,
            thumb_textures: HashMap::new(),
            menu_preview: creator.create_texture_streaming(
                PixelFormatEnum::RGBA8888,
                FRAME_WIDTH as u32,
                MAX_FRAME_HEIGHT as u32,
            )?,
        })
    }

    /// A texture holding `preview`, smoothly scaled (pictures are drawn at
    /// other sizes than theirs).
    fn smooth_texture(&self, preview: &Preview) -> anyhow::Result<Texture<'a>> {
        let (w, h) = (preview.width as u32, preview.height as u32);
        let previous = sdl2::hint::get("SDL_RENDER_SCALE_QUALITY");
        sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "linear");
        let texture = self
            .creator
            .create_texture_static(PixelFormatEnum::RGBA8888, w, h);
        sdl2::hint::set(
            "SDL_RENDER_SCALE_QUALITY",
            previous.as_deref().unwrap_or("nearest"),
        );
        let mut texture = texture?;
        texture.set_blend_mode(BlendMode::Blend);
        let bytes: Vec<u8> = preview
            .pixels
            .iter()
            .flat_map(|p| p.to_ne_bytes())
            .collect();
        texture.update(None, &bytes, w as usize * 4)?;
        Ok(texture)
    }

    /// The cover grid: each game on screen as its thumbnail, or a frame with
    /// its title when it has no cover; the selected one framed.
    fn draw_grid(
        &mut self,
        canvas: &mut Canvas<Window>,
        library: &Library,
        grid: &GridView,
        scale: u32,
    ) -> anyhow::Result<()> {
        if self.thumb_textures.len() > MAX_THUMB_TEXTURES {
            self.thumb_textures.clear();
        }
        let g = &grid.grid;
        let selected = library.selected_index();
        for (i, entry) in library
            .visible()
            .enumerate()
            .skip(library.scroll)
            .take(g.shown())
        {
            let cell = g.cell_rect(i - library.scroll);
            match grid.thumbs.get(&entry.path) {
                Some((version, Some(picture))) => {
                    let fresh = self
                        .thumb_textures
                        .get(&entry.path)
                        .is_some_and(|(v, _)| v == version);
                    if !fresh {
                        let texture = self.smooth_texture(picture)?;
                        self.thumb_textures
                            .insert(entry.path.clone(), (*version, texture));
                    }
                    let (_, texture) = &self.thumb_textures[&entry.path];
                    let area = fit_picture(cell, picture.width as u32, picture.height as u32);
                    canvas.copy(texture, None, rect(area)).map_err(sdl_error)?;
                }
                found => {
                    canvas.set_draw_color(FIELD_COLOR);
                    canvas.fill_rect(rect(cell)).map_err(sdl_error)?;
                    if found.is_some() {
                        // No cover: the title, word-wrapped.
                        let ts = if scale > 1 { scale - 1 } else { 1 };
                        let line_h = (GLYPH * ts + 2 * ts) as i32;
                        let mut y = cell.y + line_h;
                        for line in wrap(&entry.title, cell.w.saturating_sub(GLYPH * ts), ts) {
                            if y + line_h > cell.y + cell.h as i32 {
                                break;
                            }
                            let x = cell.x + (cell.w as i32 - (text_width(&line) * ts) as i32) / 2;
                            draw_text_at(canvas, &line, x, y, ts).map_err(sdl_error)?;
                            y += line_h;
                        }
                    }
                }
            }
            if i == selected {
                canvas.set_draw_color(SELECTED_FRAME);
                for k in 0..(2 * scale as i32) {
                    let r = Area {
                        x: cell.x - k - 1,
                        y: cell.y - k - 1,
                        w: cell.w + 2 * (k as u32 + 1),
                        h: cell.h + 2 * (k as u32 + 1),
                    };
                    canvas.draw_rect(rect(r)).map_err(sdl_error)?;
                }
            }
        }
        if library.visible_len() == 0 {
            canvas.set_draw_color(DIM_TEXT);
            let text = if library.len() == 0 {
                "No ROM in this folder"
            } else {
                "No match"
            };
            super::chrome::draw_text_colour(canvas, text, g.area.x, g.area.y, scale)
                .map_err(sdl_error)?;
        }
        Ok(())
    }

    /// Uploads `preview` into the picture texture (made again when the
    /// size changes), smoothly scaled: covers are drawn much smaller than
    /// their size.
    fn upload_picture(&mut self, preview: &Preview) -> anyhow::Result<()> {
        let (w, h) = (preview.width as u32, preview.height as u32);
        if !matches!(&self.picture, Some((_, tw, th)) if (*tw, *th) == (w, h)) {
            // The scale quality hint applies to textures created after it.
            let previous = sdl2::hint::get("SDL_RENDER_SCALE_QUALITY");
            sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "linear");
            let texture = self
                .creator
                .create_texture_static(PixelFormatEnum::RGBA8888, w, h);
            sdl2::hint::set(
                "SDL_RENDER_SCALE_QUALITY",
                previous.as_deref().unwrap_or("nearest"),
            );
            let mut texture = texture?;
            texture.set_blend_mode(BlendMode::Blend);
            self.picture = Some((texture, w, h));
        }
        let bytes: Vec<u8> = preview
            .pixels
            .iter()
            .flat_map(|p| p.to_ne_bytes())
            .collect();
        if let Some((texture, _, _)) = &mut self.picture {
            texture.update(None, &bytes, w as usize * 4)?;
        }
        Ok(())
    }

    pub fn present(
        &mut self,
        canvas: &mut Canvas<Window>,
        view: &LibraryView,
    ) -> anyhow::Result<()> {
        canvas.set_draw_color(BACKGROUND);
        canvas.clear();
        let l = &view.layout;
        let s = l.scale;
        let text_y = |r: Area| r.y + (r.h as i32 - (GLYPH * s) as i32) / 2;
        let pad = (GLYPH * s) as i32 / 2;

        // Menu tabs: the open one lit, the others underlined; a tab whose
        // items do not apply is greyed out.
        for (cell, tab) in view.tab_cells.iter().zip(&view.tabs) {
            if tab.selected {
                canvas.set_blend_mode(BlendMode::Blend);
                canvas.set_draw_color(BAR_COLOR);
                canvas.fill_rect(rect(*cell)).map_err(sdl_error)?;
                canvas.fill_rect(rect(*cell)).map_err(sdl_error)?;
                canvas.set_draw_color(HOVER_COLOR);
                canvas.fill_rect(rect(*cell)).map_err(sdl_error)?;
                canvas.set_blend_mode(BlendMode::None);
            } else {
                canvas.set_draw_color(FIELD_COLOR);
                canvas.fill_rect(rect(*cell)).map_err(sdl_error)?;
                canvas.set_draw_color(BACKGROUND);
                canvas.draw_rect(rect(*cell)).map_err(sdl_error)?;
            }
            let x = cell.x + (cell.w as i32 - (text_width(tab.label) * s) as i32) / 2;
            if tab.enabled {
                draw_text_at(canvas, tab.label, x, text_y(*cell), s).map_err(sdl_error)?;
            } else {
                canvas.set_draw_color(GREYED_TEXT);
                super::chrome::draw_text_colour(canvas, tab.label, x, text_y(*cell), s)
                    .map_err(sdl_error)?;
            }
        }

        // List / Covers switch.
        for (cell, label, mode) in [
            (l.view_list, "List", crate::library::ViewMode::List),
            (l.view_covers, "Covers", crate::library::ViewMode::Covers),
        ] {
            canvas.set_draw_color(FIELD_COLOR);
            canvas.fill_rect(rect(cell)).map_err(sdl_error)?;
            if view.view_mode == mode {
                canvas.set_blend_mode(BlendMode::Blend);
                canvas.set_draw_color(HOVER_COLOR);
                canvas.fill_rect(rect(cell)).map_err(sdl_error)?;
                canvas.set_blend_mode(BlendMode::None);
            }
            canvas.set_draw_color(BACKGROUND);
            canvas.draw_rect(rect(cell)).map_err(sdl_error)?;
            let x = cell.x + (cell.w as i32 - (text_width(label) * s) as i32) / 2;
            if view.view_mode == mode {
                draw_text_at(canvas, label, x, text_y(cell), s).map_err(sdl_error)?;
            } else {
                canvas.set_draw_color(DIM_TEXT);
                super::chrome::draw_text_colour(canvas, label, x, text_y(cell), s)
                    .map_err(sdl_error)?;
            }
        }

        // Filter line.
        canvas.set_draw_color(FIELD_COLOR);
        canvas.fill_rect(rect(l.filter)).map_err(sdl_error)?;
        let (filter, placeholder) = match &view.picker {
            Some(p) => (p.picker.filter(), "Type to search the covers"),
            None => (view.library.filter(), "Type to search"),
        };
        if filter.is_empty() {
            canvas.set_draw_color(DIM_TEXT);
            super::chrome::draw_text_colour(
                canvas,
                placeholder,
                l.filter.x + pad,
                text_y(l.filter),
                s,
            )
            .map_err(sdl_error)?;
        } else {
            let label = if view.picker.is_some() {
                "Cover search"
            } else {
                "Search"
            };
            let text = format!("{label}: {filter}_");
            draw_text_at(
                canvas,
                &fit(&text, l.filter.w, s),
                l.filter.x + pad,
                text_y(l.filter),
                s,
            )
            .map_err(sdl_error)?;
        }

        if let Some(grid) = &view.grid {
            self.draw_grid(canvas, view.library, grid, s)?;
        } else {
            // List: the games, or the cover names while choosing.
            let (rows, selected, scroll, count): (Vec<(&str, &str)>, usize, usize, usize) =
                match &view.picker {
                    Some(p) => (
                        p.picker
                            .visible()
                            .skip(p.picker.scroll)
                            .take(l.rows)
                            .map(|n| (n, ""))
                            .collect(),
                        p.picker.selected_index(),
                        p.picker.scroll,
                        p.picker.visible_len(),
                    ),
                    None => (
                        view.library
                            .visible()
                            .skip(view.library.scroll)
                            .take(l.rows)
                            .map(|e| (e.title.as_str(), e.region_tag()))
                            .collect(),
                        view.library.selected_index(),
                        view.library.scroll,
                        view.library.visible_len(),
                    ),
                };
            for (i, (title, tag)) in rows.into_iter().enumerate() {
                let r = Area {
                    x: l.list.x,
                    y: l.list.y + (i as u32 * l.row_h) as i32,
                    w: l.list.w,
                    h: l.row_h,
                };
                if scroll + i == selected {
                    canvas.set_blend_mode(BlendMode::Blend);
                    canvas.set_draw_color(HOVER_COLOR);
                    canvas.fill_rect(rect(r)).map_err(sdl_error)?;
                    canvas.set_blend_mode(BlendMode::None);
                }
                let tag_w = text_width(tag) * s;
                let title_room = r.w.saturating_sub(tag_w + 3 * pad as u32);
                draw_text_at(canvas, &fit(title, title_room, s), r.x + pad, text_y(r), s)
                    .map_err(sdl_error)?;
                canvas.set_draw_color(DIM_TEXT);
                super::chrome::draw_text_colour(
                    canvas,
                    tag,
                    r.x + r.w as i32 - pad - tag_w as i32,
                    text_y(r),
                    s,
                )
                .map_err(sdl_error)?;
            }
            if count == 0 {
                let text = if view.picker.is_some() {
                    "No cover name matches"
                } else if view.scanning {
                    "Scanning..."
                } else if view.library.len() == 0 {
                    "No ROM in this folder"
                } else {
                    "No match"
                };
                canvas.set_draw_color(DIM_TEXT);
                super::chrome::draw_text_colour(canvas, text, l.list.x + pad, l.list.y + pad, s)
                    .map_err(sdl_error)?;
            }

            // The selected game's cover (or latest state), proportions kept.
            match view.preview {
                Some(p) => {
                    if self.picture_generation != Some(view.preview_generation) {
                        self.upload_picture(p)?;
                        self.picture_generation = Some(view.preview_generation);
                    }
                    if let Some((texture, w, h)) = &self.picture {
                        let area = fit_picture(l.preview, *w, *h);
                        canvas.copy(texture, None, rect(area)).map_err(sdl_error)?;
                    }
                }
                None => draw_preview(
                    canvas,
                    &mut self.menu_preview,
                    l.preview,
                    None,
                    view.picker
                        .as_ref()
                        .map_or("(no cover)", |p| p.picture_text),
                    s,
                )?,
            }

            // Info panel: the selected game's, or the chooser's keys.
            let picker_lines;
            let lines: Option<&[crate::gameinfo::InfoLine]> = match &view.picker {
                Some(p) => {
                    let mut v: Vec<crate::gameinfo::InfoLine> = vec![
                        ("Cover of", p.picker.title.clone()),
                        ("Matches", p.picker.visible_len().to_string()),
                        ("Enter", "use this cover".into()),
                        ("Esc", "cancel".into()),
                    ];
                    if p.own_cover {
                        v.push(("Note", "own .png next to the ROM wins".into()));
                    }
                    picker_lines = v;
                    Some(&picker_lines)
                }
                None => view.library.selected().map(|e| e.info.as_slice()),
            };
            if let Some(info) = lines {
                let is = l.info_scale;
                let label_w =
                    (info.iter().map(|(k, _)| k.len()).max().unwrap_or(0) as u32 + 1) * GLYPH * is;
                for (i, (label, value)) in info.iter().enumerate() {
                    let y = l.info.y + (i as u32 * l.info_row_h) as i32;
                    canvas.set_draw_color(DIM_TEXT);
                    super::chrome::draw_text_colour(canvas, label, l.info.x, y, is)
                        .map_err(sdl_error)?;
                    let room = l.info.w.saturating_sub(label_w);
                    draw_text_at(
                        canvas,
                        &fit(value, room, is),
                        l.info.x + label_w as i32,
                        y,
                        is,
                    )
                    .map_err(sdl_error)?;
                }
            }
        }

        // Status line.
        canvas.set_draw_color(FIELD_COLOR);
        canvas.fill_rect(rect(l.status)).map_err(sdl_error)?;
        draw_text_at(
            canvas,
            &fit(&view.status, l.status.w, s),
            l.status.x + pad,
            text_y(l.status),
            s,
        )
        .map_err(sdl_error)?;
        if let Some(fps) = view.fps {
            let text = format!("{fps:.1} FPS");
            let w = text_width(&text) * s;
            draw_text_at(
                canvas,
                &text,
                l.status.x + l.status.w as i32 - pad - w as i32,
                text_y(l.status),
                s,
            )
            .map_err(sdl_error)?;
        }

        if let Some(menu) = &view.menu {
            self.draw_menu(canvas, menu)?;
        }
        if view.bar {
            draw_bar(canvas, &view.chrome, view.title, view.hover).map_err(sdl_error)?;
        }
        canvas.present();
        Ok(())
    }

    /// The menu panel: section headers dimmed, items (greyed out when they
    /// do not apply, the selected one lit, shortcut reminders on the
    /// right), or the slot page.
    fn draw_menu(&mut self, canvas: &mut Canvas<Window>, menu: &MenuView) -> anyhow::Result<()> {
        use crate::menu::SlotAction;
        let l = &menu.layout;
        let scale = l.text_scale;
        let pad = (GLYPH * scale) as i32 / 2;
        let text_y = |r: &Area| r.y + (r.h as i32 - (GLYPH * scale) as i32) / 2;
        canvas.set_blend_mode(BlendMode::Blend);
        canvas.set_draw_color(BAR_COLOR);
        canvas.fill_rect(rect(l.panel)).map_err(sdl_error)?;
        canvas.fill_rect(rect(l.panel)).map_err(sdl_error)?; // twice as opaque as the bar

        canvas.set_draw_color(DIM_TEXT);
        let subtitle = fit(&menu.subtitle, l.subtitle.w, scale);
        super::chrome::draw_text_colour(
            canvas,
            &subtitle,
            l.subtitle.x,
            text_y(&l.subtitle),
            scale,
        )
        .map_err(sdl_error)?;

        if let Some(slots) = &menu.slots {
            let title = match slots.action {
                SlotAction::Save => "Save state to slot:",
                SlotAction::Load => "Load state from slot:",
            };
            canvas.set_blend_mode(BlendMode::None);
            for (i, cell) in l.slots.iter().enumerate() {
                let used = slots.used[i];
                let usable = used || slots.action == SlotAction::Save;
                if i == slots.selected as usize {
                    canvas.set_blend_mode(BlendMode::Blend);
                    canvas.set_draw_color(HOVER_COLOR);
                    canvas.fill_rect(rect(*cell)).map_err(sdl_error)?;
                    canvas.set_blend_mode(BlendMode::None);
                }
                canvas.set_draw_color(DIM_TEXT);
                canvas.draw_rect(rect(*cell)).map_err(sdl_error)?;
                let number = i.to_string();
                let state = if used { "used" } else { "empty" };
                let cx = |t: &str| cell.x + (cell.w as i32 - (text_width(t) * scale) as i32) / 2;
                let half = cell.h as i32 / 2;
                let y1 = cell.y + (half - (GLYPH * scale) as i32) / 2;
                if usable {
                    draw_text_at(canvas, &number, cx(&number), y1, scale).map_err(sdl_error)?;
                } else {
                    super::chrome::draw_text_colour(canvas, &number, cx(&number), y1, scale)
                        .map_err(sdl_error)?;
                }
                canvas.set_draw_color(DIM_TEXT);
                super::chrome::draw_text_colour(canvas, state, cx(state), y1 + half, scale)
                    .map_err(sdl_error)?;
            }
            // The title sits between the grid and the preview.
            let grid_bottom = l.slots.iter().map(|c| c.y + c.h as i32).max().unwrap_or(0);
            draw_text_at(canvas, title, l.panel.x + 2 * pad, grid_bottom + pad, scale)
                .map_err(sdl_error)?;
            let empty = match slots.action {
                SlotAction::Save => "(empty: will be created)",
                SlotAction::Load => "(empty)",
            };
            return draw_preview(
                canvas,
                &mut self.menu_preview,
                l.preview,
                menu.preview,
                empty,
                scale,
            );
        }
        for (r, row) in l.rows.iter().zip(&menu.rows) {
            if row.selected {
                canvas.set_draw_color(HOVER_COLOR);
                canvas.fill_rect(rect(*r)).map_err(sdl_error)?;
            }
            let hint_w = text_width(row.hint) * scale;
            let room = r.w.saturating_sub(hint_w + 4 * pad as u32);
            let text = fit(&row.label, room, scale);
            let x = r.x + 2 * pad;
            if row.enabled {
                draw_text_at(canvas, &text, x, text_y(r), scale).map_err(sdl_error)?;
            } else {
                canvas.set_draw_color(GREYED_TEXT);
                super::chrome::draw_text_colour(canvas, &text, x, text_y(r), scale)
                    .map_err(sdl_error)?;
            }
            canvas.set_draw_color(DIM_TEXT);
            let hx = r.x + r.w as i32 - pad - hint_w as i32;
            super::chrome::draw_text_colour(canvas, row.hint, hx, text_y(r), scale)
                .map_err(sdl_error)?;
        }
        canvas.set_blend_mode(BlendMode::None);
        Ok(())
    }
}

/// `preview` scaled into `area`, or a frame with `empty` when there is none.
fn draw_preview(
    canvas: &mut Canvas<Window>,
    texture: &mut Texture,
    area: Area,
    preview: Option<&Preview>,
    empty: &str,
    scale: u32,
) -> anyhow::Result<()> {
    match preview {
        Some(p) => {
            texture
                .with_lock(None, |pixels, pitch| {
                    copy_frame(&p.pixels, p.width, pixels, pitch)
                })
                .map_err(sdl_error)?;
            let source = Rect::new(0, 0, p.width as u32, p.height as u32);
            canvas
                .copy(texture, source, rect(area))
                .map_err(sdl_error)?;
        }
        None => {
            canvas.set_draw_color(DIM_TEXT);
            canvas.draw_rect(rect(area)).map_err(sdl_error)?;
            let w = text_width(empty) * scale;
            super::chrome::draw_text_colour(
                canvas,
                empty,
                area.x + (area.w as i32 - w as i32) / 2,
                area.y + (area.h as i32 - (GLYPH * scale) as i32) / 2,
                scale,
            )
            .map_err(sdl_error)?;
        }
    }
    Ok(())
}

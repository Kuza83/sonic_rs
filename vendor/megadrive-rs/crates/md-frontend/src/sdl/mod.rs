//! Window mode: SDL2 video, audio and input around the emulator core.
//!
//! Two windows: Game (the picture only) and Library (ROM list, our bar,
//! menu, messages). The loop routes each event to its window, emulates,
//! presents both, then waits as decided by `pacing` (display vsync or the
//! clock, the audio queue correcting drift).

mod audio;
mod chrome;
mod input;
mod library;
mod video;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use md_core::Console;

use crate::boxart;
use crate::chrome::{
    Button, Hit, Layout, Rect as Area, place_windows, released_over, restore_area,
};
use crate::config::WindowState;
use crate::coverpick::CoverPicker;
use crate::library::{
    Library, ViewMode, WATCH_INTERVAL, Watcher, changes, grid_layout, latest_state, library_layout,
    state_path,
};
use crate::menu::{
    Command, Menu, MenuContext, Nav, Page, SLOTS, menu_layout, nav_from_button, nav_from_key,
};
use crate::pacing::{
    ClockPacer, VsyncMode, audio_drift, display_matches, frame_duration, frames_this_refresh,
    use_vsync,
};
use crate::picker::{Picked, pick_folder};
use crate::{Game, WindowArgs};
use chrome::WindowKind;
use input::{Action, Input};
use sdl2::event::{Event, WindowEvent};
use sdl2::mouse::MouseButton;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

/// Where `--screenshot-at` writes its result, and the input script.
pub struct Output {
    pub path: Option<PathBuf>,
    pub hash: bool,
    /// Buttons added to the player's on given frames.
    pub script: crate::script::InputScript,
}

/// Loads a ROM file into a game (ROM dropped on the window).
pub type Loader<'a> = &'a dyn Fn(&Path) -> anyhow::Result<Game>;

/// Audio kept queued, in frames (one more with vsync, whose present blocks).
const AUDIO_TARGET_FRAMES: u32 = 3;
/// Idle wait while paused or without a game.
const PAUSE_POLL: Duration = Duration::from_millis(16);
const SCREENSHOT_DIR: &str = "screenshots";
/// Pads plugged into ports 1 and 2.
const PORTS: usize = 2;
/// Title of the window without a game.
const APP_TITLE: &str = "megadrive-rs";

/// SDL reports most errors as strings.
pub(crate) fn sdl_error(e: String) -> anyhow::Error {
    anyhow::anyhow!("SDL: {e}")
}

/// The game being played and what depends on it.
struct Session {
    game: Game,
    battery: Option<crate::battery::Battery>,
    pal: bool,
}

impl Session {
    fn open(mut game: Game) -> anyhow::Result<Self> {
        let battery = crate::battery::Battery::open(&game.rom, &mut game.console)?;
        let pal = game.console.bus().vdp().is_pal();
        Ok(Self { game, battery, pal })
    }

    /// Writes the SRAM of the game being left or quit.
    fn close(&mut self) -> anyhow::Result<()> {
        if let Some(battery) = &mut self.battery {
            battery.flush(&mut self.game.console)?;
        }
        Ok(())
    }
}

/// Largest grid thumbnail (the cells at text scale 3), scaled down further
/// by the renderer for smaller cells.
const THUMB_W: usize = 192;
const THUMB_H: usize = 270;
/// Size of the Library window.
const LIBRARY_SIZE: (u32, u32) = (900, 640);
const LIBRARY_TITLE: &str = "megadrive-rs - Library";
/// List rows moved by Page Up / Page Down and the mouse wheel.
const WHEEL_ROWS: isize = 3;

/// Window mode: the Game window (picture only) and the Library window (ROM
/// list, bar, menu, messages), sharing one loop. `library_dir` is scanned
/// in the background.
pub fn run(
    game: Option<Game>,
    opts: &WindowArgs,
    output: Output,
    loader: Loader,
    mut library_dir: PathBuf,
) -> anyhow::Result<()> {
    let context = sdl2::init().map_err(sdl_error)?;
    let video_sub = context.video().map_err(sdl_error)?;
    let title = game
        .as_ref()
        .map_or(APP_TITLE, |g| g.title.as_str())
        .to_string();
    // Vsync when the display runs at the console's rate (pacing.rs); only
    // the Game window presents with vsync (two blocking presents would halve
    // the rate).
    let refresh_hz = video_sub
        .current_display_mode(0)
        .map_or(0, |mode| mode.refresh_rate);
    let first_pal = game
        .as_ref()
        .is_some_and(|g| g.console.bus().vdp().is_pal());
    let mode = if opts.vsync {
        VsyncMode::On
    } else if opts.no_vsync {
        VsyncMode::Off
    } else {
        VsyncMode::Auto
    };
    let vsync = use_vsync(mode, refresh_hz, first_pal);
    eprintln!(
        "video: vsync {} (display {refresh_hz} Hz)",
        if vsync { "on" } else { "off" }
    );

    // One display each when there are two (chrome::place_windows).
    let displays: Vec<Area> = (0..video_sub.num_video_displays().unwrap_or(1))
        .filter_map(|i| video_sub.display_bounds(i).ok())
        .map(|r| Area {
            x: r.x(),
            y: r.y(),
            w: r.width(),
            h: r.height(),
        })
        .collect();
    let game_size = (
        vdp::FRAME_WIDTH as u32 * opts.scale,
        vdp::FRAME_HEIGHT as u32 * opts.scale,
    );
    let (game_area, library_area) = place_windows(&displays, game_size, LIBRARY_SIZE);
    // Where the windows were at the last close, when still on a display.
    let remembered = crate::config::Config::load();
    let restore = |state: Option<WindowState>| {
        state.and_then(|s| restore_area(s.area, &displays).map(|area| (area, s.fullscreen)))
    };
    let (game_saved, lib_saved) = (
        restore(remembered.game_window),
        restore(remembered.library_window),
    );
    let game_area = game_saved.map_or(game_area, |(area, _)| area);
    let library_area = lib_saved.map_or(library_area, |(area, _)| area);
    let mut game_canvas = video::window_canvas(
        &context,
        &title,
        game_area,
        vsync,
        opts.decorated,
        WindowKind::Game,
    )?;
    let mut lib_canvas = video::window_canvas(
        &context,
        LIBRARY_TITLE,
        library_area,
        false,
        opts.decorated,
        WindowKind::Library,
    )?;
    let (game_id, lib_id) = (game_canvas.window().id(), lib_canvas.window().id());
    // Fullscreen on the display the window was put on.
    if game_saved.is_some_and(|(_, full)| full) {
        video::toggle_fullscreen(&mut game_canvas)?;
    }
    if lib_saved.is_some_and(|(_, full)| full) {
        video::toggle_fullscreen(&mut lib_canvas)?;
    }
    // Last windowed areas (a fullscreen window's own size is the display's).
    let (mut game_windowed, mut lib_windowed) = (game_area, library_area);
    video_sub.text_input().start();

    let game_creator = game_canvas.texture_creator();
    let lib_creator = lib_canvas.texture_creator();
    let mut screen = video::Screen::new(&game_creator)?;
    let mut lib_screen = library::LibraryScreen::new(&lib_creator)?;
    let audio = if opts.mute {
        None
    } else {
        Some(audio::Audio::new(&context)?)
    };
    let mut input = Input::new(&context)?;
    let mut session = game.map(Session::open).transpose()?;
    let mut events = context.event_pump().map_err(sdl_error)?;

    let mut library = Library::default();
    // Folder dialog running in the background (menu "ROM folder...").
    let mut picker: Option<std::sync::mpsc::Receiver<Picked>> = None;
    // Folder watcher: first scan, then updates when ROMs are added or removed.
    let mut watcher = Watcher::start(library_dir.clone(), WATCH_INTERVAL);
    let mut scanning = true;
    // Preview of the selected game: (ROM, its cover or latest state), and a
    // counter telling the Library screen to upload a new picture.
    let mut list_preview: Option<(PathBuf, Option<video::Preview>)> = None;
    let mut preview_generation: u64 = 0;
    // Bumped when the selected game's picture is computed again.
    let mut list_generation: u64 = 0;
    // Box art: downloaded in the background unless --no-covers or no curl.
    let cover_cache = boxart::cache_dir();
    let downloader = cover_cache
        .clone()
        .filter(|_| !opts.no_covers && curl_available())
        .map(|cache| boxart::Downloader::start(boxart::LIBRETRO_BOXARTS.to_string(), cache));
    if !opts.no_covers && downloader.is_none() {
        eprintln!("covers: curl not found, no download");
    }
    let mut requested: HashSet<PathBuf> = HashSet::new();
    let (mut covers_total, mut covers_pending) = (0usize, 0usize);
    // Cover chooser: libretro's names (loaded once), the open chooser, the
    // pictures looked at (name -> file, None when missing) and the shown one.
    let mut cover_names: Option<Arc<Vec<String>>> = None;
    let mut names_loading: Option<std::sync::mpsc::Receiver<Option<Vec<String>>>> = None;
    let mut chooser_for: Option<PathBuf> = None;
    let mut chooser: Option<CoverPicker> = None;
    let mut browser: Option<boxart::Browser> = None;
    let mut browsed: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut chooser_preview: Option<(String, Option<video::Preview>)> = None;
    let mut shown_key = String::new();
    let mut menu = Menu::default();
    // The AI player (--agent): its process starts in the background on the
    // first F4 (loading the model takes seconds), then plays pad 1 while on,
    // its episodes starting again from the state it took over at.
    let mut agent: Option<crate::agent::AgentPlayer> = None;
    let mut agent_loading: Option<
        std::sync::mpsc::Receiver<anyhow::Result<crate::agent::AgentProcess>>,
    > = None;
    let mut agent_on = false;
    let mut agent_start: Option<Vec<u8>> = None;
    // --agent-on: as if F4 were pressed once a game runs.
    let mut agent_autostart = opts.agent_on;
    // List or cover grid (remembered).
    let mut view_mode = remembered.library_view;
    // Cover grid thumbnails, made in the background for the games on
    // screen: ROM -> (version, picture or None without a cover).
    let thumbnailer = boxart::Thumbnailer::start(cover_cache.clone(), THUMB_W, THUMB_H);
    let mut thumbs: HashMap<PathBuf, (u64, Option<video::Preview>)> = HashMap::new();
    let mut thumbs_requested: HashSet<PathBuf> = HashSet::new();
    let mut thumbs_version: u64 = 0;
    // Slots holding a state of the game running (menu): computed again
    // after a save, a launch or a stop.
    let mut used_slots: Option<[bool; SLOTS as usize]> = None;
    // Preview of the menu's slot: (slot, picture of its state if any).
    let mut menu_preview: Option<(u8, Option<video::Preview>)> = None;
    let mut messages = crate::chrome::Messages::default();
    let mut fps_meter = crate::chrome::FpsMeter::default();
    let mut show_fps = opts.show_fps;
    let mut hover = Hit::Normal;
    let mut game_focused = true;
    // The selected game's cover dragged from the Library preview: dropped
    // on the Game window, it launches the game. The hand cursor shows it.
    let mut dragging_cover: Option<PathBuf> = None;
    let mut ghost: Option<sdl2::mouse::Cursor> = None;
    let cursors = (
        sdl2::mouse::Cursor::from_system(sdl2::mouse::SystemCursor::Hand).ok(),
        sdl2::mouse::Cursor::from_system(sdl2::mouse::SystemCursor::Arrow).ok(),
    );
    // The game takes the keyboard and controller after a launch, even when
    // the window manager keeps the focus on the Library (GNOME refuses
    // focus requests it did not see coming); a click in the Library gives
    // them back to it.
    let mut keys_to_game = session.is_some();

    let target_frames = AUDIO_TARGET_FRAMES + vsync as u32;
    let mut clock = ClockPacer::new(frame_duration(first_pal));
    let start = Instant::now();
    let (mut paused, mut fast_forward) = (false, false);
    // Emulated frames (for --screenshot-at and --max-frames), and frames
    // shown without a game (so --max-frames also ends the empty windows).
    let (mut frames, mut idle_frames): (u64, u64) = (0, 0);
    'running: loop {
        while let Ok(entries) = watcher.updates.try_recv() {
            let text = if scanning {
                format!("{} games found", entries.len())
            } else {
                let (added, removed) = changes(library.entries(), &entries);
                format!("Library: +{added} / -{removed}")
            };
            messages.show(text, start.elapsed());
            if let (Some(downloader), Some(cache)) = (&downloader, &cover_cache) {
                let now = std::time::SystemTime::now();
                for entry in &entries {
                    if !requested.contains(&entry.path)
                        && boxart::needs_download(&entry.path, cache, now)
                    {
                        downloader.request(&entry.path);
                        requested.insert(entry.path.clone());
                        covers_total += 1;
                        covers_pending += 1;
                    }
                }
            }
            library.set_entries(entries);
            list_preview = None;
            scanning = false;
        }
        if let Some(downloader) = &downloader {
            while let Ok((rom, fetched)) = downloader.done.try_recv() {
                covers_pending = covers_pending.saturating_sub(1);
                match fetched {
                    boxart::Fetched::Saved(_) => {
                        thumbs.remove(&rom);
                        thumbs_requested.remove(&rom);
                        // Shown at once if it is the selected game's.
                        if list_preview.as_ref().is_some_and(|(p, _)| *p == rom) {
                            list_preview = None;
                        }
                    }
                    boxart::Fetched::Missing => {}
                    boxart::Fetched::Failed(e) => eprintln!("cover of {}: {e}", rom.display()),
                }
            }
        }
        let lib_chrome = video::layout(&lib_canvas, vdp::FRAME_HEIGHT);
        let lib_layout = library_layout(&lib_chrome);
        // Cover grid (shown unless the cover chooser takes the list).
        let grid = grid_layout(lib_layout.content, lib_layout.scale);
        let grid_mode = view_mode == ViewMode::Covers && chooser.is_none();
        // Selection steps: a row (list) or a grid row, and a page.
        let (row_step, page_step, side_step) = if grid_mode {
            (grid.cols as isize, grid.shown() as isize, 1)
        } else {
            (1, lib_layout.rows as isize, 0)
        };
        let used = *used_slots.get_or_insert_with(|| {
            session.as_ref().map_or([false; SLOTS as usize], |s| {
                std::array::from_fn(|i| state_path(&s.game.rom, i as u8).is_file())
            })
        });
        let menu_ctx = MenuContext {
            game: session.as_ref().map(|s| s.game.title.clone()),
            selected_game: library.selected().map(|e| e.title.clone()),
            fps: show_fps,
            covers_view: view_mode == ViewMode::Covers,
            agent: opts.agent.as_ref().map(|_| agent_on),
            used_slots: used,
        };
        let mut launch_path: Option<PathBuf> = None;
        // Cover chooser actions, applied after the events.
        let (mut open_chooser, mut use_cover) = (false, false);
        let mut stop_game = false;
        let mut toggle_agent = agent_autostart && session.is_some();
        if toggle_agent {
            agent_autostart = false;
        }
        let mut toggle_view = false;
        // New Library folder (dialog or dropped folder), applied after the events.
        let mut new_folder: Option<PathBuf> = None;
        if let Some(receiver) = &picker
            && let Ok(picked) = receiver.try_recv()
        {
            match picked {
                Picked::Folder(dir) => new_folder = Some(dir),
                Picked::Cancelled => {}
                Picked::Unavailable(e) => {
                    eprintln!("zenity: {e}");
                    messages.show(
                        "No folder dialog (zenity): drop a folder here",
                        start.elapsed(),
                    );
                }
            }
            picker = None;
        }
        for event in events.poll_iter() {
            let window = event.get_window_id();
            match &event {
                Event::Quit { .. } => break 'running,
                Event::Window {
                    win_event: WindowEvent::Close,
                    ..
                } => break 'running,
                Event::Window {
                    window_id,
                    win_event: WindowEvent::FocusGained,
                    ..
                } => {
                    game_focused = *window_id == game_id;
                    if keys_to_game != game_focused {
                        input.release_keyboard();
                    }
                    keys_to_game = game_focused;
                }
                Event::MouseButtonDown { window_id, .. } if *window_id == lib_id => {
                    if keys_to_game {
                        input.release_keyboard();
                    }
                    keys_to_game = false;
                }
                // The keyboard follows the mouse: entering a window (the
                // mouse moved there) gives it the keys, no click needed.
                Event::Window {
                    window_id,
                    win_event: WindowEvent::Enter,
                    ..
                } if dragging_cover.is_none() => {
                    let into_game = *window_id == game_id;
                    let wanted = into_game && session.is_some();
                    if keys_to_game != wanted {
                        input.release_keyboard();
                    }
                    keys_to_game = wanted;
                    input_focus(if into_game { &game_canvas } else { &lib_canvas });
                }
                Event::Window {
                    window_id,
                    win_event: WindowEvent::FocusLost,
                    ..
                } if *window_id == game_id => {
                    input.release_keyboard();
                }
                Event::DropFile { filename, .. } => {
                    // A folder becomes the Library folder; a file is launched.
                    let path = PathBuf::from(filename);
                    if path.is_dir() {
                        new_folder = Some(path);
                    } else {
                        launch_path = Some(path);
                    }
                    continue;
                }
                _ => {}
            }
            let controller = matches!(event, Event::ControllerButtonDown { .. });
            // Keys and buttons of the Library window going to the game.
            // The controller stays with the game while one runs (the open menu
            // still takes its navigation buttons).
            let to_game = session.is_some()
                && (controller
                    || keys_to_game
                        && matches!(
                            event,
                            Event::KeyDown { .. } | Event::KeyUp { .. } | Event::TextInput { .. }
                        ));

            // The tabs, always shown in the Library window: a click opens
            // the menu on a tab, or closes it.
            if let Event::MouseButtonDown {
                window_id,
                mouse_btn: MouseButton::Left,
                x,
                y,
                ..
            } = &event
                && *window_id == lib_id
                && let Some(tab) =
                    menu_layout(&lib_chrome, lib_layout.tabs, menu.tab_index()).tab_at(*x, *y)
            {
                let was_open = menu.open;
                menu.click_tab(tab, &menu_ctx);
                if menu.open && !was_open {
                    menu_preview = None;
                }
                continue;
            }

            // Drag and drop of the cover onto the Game window.
            match &event {
                Event::MouseButtonDown {
                    window_id,
                    mouse_btn: MouseButton::Left,
                    clicks,
                    x,
                    y,
                    ..
                } if *window_id == lib_id
                    && !menu.open
                    && chooser.is_none()
                    && if grid_mode {
                        grid.cell_at(*x, *y)
                            .is_some_and(|i| library.scroll + i < library.visible_len())
                    } else {
                        lib_layout.preview.contains(*x, *y)
                    } =>
                {
                    // In the grid the pressed cover is selected first; a
                    // double click launches it.
                    if grid_mode && let Some(i) = grid.cell_at(*x, *y) {
                        library.select(library.scroll + i);
                        if *clicks >= 2 {
                            launch_path = library.selected().map(|e| e.path.clone());
                            continue;
                        }
                    }
                    if let Some(entry) = library.selected() {
                        dragging_cover = Some(entry.path.clone());
                        // The cover itself, see-through, held under the
                        // mouse (a cursor follows it across windows and
                        // displays); else the hand.
                        let picture = if grid_mode {
                            thumbs.get(&entry.path).and_then(|(_, p)| p.as_ref())
                        } else {
                            list_preview
                                .as_ref()
                                .filter(|(rom, _)| *rom == entry.path)
                                .and_then(|(_, p)| p.as_ref())
                        };
                        // A previous ghost goes (a release missed).
                        drop(std::mem::replace(
                            &mut ghost,
                            picture.and_then(ghost_cursor),
                        ));
                        match (&ghost, &cursors.0) {
                            (Some(cursor), _) | (None, Some(cursor)) => cursor.set(),
                            (None, None) => {}
                        }
                        if !grid_mode {
                            let text = format!("Drop on the game window to play {}", entry.title);
                            messages.show(text, start.elapsed());
                        }
                    }
                    continue;
                }
                Event::MouseButtonUp {
                    window_id,
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if dragging_cover.is_some() => {
                    if let Some(arrow) = &cursors.1 {
                        arrow.set();
                    }
                    drop(ghost.take()); // freed once no longer the cursor
                    let from = if *window_id == game_id {
                        &game_canvas
                    } else {
                        &lib_canvas
                    };
                    let dropped = released_over(
                        video::window_rect(from),
                        *x,
                        *y,
                        video::window_rect(&game_canvas),
                    );
                    match dragging_cover.take() {
                        Some(path) if dropped => launch_path = Some(path),
                        _ => messages.clear(),
                    }
                    continue;
                }
                _ => {}
            }

            // The open menu takes the navigation keys and buttons from
            // either window, and the mouse in the Library window.
            if menu.open {
                let rows = menu_layout(&lib_chrome, lib_layout.tabs, menu.tab_index());
                let on_slots = matches!(menu.page, Page::Slots(_));
                let command = match &event {
                    Event::KeyDown {
                        keycode: Some(k), ..
                    } => nav_from_key(&k.name()).map(|n| menu.nav(n, &menu_ctx)),
                    Event::ControllerButtonDown { button, .. } => {
                        nav_from_button(&button.string()).map(|n| menu.nav(n, &menu_ctx))
                    }
                    Event::MouseMotion {
                        window_id, x, y, ..
                    } if *window_id == lib_id => {
                        if rows.tab_at(*x, *y).is_some() {
                            // Tabs open on click only.
                        } else if on_slots {
                            if let Some(slot) = rows.slot_at(*x, *y) {
                                menu.select_slot(slot);
                            }
                        } else if let Some(i) = rows.row_at(*x, *y) {
                            menu.select_row(i, &menu_ctx);
                        }
                        None
                    }
                    Event::MouseButtonDown {
                        window_id,
                        mouse_btn: MouseButton::Left,
                        x,
                        y,
                        ..
                    } if *window_id == lib_id => {
                        if !rows.panel.contains(*x, *y) {
                            // A click beside the menu closes it.
                            menu.open = false;
                            Some(Command::Resume)
                        } else if on_slots {
                            rows.slot_at(*x, *y).map(|slot| {
                                menu.select_slot(slot);
                                menu.activate(&menu_ctx)
                            })
                        } else {
                            // Only a row that could be selected is chosen.
                            rows.row_at(*x, *y).and_then(|i| {
                                menu.select_row(i, &menu_ctx);
                                (menu.selected_row() == i).then(|| menu.activate(&menu_ctx))
                            })
                        }
                    }
                    _ => None,
                };
                if let Some(command) = command {
                    match command {
                        Command::None | Command::Resume => {}
                        Command::Quit => break 'running,
                        Command::Fullscreen => video::toggle_fullscreen(&mut game_canvas)?,
                        Command::ToggleFps => show_fps = !show_fps,
                        Command::ToggleView => toggle_view = true,
                        Command::ToggleAgent => toggle_agent = true,
                        Command::StopGame => stop_game = true,
                        Command::ChooseCover => open_chooser = true,
                        Command::ResetCover => {
                            let text = match (library.selected(), &cover_cache) {
                                (Some(entry), Some(cache)) => {
                                    boxart::reset(cache, &entry.path);
                                    thumbs.remove(&entry.path);
                                    thumbs_requested.remove(&entry.path);
                                    requested.remove(&entry.path);
                                    if let Some(downloader) = &downloader {
                                        downloader.request(&entry.path);
                                        requested.insert(entry.path.clone());
                                        covers_total += 1;
                                        covers_pending += 1;
                                    }
                                    list_preview = None;
                                    "Cover reset: looking for it again".to_string()
                                }
                                _ => "No game selected".to_string(),
                            };
                            messages.show(text, start.elapsed());
                        }
                        Command::ChooseFolder => {
                            if picker.is_none() {
                                picker = Some(pick_folder(&library_dir));
                                messages.show("Choose a ROM folder...", start.elapsed());
                            }
                        }
                        Command::Save(_) | Command::Load(_) | Command::Reset => {
                            let text = match &mut session {
                                None => "No game".to_string(),
                                Some(s) => {
                                    let console = &mut s.game.console;
                                    match command {
                                        Command::Save(n) => save_state(console, &s.game.rom, n),
                                        Command::Load(n) => load_state(console, &s.game.rom, n),
                                        _ => {
                                            console.reset_soft();
                                            "Reset".to_string()
                                        }
                                    }
                                }
                            };
                            menu_preview = None;
                            list_preview = None;
                            used_slots = None;
                            messages.show(text, start.elapsed());
                        }
                    }
                    continue;
                }
            }

            // The cover chooser takes the Library window's input.
            if let Some(chooser) = &mut chooser
                && !to_game
                && (window == Some(lib_id) || (controller && !game_focused))
            {
                let rows = lib_layout.rows as isize;
                match &event {
                    Event::KeyDown {
                        keycode: Some(key), ..
                    } => match key.name().as_str() {
                        "Up" => chooser.move_by(-1),
                        "Down" => chooser.move_by(1),
                        "PageUp" => chooser.move_by(-rows),
                        "PageDown" => chooser.move_by(rows),
                        "Home" => chooser.home(),
                        "End" => chooser.end(),
                        "Return" | "Keypad Enter" => use_cover = true,
                        "Backspace" => chooser.pop_filter(),
                        "Escape" => chooser_for = None,
                        _ => {}
                    },
                    Event::TextInput { text, .. } => {
                        for c in text.chars().filter(|c| c.is_ascii_graphic() || *c == ' ') {
                            chooser.push_filter(c);
                        }
                    }
                    Event::MouseWheel { y, .. } => chooser.move_by(-(*y as isize) * WHEEL_ROWS),
                    Event::MouseMotion { x, y, .. } => {
                        if let Some(row) = lib_layout.row_at(*x, *y) {
                            chooser.select(chooser.scroll + row);
                        }
                    }
                    Event::MouseButtonDown {
                        mouse_btn: MouseButton::Left,
                        clicks,
                        x,
                        y,
                        ..
                    } => {
                        if let Some(row) = lib_layout.row_at(*x, *y) {
                            chooser.select(chooser.scroll + row);
                            use_cover = *clicks >= 2;
                        }
                    }
                    Event::ControllerButtonDown { button, .. } => {
                        match nav_from_button(&button.string()) {
                            Some(Nav::Up) => chooser.move_by(-1),
                            Some(Nav::Down) => chooser.move_by(1),
                            Some(Nav::Left) => chooser.move_by(-rows),
                            Some(Nav::Right) => chooser.move_by(rows),
                            Some(Nav::Activate) => use_cover = true,
                            Some(Nav::Back) => chooser_for = None,
                            None => {}
                        }
                    }
                    _ => {}
                }
                chooser.keep_selection_visible(lib_layout.rows);
                continue;
            }

            // Library window (and the controller while it has the focus).
            if !to_game && (window == Some(lib_id) || (controller && !game_focused)) {
                if !opts.decorated {
                    match window_event(&event, &lib_chrome) {
                        Some(WindowInput::Moved(x, y)) => hover = lib_chrome.hit(x, y),
                        Some(WindowInput::Left) => hover = Hit::Normal,
                        Some(WindowInput::Click(Button::Minimize)) => {
                            lib_canvas.window_mut().minimize()
                        }
                        Some(WindowInput::Click(Button::Fullscreen)) => {
                            video::toggle_fullscreen(&mut lib_canvas)?
                        }
                        Some(WindowInput::Click(Button::Close)) => break 'running,
                        None => {}
                    }
                }
                match &event {
                    Event::KeyDown {
                        keycode: Some(key), ..
                    } => match key.name().as_str() {
                        "Up" => library.move_by(-row_step),
                        "Down" => library.move_by(row_step),
                        "Left" => library.move_by(-side_step),
                        "Right" => library.move_by(side_step),
                        "PageUp" => library.move_by(-page_step),
                        "PageDown" => library.move_by(page_step),
                        "Home" => library.home(),
                        "End" => library.end(),
                        "Return" | "Keypad Enter" => {
                            launch_path = library.selected().map(|e| e.path.clone())
                        }
                        "Backspace" => library.pop_filter(),
                        "Escape" => {
                            menu.open(&menu_ctx);
                            menu_preview = None;
                        }
                        "F10" => show_fps = !show_fps,
                        "F11" => video::toggle_fullscreen(&mut game_canvas)?,
                        "F2" => open_chooser = true,
                        "F3" => toggle_view = true,
                        "F4" => toggle_agent = true,
                        _ => {}
                    },
                    Event::TextInput { text, .. } => {
                        for c in text.chars().filter(|c| c.is_ascii_graphic() || *c == ' ') {
                            library.push_filter(c);
                        }
                    }
                    // The selection follows the mouse when it moves (a
                    // still mouse never fights the keyboard).
                    Event::MouseMotion { x, y, .. } if dragging_cover.is_none() && !menu.open => {
                        let row = if grid_mode {
                            grid.cell_at(*x, *y)
                        } else {
                            lib_layout.row_at(*x, *y)
                        };
                        if let Some(row) = row {
                            library.select(library.scroll + row);
                        }
                    }
                    Event::MouseWheel { y, .. } => {
                        let step = if grid_mode { row_step } else { WHEEL_ROWS };
                        library.move_by(-(*y as isize) * step)
                    }
                    Event::MouseButtonDown {
                        mouse_btn: MouseButton::Left,
                        clicks,
                        x,
                        y,
                        ..
                    } => {
                        if lib_layout.view_list.contains(*x, *y) {
                            toggle_view = view_mode != ViewMode::List;
                        } else if lib_layout.view_covers.contains(*x, *y) {
                            toggle_view = view_mode != ViewMode::Covers;
                        } else if grid_mode {
                            // Grid clicks are handled with the cover drag.
                        } else if let Some(row) = lib_layout.row_at(*x, *y) {
                            library.select(library.scroll + row);
                            if *clicks >= 2 {
                                launch_path = library.selected().map(|e| e.path.clone());
                            }
                        }
                    }
                    Event::ControllerButtonDown { button, .. } => {
                        match nav_from_button(&button.string()) {
                            Some(Nav::Up) => library.move_by(-row_step),
                            Some(Nav::Down) => library.move_by(row_step),
                            Some(Nav::Left) if grid_mode => library.move_by(-1),
                            Some(Nav::Right) if grid_mode => library.move_by(1),
                            Some(Nav::Left) => library.move_by(-page_step),
                            Some(Nav::Right) => library.move_by(page_step),
                            Some(Nav::Activate) => {
                                launch_path = library.selected().map(|e| e.path.clone())
                            }
                            Some(Nav::Back) => {
                                menu.open(&menu_ctx);
                                menu_preview = None;
                            }
                            None => {}
                        }
                    }
                    _ => {}
                }
                if grid_mode {
                    library.keep_selection_visible_grid(grid.cols, grid.rows);
                } else {
                    library.keep_selection_visible(lib_layout.rows);
                }
                continue;
            }

            // Game window: the game's keys, controller and shortcuts.
            if !to_game && window.is_some() && window != Some(game_id) {
                continue;
            }
            let action = input.handle(&event);
            match action {
                Action::None | Action::Quit => {}
                Action::ToggleFullscreen => video::toggle_fullscreen(&mut game_canvas)?,
                Action::ToggleFps => show_fps = !show_fps,
                Action::ToggleAgent => toggle_agent = true,
                Action::ShowLibrary => {
                    input.release_keyboard();
                    keys_to_game = false;
                    fast_forward = false;
                    focus_window(&mut lib_canvas);
                }
                Action::ToggleMenu => {
                    menu.open(&menu_ctx);
                    menu_preview = None;
                    lib_canvas.window_mut().raise();
                }
                Action::FastForward(on) => {
                    fast_forward = on;
                    if on && session.is_some() {
                        messages.show("Fast forward", start.elapsed());
                    }
                }
                Action::SelectSlot(n) => {
                    menu.slot = n;
                    menu_preview = None;
                    messages.show(format!("Slot {n}"), start.elapsed());
                }
                _ => {
                    let Some(s) = &mut session else { continue };
                    let console = &mut s.game.console;
                    let text = match action {
                        Action::TogglePause => {
                            paused = !paused;
                            messages
                                .show(if paused { "Paused" } else { "Resumed" }, start.elapsed());
                            continue;
                        }
                        Action::Screenshot => screenshot(console)?,
                        Action::SaveState => {
                            menu_preview = None;
                            list_preview = None;
                            used_slots = None;
                            save_state(console, &s.game.rom, menu.slot)
                        }
                        Action::LoadState => load_state(console, &s.game.rom, menu.slot),
                        Action::Reset => {
                            console.reset_soft();
                            "Reset".to_string()
                        }
                        _ => continue,
                    };
                    messages.show(text, start.elapsed());
                }
            }
        }

        if toggle_view {
            view_mode = view_mode.toggled();
            let mut config = crate::config::Config::load();
            config.library_view = view_mode;
            config.save();
        }
        if view_mode == ViewMode::Covers {
            library.keep_selection_visible_grid(grid.cols, grid.rows);
        } else {
            library.keep_selection_visible(lib_layout.rows);
        }

        // Cover chooser: open (the names list first), Esc, choice.
        if chooser.is_some() && chooser_for.is_none() {
            chooser = None;
            chooser_preview = None;
        }
        if open_chooser {
            match (library.selected(), &cover_cache) {
                (None, _) => messages.show("No game selected", start.elapsed()),
                (_, None) => messages.show("No cache folder (HOME unset)", start.elapsed()),
                (Some(entry), Some(cache)) => {
                    menu.open = false;
                    chooser_for = Some(entry.path.clone());
                    if let Some(names) = &cover_names {
                        chooser = Some(CoverPicker::new(entry.path.clone(), Arc::clone(names)));
                    } else if names_loading.is_none() {
                        if curl_available() {
                            names_loading = Some(boxart::load_index(
                                boxart::LIBRETRO_BOXARTS.to_string(),
                                cache.clone(),
                            ));
                            messages.show("Loading the cover list...", start.elapsed());
                        } else {
                            messages.show("Choosing covers needs curl", start.elapsed());
                            chooser_for = None;
                        }
                    }
                }
            }
        }
        if let Some(receiver) = &names_loading
            && let Ok(names) = receiver.try_recv()
        {
            names_loading = None;
            match names {
                Some(names) => {
                    let names = Arc::new(names);
                    if let Some(rom) = &chooser_for {
                        chooser = Some(CoverPicker::new(rom.clone(), Arc::clone(&names)));
                    }
                    cover_names = Some(names);
                }
                None => {
                    messages.show("Cannot load the cover list (network?)", start.elapsed());
                    chooser_for = None;
                }
            }
        }
        if let Some(b) = &browser {
            while let Ok((name, file)) = b.done.try_recv() {
                if chooser_preview.as_ref().is_some_and(|(n, _)| *n == name) {
                    chooser_preview = None; // shown again with its picture
                }
                browsed.insert(name, file);
            }
        }
        if use_cover
            && let (Some(c), Some(cache)) = (&chooser, &cover_cache)
            && let Some(name) = c.selected_name()
        {
            match browsed.get(name) {
                Some(Some(file)) => {
                    let text = match boxart::choose(cache, &c.rom, file) {
                        Ok(()) if boxart::own_cover(&c.rom).is_some() => {
                            "Cover saved, but your own .png next to the ROM wins".to_string()
                        }
                        Ok(()) => format!("Cover: {name}"),
                        Err(e) => format!("Cannot save the cover: {e}"),
                    };
                    messages.show(text, start.elapsed());
                    thumbs.remove(&c.rom);
                    thumbs_requested.remove(&c.rom);
                    chooser_for = None;
                    chooser = None;
                    chooser_preview = None;
                    list_preview = None;
                }
                Some(None) => messages.show("No picture for this name", start.elapsed()),
                None => messages.show("Wait for the picture...", start.elapsed()),
            }
        }
        if let (Some(c), Some(cache)) = (&chooser, &cover_cache) {
            let name = c.selected_name().map(str::to_string);
            if chooser_preview.as_ref().map(|(n, _)| n) != name.as_ref()
                && let Some(name) = name
            {
                let picture = match browsed.get(&name) {
                    Some(file) => file.as_deref().and_then(boxart::load_cover),
                    None => {
                        browser
                            .get_or_insert_with(|| {
                                boxart::Browser::start(
                                    boxart::LIBRETRO_BOXARTS.to_string(),
                                    cache.join("browse"),
                                )
                            })
                            .request(&name);
                        None
                    }
                };
                let picture = picture.map(|i| video::Preview {
                    pixels: i.pixels,
                    width: i.width,
                    height: i.height,
                });
                chooser_preview = Some((name, picture));
            }
        }

        if let Some(dir) = new_folder {
            let mut config = crate::config::Config::load();
            config.library = Some(dir.clone());
            config.save();
            messages.show(format!("ROM folder: {}", dir.display()), start.elapsed());
            library.set_entries(Vec::new());
            list_preview = None;
            watcher = Watcher::start(dir.clone(), WATCH_INTERVAL);
            scanning = true;
            library_dir = dir;
        }
        // The AI player: F4 or the menu turns it on or off.
        if toggle_agent {
            match (&opts.agent, &session) {
                (None, _) => messages.show("No AI: start with --agent MODEL", start.elapsed()),
                (_, None) => messages.show("No game for the AI", start.elapsed()),
                _ if agent_on => {
                    agent_on = false;
                    messages.show("AI off: pad 1 is yours", start.elapsed());
                }
                (Some(model), Some(_)) if agent.is_none() => {
                    if agent_loading.is_none() {
                        let (model, deterministic) = (model.clone(), opts.agent_deterministic);
                        let python = crate::agent::default_python();
                        let (sender, receiver) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let _ = sender.send(crate::agent::AgentProcess::spawn(
                                &python,
                                &model,
                                deterministic,
                            ));
                        });
                        agent_loading = Some(receiver);
                        messages.show("AI starting...", start.elapsed());
                    }
                }
                _ => agent_on = true,
            }
            if agent_on && let (Some(player), Some(s)) = (&mut agent, &mut session) {
                agent_start = Some(s.game.console.save_state());
                player.begin_episode(&s.game.console);
                messages.show("AI playing (F4 to take over)", start.elapsed());
            }
        }
        if let Some(receiver) = &agent_loading
            && let Ok(result) = receiver.try_recv()
        {
            agent_loading = None;
            match result {
                Ok(process) => {
                    let mut player = crate::agent::AgentPlayer::new(process);
                    player.continue_acts = opts.agent_continue;
                    if let Some(s) = &mut session {
                        agent_start = Some(s.game.console.save_state());
                        player.begin_episode(&s.game.console);
                        agent_on = true;
                        messages.show("AI playing (F4 to take over)", start.elapsed());
                    }
                    agent = Some(player);
                }
                Err(e) => {
                    eprintln!("AI: {e:#}");
                    messages.show("AI failed to start (see the terminal)", start.elapsed());
                }
            }
        }

        if stop_game && let Some(mut old) = session.take() {
            old.close()?;
            if let Some(audio) = &audio {
                audio.clear();
            }
            game_canvas.window_mut().set_title(APP_TITLE).ok();
            paused = false;
            keys_to_game = false;
            agent_on = false;
            used_slots = None;
            menu_preview = None;
            list_preview = None;
            messages.show(format!("Stopped {}", old.game.title), start.elapsed());
        }
        if let Some(path) = launch_path {
            match loader(&path) {
                Ok(game) => {
                    if let Some(old) = &mut session {
                        old.close()?;
                    }
                    game_canvas.window_mut().set_title(&game.title).ok();
                    messages.show(format!("Loaded {}", game.title), start.elapsed());
                    let new = Session::open(game)?;
                    clock = ClockPacer::new(frame_duration(new.pal));
                    if let Some(audio) = &audio {
                        audio.clear();
                    }
                    session = Some(new);
                    used_slots = None;
                    agent_on = false;
                    paused = false;
                    menu_preview = None;
                    focus_window(&mut game_canvas);
                    input.release_keyboard();
                    keys_to_game = true;
                }
                Err(e) => {
                    eprintln!("cannot load {}: {e:#}", path.display());
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    messages.show(format!("Cannot load {name}"), start.elapsed());
                }
            }
        }

        // The menu pauses the game.
        let running = session.is_some() && !paused && !menu.open;
        if let Some(s) = &mut session {
            let console = &mut s.game.console;
            if !running {
                for port in 0..PORTS {
                    console.set_buttons(port, input.buttons(port));
                }
            } else {
                // Following the display: 0-2 frames per refresh keep the
                // sound queue on target; otherwise one frame per loop.
                let count = match &audio {
                    Some(audio) if vsync && display_matches(refresh_hz, s.pal) && !fast_forward => {
                        frames_this_refresh(audio_drift(audio.queued_pairs(), target_frames, s.pal))
                    }
                    _ => 1,
                };
                for _ in 0..count {
                    // The AI chooses pad 1 while it plays.
                    let agent_buttons = match &mut agent {
                        Some(player) if agent_on => match player.buttons(console) {
                            Ok(buttons) => Some(buttons),
                            Err(e) => {
                                eprintln!("AI: {e:#}");
                                messages.show("AI stopped (see the terminal)", start.elapsed());
                                agent_on = false;
                                agent = None;
                                None
                            }
                        },
                        _ => None,
                    };
                    crate::run_scripted_frame(
                        console,
                        &output.script,
                        |port| match agent_buttons {
                            Some(buttons) if port == 0 => buttons,
                            _ => input.buttons(port),
                        },
                    );
                    if let (Some(player), Some(start_state)) = (&mut agent, &mut agent_start)
                        && agent_on
                        && let Some(outcome) = player.episode_over(console)
                    {
                        use crate::agent::Outcome;
                        match outcome {
                            // --agent-continue: the game goes on to the next act.
                            Outcome::Finished if player.continue_acts => {}
                            // From now on, a death starts over this act.
                            Outcome::NextAct => {
                                *start_state = console.save_state();
                                player.begin_episode(console);
                            }
                            // As in training: the episode starts over.
                            _ => {
                                console.load_state(start_state).ok();
                                player.begin_episode(console);
                            }
                        }
                    }
                    frames += 1;
                    if let Some(audio) = &audio
                        && !fast_forward
                    {
                        audio.queue(console.audio_samples())?;
                    }
                    if let Some(battery) = &mut s.battery {
                        battery.tick(console)?;
                    }
                }
            }
        } else {
            idle_frames += 1;
        }

        // Grid thumbnails: ask for the games on screen, take the ready ones.
        if view_mode == ViewMode::Covers {
            for entry in library.visible().skip(library.scroll).take(grid.shown()) {
                if thumbs_requested.insert(entry.path.clone()) {
                    thumbnailer.request(&entry.path);
                }
            }
        }
        while let Ok((rom, image)) = thumbnailer.done.try_recv() {
            thumbs_version += 1;
            let picture = image.map(|i| video::Preview {
                pixels: i.pixels,
                width: i.width,
                height: i.height,
            });
            thumbs.insert(rom, (thumbs_version, picture));
        }

        // Previews: the selected game's cover (else its latest state), the
        // menu's slot.
        let selected_rom = library.selected().map(|e| e.path.clone());
        if list_preview.as_ref().map(|(p, _)| p) != selected_rom.as_ref() {
            list_preview = selected_rom.map(|rom| {
                let picture =
                    cover_preview(&rom, cover_cache.as_deref()).or_else(|| rom_preview(&rom));
                (rom, picture)
            });
            list_generation += 1;
        }
        if menu.open
            && matches!(menu.page, Page::Slots(_))
            && menu_preview
                .as_ref()
                .is_none_or(|(slot, _)| *slot != menu.slot)
        {
            let picture = session
                .as_ref()
                .and_then(|s| state_preview(&s.game.console, &s.game.rom, menu.slot));
            menu_preview = Some((menu.slot, picture));
        }

        let now = start.elapsed();
        fps_meter.frame(now);
        screen.present(&mut game_canvas, session.as_ref().map(|s| &s.game.console))?;

        // The picture shown: the chooser's, else the selected game's; a new
        // one is uploaded when it changes.
        let (shown_preview, key) = match &chooser {
            Some(_) => {
                let p = chooser_preview.as_ref().and_then(|(_, p)| p.as_ref());
                let name = chooser_preview.as_ref().map_or("", |(n, _)| n.as_str());
                (p, format!("cover:{name}:{}", p.is_some()))
            }
            None => (
                list_preview.as_ref().and_then(|(_, p)| p.as_ref()),
                format!("game:{list_generation}"),
            ),
        };
        if key != shown_key {
            shown_key = key;
            preview_generation += 1;
        }
        let picture_text = match chooser_preview.as_ref().map(|(n, _)| browsed.get(n)) {
            Some(Some(None)) => "(no picture)",
            _ => "Loading picture...",
        };
        let agent_status = match (&agent, &session) {
            (Some(player), Some(s)) if agent_on => {
                let (x, best) = player.position(&s.game.console);
                Some(format!(
                    "AI: {}, x {x}, best {best} - episode {}, {} finished",
                    crate::agent::ACTION_NAMES[player.last_action],
                    player.episodes + 1,
                    player.finished
                ))
            }
            _ => None,
        };
        let default_status = match (&session, scanning) {
            _ if agent_status.is_some() => agent_status.unwrap_or_default(),
            (_, true) => format!("Scanning {}...", library_dir.display()),
            // The grid shows no titles: the selected game's is given here.
            _ if grid_mode && library.selected().is_some() => {
                let e = library.selected().unwrap();
                format!("{} ({}) - {} games", e.title, e.region_tag(), library.len())
            }
            _ if covers_pending > 0 => {
                format!(
                    "Downloading covers {}/{covers_total}",
                    covers_total - covers_pending
                )
            }
            (Some(s), _) if paused => format!("Paused - {}", s.game.title),
            (Some(s), _) => format!("Playing {} - {} games", s.game.title, library.len()),
            (None, _) => format!("{} games in {}", library.len(), library_dir.display()),
        };
        let view = library::LibraryView {
            picker: chooser.as_ref().map(|c| library::PickerView {
                picker: c,
                picture_text,
                own_cover: boxart::own_cover(&c.rom).is_some(),
            }),
            chrome: lib_chrome.clone(),
            layout: lib_layout.clone(),
            bar: !opts.decorated,
            title: LIBRARY_TITLE,
            hover,
            library: &library,
            scanning,
            preview: shown_preview,
            preview_generation,
            status: messages.current(now).map_or(default_status, str::to_string),
            fps: fps_meter.fps().filter(|_| show_fps),
            grid: grid_mode.then_some(library::GridView {
                grid,
                thumbs: &thumbs,
            }),
            view_mode,
            tab_cells: menu_layout(&lib_chrome, lib_layout.tabs, menu.tab_index()).tabs,
            tabs: menu.tabs(&menu_ctx),
            menu: menu.open.then(|| library::MenuView {
                layout: menu_layout(&lib_chrome, lib_layout.tabs, menu.tab_index()),
                subtitle: menu.subtitle(&menu_ctx),
                rows: menu.rows(&menu_ctx),
                slots: match menu.page {
                    Page::Slots(action) => Some(library::SlotsView {
                        action,
                        used: menu_ctx.used_slots,
                        selected: menu.slot,
                    }),
                    Page::Main => None,
                },
                preview: menu_preview.as_ref().and_then(|(_, p)| p.as_ref()),
            }),
        };
        lib_screen.present(&mut lib_canvas, &view)?;

        if let Some(s) = &session
            && opts.screenshot_at.is_some_and(|n| frames >= n)
        {
            crate::save_frame(&s.game.console, output.path.as_deref(), output.hash)?;
            break;
        }
        if opts
            .max_frames
            .is_some_and(|n| frames.max(idle_frames) >= n)
        {
            break;
        }

        let pal = session.as_ref().is_some_and(|s| s.pal);
        let wait = if !running {
            PAUSE_POLL
        } else if fast_forward || (vsync && display_matches(refresh_hz, pal)) {
            // The blocking present paces the loop on the display.
            Duration::ZERO
        } else {
            // Even frames on the clock; the audio queue only nudges the
            // schedule against the sound card's drift.
            if let Some(audio) = &audio {
                clock.nudge(audio_drift(audio.queued_pairs(), target_frames, pal));
            }
            clock.wait(start.elapsed(), frames)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        game_windowed = video::windowed_area(&game_canvas).unwrap_or(game_windowed);
        lib_windowed = video::windowed_area(&lib_canvas).unwrap_or(lib_windowed);
    }
    if let Some(s) = &mut session {
        s.close()?;
    }
    if let Some(player) = &agent {
        eprintln!(
            "AI: {} decisions, {} episodes over, {} finished",
            player.decisions, player.episodes, player.finished
        );
    }
    // Remembered for the next launch (same displays, same places).
    let mut config = crate::config::Config::load();
    config.game_window = Some(WindowState {
        area: video::windowed_area(&game_canvas).unwrap_or(game_windowed),
        fullscreen: video::is_fullscreen(&game_canvas),
    });
    config.library_window = Some(WindowState {
        area: video::windowed_area(&lib_canvas).unwrap_or(lib_windowed),
        fullscreen: video::is_fullscreen(&lib_canvas),
    });
    config.save();
    Ok(())
}

/// Brings a window to the front with the keyboard focus:
/// SDL_SetWindowInputFocus asks for the focus outright under X11 (and
/// XWayland). The window manager may still refuse (focus stealing
/// prevention), hence `keys_to_game`.
fn focus_window(canvas: &mut sdl2::render::Canvas<sdl2::video::Window>) {
    canvas.window_mut().raise();
    input_focus(canvas);
}

/// Asks for the keyboard focus without raising the window (the mouse
/// entered it).
fn input_focus(canvas: &sdl2::render::Canvas<sdl2::video::Window>) {
    // SAFETY: the raw handle comes from a live window owned by `canvas`;
    // SDL_SetWindowInputFocus only reads it (it returns an error where the
    // video driver cannot give the focus, which is fine to ignore).
    unsafe {
        sdl2::sys::SDL_SetWindowInputFocus(canvas.window().raw());
    }
}

/// Largest dragged cover image and its opacity (out of 255).
const GHOST_W: usize = 120;
const GHOST_H: usize = 168;
const GHOST_ALPHA: u8 = 170;

/// A cursor showing `picture` see-through, held by its centre (SDL copies
/// the pixels: the buffer may go once the cursor is made).
fn ghost_cursor(picture: &video::Preview) -> Option<sdl2::mouse::Cursor> {
    let image = boxart::Image {
        pixels: picture.pixels.clone(),
        width: picture.width,
        height: picture.height,
    };
    let ghost = boxart::drag_ghost(&image, GHOST_W, GHOST_H, GHOST_ALPHA);
    let (w, h) = (ghost.width as u32, ghost.height as u32);
    let mut bytes: Vec<u8> = ghost.pixels.iter().flat_map(|p| p.to_ne_bytes()).collect();
    let surface = sdl2::surface::Surface::from_data(
        &mut bytes,
        w,
        h,
        w * 4,
        sdl2::pixels::PixelFormatEnum::RGBA8888,
    )
    .ok()?;
    sdl2::mouse::Cursor::from_surface(surface, w as i32 / 2, h as i32 / 2).ok()
}

/// Whether the `curl` program can be run (box art downloads).
fn curl_available() -> bool {
    std::process::Command::new("curl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The cover of the ROM at `rom` (own picture, else cached libretro one).
fn cover_preview(rom: &Path, cache: Option<&Path>) -> Option<video::Preview> {
    let image = boxart::load_cover(&boxart::cover_source(rom, cache)?)?;
    Some(video::Preview {
        pixels: image.pixels,
        width: image.width,
        height: image.height,
    })
}

/// Picture of the latest save state of the ROM at `rom`, for the Library
/// preview: the ROM is loaded into a fresh console and the state applied
/// (states hold the frame that was on screen). None without a state.
fn rom_preview(rom: &Path) -> Option<video::Preview> {
    let bytes = std::fs::read(latest_state(rom)?).ok()?;
    let cart = md_core::Cartridge::from_bytes(&crate::archive::read_rom(rom).ok()?).ok()?;
    let config = md_core::MdConfig {
        region: md_core::Region::detect(cart.region_field()),
        ..md_core::MdConfig::default()
    };
    let mut console = Console::new(cart, config);
    console.load_state(&bytes).ok()?;
    Some(video::Preview {
        pixels: console.framebuffer().to_vec(),
        width: console.screen_width(),
        height: console.screen_height(),
    })
}

/// Mouse input for our window chrome.
enum WindowInput {
    Moved(i32, i32),
    Left,
    Click(Button),
}

/// Mouse events concerning the bar: motion (shows it), leaving the window
/// (hides it), left clicks on its buttons. A double click on the bar is
/// not used: the window manager takes the clicks of the drag area.
fn window_event(event: &Event, layout: &Layout) -> Option<WindowInput> {
    match *event {
        Event::MouseMotion { x, y, .. } => Some(WindowInput::Moved(x, y)),
        Event::Window {
            win_event: WindowEvent::Leave,
            ..
        } => Some(WindowInput::Left),
        Event::MouseButtonDown {
            mouse_btn: MouseButton::Left,
            x,
            y,
            ..
        } => match layout.hit(x, y) {
            Hit::Button(button) => Some(WindowInput::Click(button)),
            _ => None,
        },
        _ => None,
    }
}

/// Picture of the state saved in `slot` for `rom`: the state is loaded into
/// a copy of `console` (states hold the frame that was on screen), so the
/// game itself is untouched. None when the slot is empty or invalid.
fn state_preview(console: &Console, rom: &Path, slot: u8) -> Option<video::Preview> {
    let bytes = std::fs::read(state_path(rom, slot)).ok()?;
    let mut copy = console.clone();
    copy.load_state(&bytes).ok()?;
    Some(video::Preview {
        pixels: copy.framebuffer().to_vec(),
        width: copy.screen_width(),
        height: copy.screen_height(),
    })
}

/// F5: writes the state of the current slot; returns the on-screen
/// message (a failure is reported, the game goes on).
fn save_state(console: &mut Console, rom: &Path, slot: u8) -> String {
    let path = state_path(rom, slot);
    match std::fs::write(&path, console.save_state()) {
        Ok(()) => {
            println!("saved {}", path.display());
            format!("State {slot} saved")
        }
        Err(e) => {
            eprintln!("cannot write {}: {e}", path.display());
            format!("Cannot save state {slot}")
        }
    }
}

/// F8: loads the state of the current slot. A missing or invalid file is
/// reported and the game goes on unchanged. Returns the on-screen message.
fn load_state(console: &mut Console, rom: &Path, slot: u8) -> String {
    let path = state_path(rom, slot);
    let Ok(bytes) = std::fs::read(&path) else {
        return format!("No state in slot {slot}");
    };
    match console.load_state(&bytes) {
        Ok(()) => {
            println!("loaded {}", path.display());
            format!("State {slot} loaded")
        }
        Err(e) => {
            eprintln!("cannot load {}: {e}", path.display());
            format!("State {slot} is invalid")
        }
    }
}

/// F12: the current frame as `screenshots/frame_<n>.png`; returns the
/// on-screen message.
fn screenshot(console: &Console) -> anyhow::Result<String> {
    let path =
        PathBuf::from(SCREENSHOT_DIR).join(format!("frame_{:06}.png", console.frame_count()));
    crate::headless::write_png(&path, console)
        .with_context(|| format!("screenshot {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok("Screenshot saved".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu preview shows the frame saved in the slot, and leaves the
    /// running game untouched.
    #[test]
    fn state_preview_shows_the_saved_frame() {
        let dir = std::env::temp_dir().join(format!("md-rs-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("showcase.bin");
        let cart = md_core::Cartridge::from_bytes(&md_core::testrom::showcase()).unwrap();
        let mut console = Console::new(cart, md_core::MdConfig::default());
        for _ in 0..3 {
            console.run_frame();
        }
        let saved_frame = console.framebuffer().to_vec();
        assert_eq!(save_state(&mut console, &rom, 2), "State 2 saved");
        for _ in 0..40 {
            console.run_frame(); // showcase scrolls: the picture moves on
        }
        let before = console.frame_count();
        let preview = state_preview(&console, &rom, 2).expect("slot 2 has a state");
        assert_eq!(preview.pixels, saved_frame);
        assert_eq!((preview.width, preview.height), (320, 224));
        assert_eq!(console.frame_count(), before, "the game is untouched");
        assert!(state_preview(&console, &rom, 5).is_none(), "empty slot");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Library preview of a game that is not running: its ROM file
    /// and latest state give the frame that was saved.
    #[test]
    fn rom_preview_uses_the_latest_state() {
        let dir = std::env::temp_dir().join(format!("md-rs-rom-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("showcase.bin");
        std::fs::write(&rom, md_core::testrom::showcase()).unwrap();
        assert!(rom_preview(&rom).is_none(), "no state yet");
        let cart = md_core::Cartridge::from_bytes(&md_core::testrom::showcase()).unwrap();
        let mut console = Console::new(cart, md_core::MdConfig::default());
        for _ in 0..5 {
            console.run_frame();
        }
        assert_eq!(save_state(&mut console, &rom, 4), "State 4 saved");
        let preview = rom_preview(&rom).expect("state 4 exists");
        assert_eq!(preview.pixels, console.framebuffer());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_files_sit_next_to_the_rom() {
        assert_eq!(
            state_path(Path::new("roms/test/showcase.bin"), 3),
            PathBuf::from("roms/test/showcase.state3")
        );
    }
}

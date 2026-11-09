//! The ROM library shown in the Library window, independent of SDL: scan of
//! a folder (in a background thread), filter, selection and scrolling, and
//! the files next to a ROM (save states).
//!
//! The ROMs are the user's own files, read on their machine to get the
//! header titles; tests only use generated ROMs (CLAUDE.md rule 7).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime};

use md_core::{Cartridge, Region};

/// Extensions of the files the library lists.
const ROM_EXTENSIONS: [&str; 5] = ["bin", "md", "gen", "smd", "zip"];
/// Folder depth explored below the library folder.
const MAX_DEPTH: usize = 8;
/// Sub-folders of the library folder left out of the list: `test` holds the
/// project's homebrew test ROMs (roms/test, CLAUDE.md).
const EXCLUDED_DIRS: [&str; 1] = ["test"];
/// Save state slots (`<rom>.state0` to `.state9`).
const STATE_SLOTS: u8 = 10;

/// File of save state `slot` for the ROM at `rom`: `<rom>.state<slot>`.
pub fn state_path(rom: &Path, slot: u8) -> PathBuf {
    rom.with_extension(format!("state{slot}"))
}

/// The most recently written save state of `rom`, if any.
pub fn latest_state(rom: &Path) -> Option<PathBuf> {
    (0..STATE_SLOTS)
        .map(|slot| state_path(rom, slot))
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// A game of the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub title: String,
    pub region: Region,
    /// ROM image size in bytes (unpacked).
    pub size: usize,
    /// The file as it was when read: an unchanged file keeps its entry.
    pub stamp: Stamp,
    /// What the header says (info panel).
    pub info: Vec<crate::gameinfo::InfoLine>,
}

/// Size and modification time of a file: enough to notice an added,
/// removed, replaced or rewritten ROM without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

impl Entry {
    /// "EU", "US", "JP", "JP PAL", for the list.
    pub fn region_tag(&self) -> &'static str {
        match self.region {
            Region::Usa => "US",
            Region::Europe => "EU",
            Region::JapanNtsc => "JP",
            Region::JapanPal => "JP PAL",
        }
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn is_rom_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ROM_EXTENSIONS.iter().any(|r| e.eq_ignore_ascii_case(r)))
}

/// The ROM at `path` as a library entry, or None when it is not a loadable
/// ROM (archive without ROM, too small, unreadable).
fn entry(path: &Path) -> Option<Entry> {
    let stamp = stamp(path)?;
    let data = crate::archive::read_rom(path).ok()?;
    let cart = Cartridge::from_bytes(&data).ok()?;
    let header = cart.header();
    Some(Entry {
        path: path.to_path_buf(),
        title: crate::chrome::game_title(&header.overseas_title, &header.domestic_title, path),
        region: Region::detect(cart.region_field()),
        size: cart.rom().len(),
        stamp,
        info: crate::gameinfo::game_info(&cart),
    })
}

/// The ROMs under `dir` (sub-folders included), sorted by title then path.
#[cfg(test)]
pub fn scan(dir: &Path) -> Vec<Entry> {
    rescan(dir, &[])
}

/// `scan` reusing the entries of `previous` whose file is unchanged (same
/// path, size and modification time): only new or rewritten files are read.
pub fn rescan(dir: &Path, previous: &[Entry]) -> Vec<Entry> {
    let mut files = Vec::new();
    collect(dir, 0, &mut files);
    let mut entries: Vec<Entry> = files
        .iter()
        .filter_map(|p| {
            let known = previous.iter().find(|e| &e.path == p);
            match known {
                Some(e) if stamp(p) == Some(e.stamp) => Some(e.clone()),
                _ => entry(p),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
}

fn collect(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        let excluded = depth == 0 && EXCLUDED_DIRS.iter().any(|d| item.file_name() == *d);
        match item.file_type() {
            Ok(t) if t.is_dir() && depth < MAX_DEPTH && !excluded => {
                collect(&path, depth + 1, files)
            }
            Ok(t) if !t.is_dir() && is_rom_file(&path) => files.push(path),
            _ => {}
        }
    }
}

/// The ROM files under `dir` and their stamps, sorted: compared between
/// two looks at the folder to notice additions, removals and rewrites.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Option<Stamp>)> {
    let mut files = Vec::new();
    collect(dir, 0, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let s = stamp(&p);
            (p, s)
        })
        .collect()
}

/// How often the library folder is looked at for changes.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Keeps the list in step with the library folder, in a background thread:
/// a first scan, then a look every `interval` (file names, sizes and dates
/// only); when something changed, a rescan (unchanged files reused) is sent.
/// Dropping the watcher stops the thread.
pub struct Watcher {
    pub updates: Receiver<Vec<Entry>>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    pub fn start(dir: PathBuf, interval: Duration) -> Watcher {
        let (sender, updates) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut seen = snapshot(&dir);
            let mut entries = rescan(&dir, &[]);
            if sender.send(entries.clone()).is_err() {
                return;
            }
            while !stopped.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                let now = snapshot(&dir);
                if now != seen {
                    seen = now;
                    entries = rescan(&dir, &entries);
                    if sender.send(entries.clone()).is_err() {
                        return;
                    }
                }
            }
        });
        Watcher { updates, stop }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Games added and removed between two lists (for the status line).
pub fn changes(before: &[Entry], after: &[Entry]) -> (usize, usize) {
    let added = after
        .iter()
        .filter(|a| !before.iter().any(|b| b.path == a.path))
        .count();
    let removed = before
        .iter()
        .filter(|b| !after.iter().any(|a| a.path == b.path))
        .count();
    (added, removed)
}

/// The list as the Library window shows it: entries matching the filter, the
/// selected one, and the first row on screen.
#[derive(Debug, Clone, Default)]
pub struct Library {
    entries: Vec<Entry>,
    filter: String,
    /// Indices into `entries` of the entries matching the filter.
    visible: Vec<usize>,
    /// Position of the selection in `visible`.
    selected: usize,
    /// First visible row on screen.
    pub scroll: usize,
}

impl Library {
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let keep = self.selected().map(|e| e.path.clone());
        self.entries = entries;
        self.refilter(keep);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn push_filter(&mut self, c: char) {
        let keep = self.selected().map(|e| e.path.clone());
        self.filter.push(c);
        self.refilter(keep);
    }

    pub fn pop_filter(&mut self) {
        let keep = self.selected().map(|e| e.path.clone());
        self.filter.pop();
        self.refilter(keep);
    }

    /// Keeps the entries whose title or file name contains the filter
    /// (ignoring case), and the selected entry if it still matches.
    fn refilter(&mut self, keep: Option<PathBuf>) {
        let needle = self.filter.to_lowercase();
        self.visible = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                needle.is_empty()
                    || e.title.to_lowercase().contains(&needle)
                    || e.file_name().to_lowercase().contains(&needle)
            })
            .collect();
        self.selected = keep
            .and_then(|path| {
                self.visible
                    .iter()
                    .position(|&i| self.entries[i].path == path)
            })
            .unwrap_or(0);
        self.scroll = self.scroll.min(self.selected);
    }

    /// Entries matching the filter, in order.
    pub fn visible(&self) -> impl Iterator<Item = &Entry> {
        self.visible.iter().map(|&i| &self.entries[i])
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.visible.get(self.selected).map(|&i| &self.entries[i])
    }

    /// Moves the selection by `delta` rows, within the list.
    pub fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    pub fn home(&mut self) {
        self.selected = 0;
    }

    pub fn end(&mut self) {
        self.selected = self.visible.len().saturating_sub(1);
    }

    /// Selects row `row` of the visible list (mouse).
    pub fn select(&mut self, row: usize) {
        if row < self.visible.len() {
            self.selected = row;
        }
    }

    /// Grid view: scrolls by whole rows of `cols` so that the selection is
    /// among the `rows` rows on screen (`scroll` stays a multiple of
    /// `cols`).
    pub fn keep_selection_visible_grid(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let row = self.selected / cols;
        let first = self.scroll / cols;
        let first = if row < first {
            row
        } else if row >= first + rows {
            row + 1 - rows
        } else {
            first
        };
        self.scroll = first * cols;
    }

    /// Scrolls so that the selection is among the `rows` rows on screen.
    pub fn keep_selection_visible(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }
}

/// Geometry of the Library window below its bar: filter line, list (left),
/// preview (right, at the picture's 320:224 aspect) and status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryLayout {
    /// The menu tabs' row, at the top (`menu::menu_layout` places the tabs).
    pub tabs: crate::chrome::Rect,
    /// The List / Covers switch at the right end of the tabs' row: its two
    /// halves.
    pub view_list: crate::chrome::Rect,
    pub view_covers: crate::chrome::Rect,
    /// List, picture and info panel together: the cover grid's area.
    pub content: crate::chrome::Rect,
    pub filter: crate::chrome::Rect,
    pub list: crate::chrome::Rect,
    pub row_h: u32,
    /// Rows that fit in the list.
    pub rows: usize,
    pub preview: crate::chrome::Rect,
    /// Info panel under the picture, one `info_row_h` line per field.
    pub info: crate::chrome::Rect,
    pub info_row_h: u32,
    pub info_scale: u32,
    pub status: crate::chrome::Rect,
    pub scale: u32,
}

/// Width of each half of the List / Covers switch, in characters.
const VIEW_SWITCH_CHARS: u32 = 8;

/// How the Library shows its games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Titles, with the selected game's cover and info beside.
    #[default]
    List,
    /// Covers only, in a grid.
    Covers,
}

impl ViewMode {
    pub fn toggled(self) -> ViewMode {
        match self {
            ViewMode::List => ViewMode::Covers,
            ViewMode::Covers => ViewMode::List,
        }
    }
}

/// The cover grid in `area`: cells of a box's shape (512:720), as many
/// columns as fit, the rows fully visible counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub area: crate::chrome::Rect,
    pub cols: usize,
    pub rows: usize,
    pub cell_w: u32,
    pub cell_h: u32,
    pub gap: u32,
}

/// Cover cell width per text scale step.
const CELL_UNIT: u32 = 64;

pub fn grid_layout(area: crate::chrome::Rect, scale: u32) -> Grid {
    let gap = 4 * scale;
    let cell_w = CELL_UNIT * scale;
    let cell_h = cell_w * 720 / 512;
    let cols = ((area.w + gap) / (cell_w + gap)).max(1) as usize;
    let rows = ((area.h + gap) / (cell_h + gap)).max(1) as usize;
    Grid {
        area,
        cols,
        rows,
        cell_w,
        cell_h,
        gap,
    }
}

impl Grid {
    /// Cell of the `i`-th game shown (counted from the first one on screen).
    pub fn cell_rect(&self, i: usize) -> crate::chrome::Rect {
        let (col, row) = ((i % self.cols) as u32, (i / self.cols) as u32);
        crate::chrome::Rect {
            x: self.area.x + (col * (self.cell_w + self.gap)) as i32,
            y: self.area.y + (row * (self.cell_h + self.gap)) as i32,
            w: self.cell_w,
            h: self.cell_h,
        }
    }

    /// Games on screen: the fully visible rows.
    pub fn shown(&self) -> usize {
        self.cols * self.rows
    }

    /// Which game shown is under window pixel (`x`, `y`), counted from the
    /// first one on screen.
    pub fn cell_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.shown()).find(|&i| self.cell_rect(i).contains(x, y))
    }
}

/// Lines of the info panel (`gameinfo::game_info`).
pub const INFO_LINES: u32 = 6;

/// Share of the width given to the list.
const LIST_PERCENT: u32 = 55;

/// A `width` × `height` picture scaled to fit `area` with its proportions
/// kept, centred horizontally and at the top.
pub fn fit_picture(area: crate::chrome::Rect, width: u32, height: u32) -> crate::chrome::Rect {
    let (width, height) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let (aw, ah) = (u64::from(area.w), u64::from(area.h));
    let (w, h) = if aw * height <= ah * width {
        (aw, aw * height / width)
    } else {
        (ah * width / height, ah)
    };
    crate::chrome::Rect {
        x: area.x + ((aw - w) / 2) as i32,
        y: area.y,
        w: w.max(1) as u32,
        h: h.max(1) as u32,
    }
}

pub fn library_layout(chrome: &crate::chrome::Layout) -> LibraryLayout {
    use crate::chrome::{GLYPH, Rect};
    let s = chrome.scale;
    let pad = 4 * s as i32;
    let line_h = GLYPH * s + 4 * s;
    let (w, h) = (chrome.width as i32, chrome.height as i32);
    let tabs = Rect {
        x: pad,
        y: chrome.bar.h as i32 + pad,
        w: (w - 2 * pad).max(1) as u32,
        h: line_h,
    };
    let filter = Rect {
        y: tabs.y + line_h as i32 + pad,
        ..tabs
    };
    let status = Rect {
        x: pad,
        y: (h - pad - line_h as i32).max(0),
        w: filter.w,
        h: line_h,
    };
    let top = filter.y + line_h as i32 + pad;
    let content_h = (status.y - pad - top).max(1) as u32;
    let list_w = ((w - 3 * pad).max(2) as u32) * LIST_PERCENT / 100;
    let list = Rect {
        x: pad,
        y: top,
        w: list_w,
        h: content_h,
    };
    // The right column: the picture (portrait covers, landscape states,
    // fitted by `fit_picture`) above the info panel. The panel's text is
    // one step smaller than the list's from scale 2.
    let info_scale = if s > 1 { s - 1 } else { 1 };
    let info_row_h = GLYPH * info_scale + 3 * info_scale;
    let column_w = (w - 3 * pad - list_w as i32).max(1) as u32;
    let info_h = (INFO_LINES * info_row_h).min(content_h / 2);
    let preview = Rect {
        x: list.x + list_w as i32 + pad,
        y: top,
        w: column_w,
        h: content_h.saturating_sub(info_h + pad as u32).max(1),
    };
    let info = Rect {
        x: preview.x,
        y: top + content_h as i32 - info_h as i32,
        w: column_w,
        h: info_h.max(1),
    };
    let switch_w = VIEW_SWITCH_CHARS * GLYPH * s;
    let view_covers = Rect {
        x: tabs.x + tabs.w as i32 - switch_w as i32,
        w: switch_w,
        ..tabs
    };
    let view_list = Rect {
        x: view_covers.x - switch_w as i32,
        ..view_covers
    };
    let content = Rect {
        x: list.x,
        y: list.y,
        w: (preview.x + preview.w as i32 - list.x) as u32,
        h: list.h,
    };
    LibraryLayout {
        tabs,
        view_list,
        view_covers,
        content,
        filter,
        rows: (content_h / line_h) as usize,
        row_h: line_h,
        list,
        preview,
        info,
        info_row_h,
        info_scale,
        status,
        scale: s,
    }
}

impl LibraryLayout {
    /// List row (counted from the first row on screen) under (`x`, `y`).
    pub fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.list.contains(x, y) {
            return None;
        }
        let row = ((y - self.list.y) as u32 / self.row_h) as usize;
        (row < self.rows).then_some(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-rs-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Generated ROM with overseas title `title` and country `country`.
    fn rom(title: &str, country: &[u8; 4]) -> Vec<u8> {
        let mut rom = md_core::testrom::showcase();
        let mut field = [b' '; 48];
        field[..title.len()].copy_from_slice(title.as_bytes());
        rom[0x150..0x180].copy_from_slice(&field);
        rom[0x1F0..0x1F4].copy_from_slice(country);
        rom
    }

    fn zip_of(name: &str, data: &[u8]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn sample_library(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        std::fs::write(dir.join("zeta.bin"), rom("ZETA QUEST", b"E   ")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/alpha.MD"), rom("ALPHA STRIKE", b"U   ")).unwrap();
        std::fs::write(
            dir.join("mid.zip"),
            zip_of("mid.bin", &rom("Mid Game", b"J   ")),
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a ROM").unwrap();
        // The test folder is left out; a deeper one named test is not.
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::write(dir.join("test/hidden.bin"), rom("HIDDEN TEST ROM", b"U   ")).unwrap();
        std::fs::write(dir.join("broken.bin"), b"too small").unwrap();
        dir
    }

    #[test]
    fn scan_finds_roms_with_title_and_region() {
        let dir = sample_library("scan");
        let entries = scan(&dir);
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            ["ALPHA STRIKE", "Mid Game", "ZETA QUEST"],
            "sorted, case-insensitive"
        );
        let tags: Vec<&str> = entries.iter().map(Entry::region_tag).collect();
        assert_eq!(tags, ["US", "JP", "EU"]);
        assert!(
            entries[1].path.ends_with("mid.zip"),
            "zip archives included"
        );
        assert!(entries.iter().all(|e| e.size >= 0x1000));
        let watcher = Watcher::start(dir.clone(), Duration::from_millis(20));
        assert_eq!(watcher.updates.recv().unwrap(), entries, "first scan");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only the library's own `test` folder is left out (roms/test holds
    /// the project's test ROMs); a deeper folder named test is listed.
    #[test]
    fn test_folder_excluded_at_the_top_only() {
        let dir = temp_dir("excluded");
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::write(dir.join("test/hidden.bin"), rom("HIDDEN", b"U   ")).unwrap();
        std::fs::create_dir_all(dir.join("sub/test")).unwrap();
        std::fs::write(dir.join("sub/test/deep.bin"), rom("DEEP GAME", b"U   ")).unwrap();
        std::fs::write(dir.join("top.bin"), rom("TOP GAME", b"U   ")).unwrap();
        let titles: Vec<String> = scan(&dir).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["DEEP GAME", "TOP GAME"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Adding, rewriting and removing ROMs in the folder sends updated
    /// lists; unchanged files keep their entry.
    #[test]
    fn watcher_follows_the_folder() {
        let dir = temp_dir("watch");
        std::fs::write(dir.join("one.bin"), rom("ONE", b"U   ")).unwrap();
        let watcher = Watcher::start(dir.clone(), Duration::from_millis(20));
        let wait = || {
            watcher
                .updates
                .recv_timeout(Duration::from_secs(5))
                .expect("an update")
        };
        let first = wait();
        assert_eq!(first.len(), 1);
        std::fs::write(dir.join("two.bin"), rom("TWO", b"E   ")).unwrap();
        let second = wait();
        assert_eq!(
            second.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(),
            ["ONE", "TWO"]
        );
        assert_eq!(changes(&first, &second), (1, 0));
        std::fs::remove_file(dir.join("one.bin")).unwrap();
        let third = wait();
        assert_eq!(third.len(), 1);
        assert_eq!(changes(&second, &third), (0, 1));
        // Rewritten with another title: read again.
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(dir.join("two.bin"), rom("TWO REVISED", b"E   ")).unwrap();
        assert_eq!(wait()[0].title, "TWO REVISED");
        drop(watcher);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rescan_reuses_unchanged_entries() {
        let dir = temp_dir("rescan");
        std::fs::write(dir.join("a.bin"), rom("A", b"U   ")).unwrap();
        let mut known = scan(&dir);
        known[0].title = "CACHED".into(); // as if read earlier
        assert_eq!(
            rescan(&dir, &known)[0].title,
            "CACHED",
            "unchanged file not read"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filter_navigation_and_kept_selection() {
        let dir = sample_library("filter");
        let mut lib = Library::default();
        lib.set_entries(scan(&dir));
        assert_eq!(lib.visible_len(), 3);
        lib.move_by(1);
        assert_eq!(lib.selected().unwrap().title, "Mid Game");
        lib.move_by(10);
        assert_eq!(
            lib.selected().unwrap().title,
            "ZETA QUEST",
            "stops at the end"
        );
        lib.move_by(-10);
        assert_eq!(lib.selected_index(), 0, "stops at the top");
        lib.end();
        for c in "ZETA".chars() {
            lib.push_filter(c.to_ascii_lowercase());
        }
        assert_eq!(lib.visible_len(), 1);
        assert_eq!(
            lib.selected().unwrap().title,
            "ZETA QUEST",
            "selection kept"
        );
        lib.pop_filter();
        lib.pop_filter();
        lib.pop_filter();
        lib.pop_filter();
        lib.push_filter('m');
        lib.push_filter('i');
        lib.push_filter('d');
        assert_eq!(
            lib.selected().unwrap().title,
            "Mid Game",
            "file name matches too"
        );
        lib.push_filter('x');
        assert_eq!(lib.selected(), None, "nothing matches");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scrolling_follows_the_selection() {
        let mut lib = Library::default();
        lib.set_entries(
            (0..20)
                .map(|i| Entry {
                    path: PathBuf::from(format!("{i:02}.bin")),
                    title: format!("GAME {i:02}"),
                    region: Region::Usa,
                    size: 0x1000,
                    stamp: Stamp {
                        len: 0x1000,
                        modified: None,
                    },
                    info: Vec::new(),
                })
                .collect(),
        );
        lib.move_by(12);
        lib.keep_selection_visible(5);
        assert_eq!(lib.scroll, 8, "row 12 is the last of 5 rows");
        lib.move_by(-10);
        lib.keep_selection_visible(5);
        assert_eq!(lib.scroll, 2);
    }

    #[test]
    fn layout_areas_fit_the_window() {
        let chrome = crate::chrome::Layout::new(960, 640, true);
        let l = library_layout(&chrome);
        let within = |r: &crate::chrome::Rect| {
            r.x >= 0
                && r.y >= chrome.bar.h as i32
                && r.x + r.w as i32 <= 960
                && r.y + r.h as i32 <= 640
        };
        assert!(
            [l.tabs, l.filter, l.list, l.preview, l.status]
                .iter()
                .all(within)
        );
        assert!(
            l.tabs.y + l.tabs.h as i32 <= l.filter.y,
            "tabs above the search line"
        );
        assert!(
            within(&l.view_list) && within(&l.view_covers),
            "view switch in the window"
        );
        assert_eq!(
            l.view_covers.x + l.view_covers.w as i32,
            l.tabs.x + l.tabs.w as i32,
            "at the right end"
        );
        assert_eq!(
            (l.content.x, l.content.y, l.content.h),
            (l.list.x, l.list.y, l.list.h)
        );
        assert_eq!(
            l.content.x + l.content.w as i32,
            l.preview.x + l.preview.w as i32
        );
        assert!(
            l.list.x + l.list.w as i32 <= l.preview.x,
            "list left of the preview"
        );
        assert!(l.rows >= 10, "{} rows", l.rows);
        assert!(within(&l.info), "info panel inside the window");
        assert!(
            l.preview.y + l.preview.h as i32 <= l.info.y,
            "picture above the info panel"
        );
        assert!(
            l.info.h >= INFO_LINES * l.info_row_h,
            "every info line fits"
        );
        assert_eq!(
            l.info.y + l.info.h as i32,
            l.list.y + l.list.h as i32,
            "panel at the bottom"
        );
        assert_eq!(l.row_at(l.list.x + 1, l.list.y + 1), Some(0));
        assert_eq!(
            l.row_at(l.list.x + 1, l.list.y + l.row_h as i32 + 1),
            Some(1)
        );
        assert_eq!(l.row_at(l.preview.x + 1, l.preview.y + 1), None);
    }

    #[test]
    fn cover_grid_geometry() {
        use crate::chrome::Rect;
        let area = Rect {
            x: 8,
            y: 100,
            w: 884,
            h: 420,
        };
        let g = grid_layout(area, 2);
        assert_eq!((g.cell_w, g.cell_h), (128, 180), "a box's shape");
        assert_eq!(
            (g.cols, g.rows),
            (6, 2),
            "(884 + 8) / 136 columns, (420 + 8) / 188 rows"
        );
        for i in 0..g.shown() {
            let r = g.cell_rect(i);
            assert!(
                r.x + r.w as i32 <= area.x + area.w as i32
                    && r.y + r.h as i32 <= area.y + area.h as i32
            );
            assert_eq!(g.cell_at(r.x + 1, r.y + 1), Some(i));
        }
        assert_eq!(g.cell_at(area.x + 128 + 2, area.y + 1), None, "in the gap");
        let tiny = grid_layout(
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            2,
        );
        assert_eq!((tiny.cols, tiny.rows), (1, 1), "always one cell");
    }

    #[test]
    fn grid_navigation_scrolls_whole_rows() {
        let mut lib = Library::default();
        lib.set_entries(
            (0..20)
                .map(|i| Entry {
                    path: PathBuf::from(format!("/r/{i:02}.bin")),
                    title: format!("GAME {i:02}"),
                    region: Region::Usa,
                    size: 0,
                    stamp: Stamp {
                        len: 0,
                        modified: None,
                    },
                    info: Vec::new(),
                })
                .collect(),
        );
        let (cols, rows) = (6, 2);
        lib.move_by(cols as isize); // down a row
        lib.move_by(cols as isize);
        lib.keep_selection_visible_grid(cols, rows);
        assert_eq!(
            (lib.selected_index(), lib.scroll),
            (12, 6),
            "one row scrolled"
        );
        lib.move_by(cols as isize);
        lib.move_by(cols as isize);
        assert_eq!(lib.selected_index(), 19, "bounded at the last game");
        lib.keep_selection_visible_grid(cols, rows);
        assert_eq!(lib.scroll, 12);
        lib.home();
        lib.keep_selection_visible_grid(cols, rows);
        assert_eq!(lib.scroll, 0);
        assert_eq!(ViewMode::List.toggled(), ViewMode::Covers);
    }

    #[test]
    fn pictures_fit_with_their_proportions() {
        use crate::chrome::Rect;
        let area = Rect {
            x: 10,
            y: 20,
            w: 400,
            h: 500,
        };
        // A portrait cover is as tall as the area, centred.
        assert_eq!(
            fit_picture(area, 600, 1000),
            Rect {
                x: 60,
                y: 20,
                w: 300,
                h: 500
            }
        );
        // A landscape state is as wide as the area, at the top.
        assert_eq!(
            fit_picture(area, 320, 224),
            Rect {
                x: 10,
                y: 20,
                w: 400,
                h: 280
            }
        );
    }

    #[test]
    fn latest_state_is_the_newest_slot() {
        let dir = temp_dir("states");
        let rom = dir.join("game.bin");
        assert_eq!(latest_state(&rom), None);
        std::fs::write(state_path(&rom, 7), b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(state_path(&rom, 2), b"new").unwrap();
        assert_eq!(latest_state(&rom), Some(dir.join("game.state2")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

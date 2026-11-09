//! The Library window's menu (Esc, the bar's menu button, or the
//! controller's guide button), independent of SDL: three sections (the game
//! running, the Library, the application), items greyed out when they do
//! not apply, a save state slot page, navigation from keys and controller
//! buttons, and the geometry used to draw it and hit it with the mouse.

use crate::chrome::{GLYPH, Layout, Rect};

/// Save state slots 0-9 (the same as the 0-9 keys).
pub const SLOTS: u8 = 10;
/// Slots per row of the slot page.
pub const SLOT_COLUMNS: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Game,
    Library,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Resume,
    SaveState,
    LoadState,
    Reset,
    StopGame,
    AiPlayer,
    RomFolder,
    ChooseCover,
    ResetCover,
    CoverGrid,
    Fullscreen,
    ShowFps,
    Quit,
}

/// The tabs, left to right.
pub const TABS: [Section; 3] = [Section::Game, Section::Library, Section::App];

/// The items of `section`'s tab, top to bottom.
pub fn tab_items(section: Section) -> &'static [Item] {
    match section {
        Section::Game => &[
            Item::Resume,
            Item::SaveState,
            Item::LoadState,
            Item::Reset,
            Item::StopGame,
            Item::AiPlayer,
        ],
        Section::Library => &[
            Item::RomFolder,
            Item::ChooseCover,
            Item::ResetCover,
            Item::CoverGrid,
        ],
        Section::App => &[Item::Fullscreen, Item::ShowFps, Item::Quit],
    }
}

/// Most items in one tab (sizes the panel).
const MAX_TAB_ITEMS: usize = 6;

/// What the menu needs to know of the application, given each frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuContext {
    /// Title of the game running.
    pub game: Option<String>,
    /// Title of the game selected in the Library list.
    pub selected_game: Option<String>,
    /// The frames per second are shown.
    pub fps: bool,
    /// The Library shows the cover grid.
    pub covers_view: bool,
    /// The AI player: None without --agent, else whether it plays.
    pub agent: Option<bool>,
    /// Slots holding a state of the game running.
    pub used_slots: [bool; SLOTS as usize],
}

/// What the window loop should do after a menu input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    None,
    /// The menu closed: emulation goes on.
    Resume,
    Save(u8),
    Load(u8),
    Reset,
    /// Stop the game running (its SRAM saved).
    StopGame,
    Fullscreen,
    ToggleFps,
    /// List / cover grid.
    ToggleView,
    /// The AI player takes pad 1 or gives it back.
    ToggleAgent,
    /// Choose the Library folder (folder dialog).
    ChooseFolder,
    /// Choose the selected game's cover among libretro's.
    ChooseCover,
    /// Forget the selected game's cover: looked for again.
    ResetCover,
    Quit,
}

/// A navigation input while the menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
}

/// Navigation from a key (SDL key name): arrows, Return / Space, Esc /
/// Backspace.
pub fn nav_from_key(name: &str) -> Option<Nav> {
    Some(match name {
        "Up" => Nav::Up,
        "Down" => Nav::Down,
        "Left" => Nav::Left,
        "Right" => Nav::Right,
        "Return" | "Space" | "Keypad Enter" => Nav::Activate,
        "Escape" | "Backspace" => Nav::Back,
        _ => return None,
    })
}

/// Navigation from a controller button (SDL name): d-pad, A or Start to
/// choose, B or the guide button to go back.
pub fn nav_from_button(name: &str) -> Option<Nav> {
    Some(match name {
        "dpup" => Nav::Up,
        "dpdown" => Nav::Down,
        "dpleft" => Nav::Left,
        "dpright" => Nav::Right,
        "a" | "start" => Nav::Activate,
        "b" | "guide" | "back" => Nav::Back,
        _ => return None,
    })
}

/// What the slot page does with the chosen slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAction {
    Save,
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Main,
    Slots(SlotAction),
}

/// A tab ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    pub label: &'static str,
    pub enabled: bool,
    pub selected: bool,
}

/// An item row ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub label: String,
    /// Keyboard shortcut reminder ("F5"), drawn on the right.
    pub hint: &'static str,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub open: bool,
    pub page: Page,
    /// Index into `TABS`.
    tab: usize,
    /// Index into the tab's items.
    selected: usize,
    /// Save state slot, shared with the 0-9 keys and F5 / F8.
    pub slot: u8,
}

/// Whether `item` applies now.
pub fn enabled(item: Item, ctx: &MenuContext) -> bool {
    match item {
        Item::Resume | Item::SaveState | Item::LoadState | Item::Reset | Item::StopGame => {
            ctx.game.is_some()
        }
        Item::AiPlayer => ctx.game.is_some() && ctx.agent.is_some(),
        Item::ChooseCover | Item::ResetCover => ctx.selected_game.is_some(),
        Item::RomFolder | Item::CoverGrid | Item::Fullscreen | Item::ShowFps | Item::Quit => true,
    }
}

/// Whether some item of `section`'s tab applies now.
pub fn tab_enabled(section: Section, ctx: &MenuContext) -> bool {
    tab_items(section).iter().any(|&i| enabled(i, ctx))
}

impl Menu {
    /// Opens on the Game tab with a game, else on the Library tab.
    pub fn open(&mut self, ctx: &MenuContext) {
        self.open = true;
        self.page = Page::Main;
        let section = if ctx.game.is_some() {
            Section::Game
        } else {
            Section::Library
        };
        self.show_tab(TABS.iter().position(|&t| t == section).unwrap(), ctx);
    }

    /// Shows tab `tab`, its first item that applies selected.
    fn show_tab(&mut self, tab: usize, ctx: &MenuContext) {
        self.tab = tab;
        self.selected = tab_items(TABS[tab])
            .iter()
            .position(|&i| enabled(i, ctx))
            .unwrap_or(0);
    }

    pub fn tab(&self) -> Section {
        TABS[self.tab]
    }

    pub fn selected_row(&self) -> usize {
        self.selected
    }

    pub fn selected_item(&self) -> Option<Item> {
        tab_items(self.tab()).get(self.selected).copied()
    }

    /// Index of the open tab in `TABS`.
    pub fn tab_index(&self) -> usize {
        self.tab
    }

    /// A click on tab `tab`: opens the menu on it, or closes the menu when
    /// it is that tab already open. A tab whose items do not apply does
    /// nothing.
    pub fn click_tab(&mut self, tab: usize, ctx: &MenuContext) -> Command {
        if tab >= TABS.len() || !tab_enabled(TABS[tab], ctx) {
            return Command::None;
        }
        if self.open && self.tab == tab {
            self.open = false;
            return Command::Resume;
        }
        self.open = true;
        self.page = Page::Main;
        self.show_tab(tab, ctx);
        Command::None
    }

    /// Selects row `row` of the tab (mouse hover) when it applies.
    pub fn select_row(&mut self, row: usize, ctx: &MenuContext) {
        let items = tab_items(self.tab());
        if self.page == Page::Main && items.get(row).is_some_and(|&i| enabled(i, ctx)) {
            self.selected = row;
        }
    }

    /// Selects slot `slot` (mouse hover on the slot page).
    pub fn select_slot(&mut self, slot: u8) {
        if slot < SLOTS {
            self.slot = slot;
        }
    }

    /// The next item of the tab that applies, `step` rows at a time (wraps).
    fn step(&mut self, step: isize, ctx: &MenuContext) {
        let items = tab_items(self.tab());
        let n = items.len() as isize;
        let mut row = self.selected as isize;
        for _ in 0..n {
            row = (row + step).rem_euclid(n);
            if enabled(items[row as usize], ctx) {
                self.selected = row as usize;
                return;
            }
        }
    }

    /// The next tab with items that apply, `step` tabs at a time (wraps).
    fn step_tab(&mut self, step: isize, ctx: &MenuContext) {
        let n = TABS.len() as isize;
        let mut tab = self.tab as isize;
        for _ in 0..n {
            tab = (tab + step).rem_euclid(n);
            if tab_enabled(TABS[tab as usize], ctx) {
                self.show_tab(tab as usize, ctx);
                return;
            }
        }
    }

    /// Applies a navigation input.
    pub fn nav(&mut self, nav: Nav, ctx: &MenuContext) -> Command {
        match self.page {
            Page::Main => match nav {
                Nav::Up => self.step(-1, ctx),
                Nav::Down => self.step(1, ctx),
                Nav::Left => self.step_tab(-1, ctx),
                Nav::Right => self.step_tab(1, ctx),
                Nav::Back => {
                    self.open = false;
                    return Command::Resume;
                }
                Nav::Activate => return self.activate(ctx),
            },
            Page::Slots(action) => {
                let (s, c) = (SLOTS, SLOT_COLUMNS);
                match nav {
                    Nav::Left => self.slot = (self.slot + s - 1) % s,
                    Nav::Right => self.slot = (self.slot + 1) % s,
                    Nav::Up => self.slot = (self.slot + s - c) % s,
                    Nav::Down => self.slot = (self.slot + c) % s,
                    Nav::Back => self.page = Page::Main,
                    Nav::Activate => return self.activate_slot(action, ctx),
                }
            }
        }
        Command::None
    }

    /// Chooses the selected item (or slot, on the slot page).
    pub fn activate(&mut self, ctx: &MenuContext) -> Command {
        if let Page::Slots(action) = self.page {
            return self.activate_slot(action, ctx);
        }
        let Some(item) = self.selected_item().filter(|&i| enabled(i, ctx)) else {
            return Command::None;
        };
        let command = match item {
            Item::SaveState => {
                self.page = Page::Slots(SlotAction::Save);
                return Command::None;
            }
            Item::LoadState => {
                self.page = Page::Slots(SlotAction::Load);
                return Command::None;
            }
            // Check boxes: the menu stays open.
            Item::ShowFps => return Command::ToggleFps,
            Item::CoverGrid => return Command::ToggleView,
            Item::Resume => Command::Resume,
            Item::Reset => Command::Reset,
            Item::StopGame => Command::StopGame,
            Item::AiPlayer => Command::ToggleAgent,
            Item::RomFolder => Command::ChooseFolder,
            Item::ChooseCover => Command::ChooseCover,
            Item::ResetCover => Command::ResetCover,
            Item::Fullscreen => Command::Fullscreen,
            Item::Quit => Command::Quit,
        };
        self.open = false;
        command
    }

    /// Saves to or loads from the selected slot; an empty slot cannot be
    /// loaded.
    fn activate_slot(&mut self, action: SlotAction, ctx: &MenuContext) -> Command {
        if action == SlotAction::Load && !ctx.used_slots[self.slot as usize] {
            return Command::None;
        }
        self.open = false;
        self.page = Page::Main;
        match action {
            SlotAction::Save => Command::Save(self.slot),
            SlotAction::Load => Command::Load(self.slot),
        }
    }

    /// The tabs to draw.
    pub fn tabs(&self, ctx: &MenuContext) -> Vec<TabView> {
        TABS.iter()
            .enumerate()
            .map(|(i, &section)| TabView {
                label: match section {
                    Section::Game => "Game",
                    Section::Library => "Library",
                    Section::App => "App",
                },
                enabled: tab_enabled(section, ctx),
                selected: self.open && i == self.tab,
            })
            .collect()
    }

    /// The line under the tabs: what the tab's items apply to.
    pub fn subtitle(&self, ctx: &MenuContext) -> String {
        match self.tab() {
            Section::Game => ctx.game.clone().unwrap_or_else(|| "No game running".into()),
            Section::Library => match &ctx.selected_game {
                Some(title) => format!("Selected: {title}"),
                None => "No game selected".into(),
            },
            Section::App => "megadrive-rs".into(),
        }
    }

    /// The active tab's item rows, labels and states from `ctx`.
    pub fn rows(&self, ctx: &MenuContext) -> Vec<RowView> {
        tab_items(self.tab())
            .iter()
            .enumerate()
            .map(|(i, &item)| RowView {
                label: label(item, ctx),
                hint: hint(item),
                enabled: enabled(item, ctx),
                selected: i == self.selected,
            })
            .collect()
    }
}

fn label(item: Item, ctx: &MenuContext) -> String {
    match item {
        Item::Resume => "Resume".into(),
        Item::SaveState => "Save state...".into(),
        Item::LoadState => "Load state...".into(),
        Item::Reset => "Reset".into(),
        Item::StopGame => "Stop game".into(),
        Item::AiPlayer => match ctx.agent {
            Some(true) => "AI player [x]".into(),
            Some(false) => "AI player [ ]".into(),
            None => "AI player (--agent)".into(),
        },
        Item::RomFolder => "ROM folder...".into(),
        Item::ChooseCover => match &ctx.selected_game {
            Some(title) => format!("Cover of \"{title}\"..."),
            None => "Choose cover...".into(),
        },
        Item::ResetCover => "Reset cover".into(),
        Item::CoverGrid => format!("Cover grid [{}]", if ctx.covers_view { 'x' } else { ' ' }),
        Item::Fullscreen => "Game fullscreen".into(),
        Item::ShowFps => format!("Show FPS [{}]", if ctx.fps { 'x' } else { ' ' }),
        Item::Quit => "Quit".into(),
    }
}

fn hint(item: Item) -> &'static str {
    match item {
        Item::SaveState => "F5",
        Item::LoadState => "F8",
        Item::ChooseCover => "F2",
        Item::CoverGrid => "F3",
        Item::AiPlayer => "F4",
        Item::Fullscreen => "F11",
        Item::ShowFps => "F10",
        _ => "",
    }
}

/// Geometry of the menu in the Library window: the tab cells in the
/// `strip` row (always shown), and the panel of tab `tab`, dropped below
/// it: the subtitle line, one rect per item row, and for the slot page one
/// cell per slot and the preview of the selected slot's state below them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLayout {
    pub tabs: Vec<Rect>,
    pub panel: Rect,
    pub subtitle: Rect,
    pub rows: Vec<Rect>,
    pub slots: Vec<Rect>,
    pub preview: Rect,
    pub text_scale: u32,
}

/// Panel width in characters: the longest labels plus their hints.
const PANEL_CHARS: u32 = 36;
/// Content height in item rows: room for the slot grid and its preview.
const CONTENT_ROWS: u32 = 11;
/// Tab width in characters.
const TAB_CHARS: u32 = 11;

pub fn menu_layout(layout: &Layout, strip: Rect, tab: usize) -> MenuLayout {
    let scale = layout.scale;
    let item_h = GLYPH * scale * 2;
    let pad = (GLYPH * scale) as i32;
    let tab_w = TAB_CHARS * GLYPH * scale;
    let tabs: Vec<Rect> = (0..TABS.len() as u32)
        .map(|i| Rect {
            x: strip.x + (i * tab_w) as i32,
            w: tab_w,
            ..strip
        })
        .collect();
    let content_w = PANEL_CHARS * GLYPH * scale;
    let content_h = CONTENT_ROWS * item_h;
    let panel_w = content_w + 2 * pad as u32;
    // Under the open tab, kept inside the window.
    let max_x = (layout.width as i32 - panel_w as i32).max(0);
    let panel = Rect {
        x: tabs[tab.min(TABS.len() - 1)].x.min(max_x),
        y: strip.y + strip.h as i32,
        w: panel_w,
        h: item_h + content_h + 3 * pad as u32,
    };
    let subtitle = Rect {
        x: panel.x + pad,
        y: panel.y + pad,
        w: content_w,
        h: item_h,
    };
    let top = subtitle.y + item_h as i32 + pad;
    let rows = (0..MAX_TAB_ITEMS as i32)
        .map(|i| Rect {
            x: panel.x + pad,
            y: top + i * item_h as i32,
            w: content_w,
            h: item_h,
        })
        .collect();
    // Slot page: two rows of cells, a title line, then the preview
    // (picture aspect) in the room left.
    let cell_w = content_w / u32::from(SLOT_COLUMNS);
    let cell_h = item_h * 2;
    let slots = (0..SLOTS)
        .map(|i| Rect {
            x: panel.x + pad + (u32::from(i % SLOT_COLUMNS) * cell_w) as i32,
            y: top + (u32::from(i / SLOT_COLUMNS) * cell_h) as i32,
            w: cell_w,
            h: cell_h,
        })
        .collect();
    let grid_h = cell_h * u32::from(SLOTS.div_ceil(SLOT_COLUMNS));
    let room = Rect {
        x: panel.x + pad,
        y: top + (grid_h + item_h) as i32,
        w: content_w,
        h: content_h.saturating_sub(grid_h + item_h),
    };
    MenuLayout {
        preview: crate::library::fit_picture(room, 320, 224),
        tabs,
        panel,
        subtitle,
        rows,
        slots,
        text_scale: scale,
    }
}

impl MenuLayout {
    /// Item row under window pixel (`x`, `y`).
    pub fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        self.rows.iter().position(|r| r.contains(x, y))
    }

    /// Tab under window pixel (`x`, `y`).
    pub fn tab_at(&self, x: i32, y: i32) -> Option<usize> {
        self.tabs.iter().position(|r| r.contains(x, y))
    }

    /// Slot cell under window pixel (`x`, `y`).
    pub fn slot_at(&self, x: i32, y: i32) -> Option<u8> {
        self.slots
            .iter()
            .position(|r| r.contains(x, y))
            .map(|i| i as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_game() -> MenuContext {
        MenuContext {
            game: Some("Streets of Rage".into()),
            selected_game: Some("QuackShot".into()),
            ..MenuContext::default()
        }
    }

    /// Opens the tab of `item` and selects it.
    fn pick(m: &mut Menu, item: Item, ctx: &MenuContext) {
        let tab = TABS
            .iter()
            .position(|&t| tab_items(t).contains(&item))
            .unwrap();
        m.open(ctx);
        if m.tab_index() != tab {
            m.click_tab(tab, ctx);
        }
        m.select_row(
            tab_items(TABS[tab])
                .iter()
                .position(|&i| i == item)
                .unwrap(),
            ctx,
        );
    }

    #[test]
    fn opens_on_the_game_or_library_tab() {
        let mut m = Menu::default();
        m.open(&with_game());
        assert_eq!(
            (m.tab(), m.selected_item()),
            (Section::Game, Some(Item::Resume))
        );
        m.open(&MenuContext::default());
        assert_eq!(
            (m.tab(), m.selected_item()),
            (Section::Library, Some(Item::RomFolder)),
            "no game"
        );
    }

    #[test]
    fn tabs_with_left_right_items_with_up_down() {
        let ctx = with_game();
        let mut m = Menu::default();
        m.open(&ctx);
        m.nav(Nav::Up, &ctx);
        assert_eq!(m.selected_item(), Some(Item::StopGame), "wraps in the tab");
        m.nav(Nav::Down, &ctx);
        assert_eq!(m.selected_item(), Some(Item::Resume));
        m.nav(Nav::Right, &ctx);
        assert_eq!(
            (m.tab(), m.selected_item()),
            (Section::Library, Some(Item::RomFolder))
        );
        m.nav(Nav::Right, &ctx);
        assert_eq!(
            (m.tab(), m.selected_item()),
            (Section::App, Some(Item::Fullscreen))
        );
        m.nav(Nav::Right, &ctx);
        assert_eq!(m.tab(), Section::Game, "wraps over the tabs");
        m.nav(Nav::Left, &ctx);
        assert_eq!(m.tab(), Section::App);
        // Without a game the Game tab is skipped; without a selection the
        // cover items are.
        let empty = MenuContext::default();
        m.open(&empty);
        m.nav(Nav::Down, &empty);
        assert_eq!(
            m.selected_item(),
            Some(Item::CoverGrid),
            "the greyed cover items skipped"
        );
        m.nav(Nav::Left, &empty);
        assert_eq!(m.tab(), Section::App, "Game tab skipped");
        m.nav(Nav::Right, &empty);
        assert_eq!(m.tab(), Section::Library);
        assert_eq!(m.click_tab(0, &empty), Command::None);
        assert_eq!(m.tab(), Section::Library, "Game tab not clickable");
        m.select_row(1, &empty);
        assert_eq!(
            m.selected_item(),
            Some(Item::RomFolder),
            "greyed rows not hoverable"
        );
        assert_eq!(m.nav(Nav::Back, &empty), Command::Resume);
        assert!(!m.open);
    }

    #[test]
    fn labels_follow_the_context() {
        let mut ctx = with_game();
        ctx.fps = true;
        let mut m = Menu::default();
        m.open(&ctx);
        let tabs = m.tabs(&ctx);
        assert_eq!(
            tabs.iter().map(|t| t.label).collect::<Vec<_>>(),
            ["Game", "Library", "App"]
        );
        assert!(tabs[0].selected && tabs.iter().all(|t| t.enabled));
        assert_eq!(m.subtitle(&ctx), "Streets of Rage");
        assert_eq!(m.rows(&ctx)[1].hint, "F5");
        m.nav(Nav::Right, &ctx);
        assert_eq!(m.subtitle(&ctx), "Selected: QuackShot");
        assert_eq!(m.rows(&ctx)[1].label, "Cover of \"QuackShot\"...");
        m.nav(Nav::Right, &ctx);
        assert_eq!(m.rows(&ctx)[1].label, "Show FPS [x]");
        let empty = MenuContext::default();
        m.open(&empty);
        assert!(!m.tabs(&empty)[0].enabled, "Game tab greyed");
        assert_eq!(m.subtitle(&empty), "No game selected");
        assert_eq!(m.rows(&empty)[1].label, "Choose cover...");
        assert!(!m.rows(&empty)[1].enabled);
    }

    #[test]
    fn slot_page_saves_loads_and_goes_back() {
        let mut ctx = with_game();
        ctx.used_slots[3] = true;
        let mut m = Menu::default();
        pick(&mut m, Item::SaveState, &ctx);
        assert_eq!(m.nav(Nav::Activate, &ctx), Command::None);
        assert_eq!(m.page, Page::Slots(SlotAction::Save));
        m.nav(Nav::Down, &ctx); // 0 -> 5
        m.nav(Nav::Right, &ctx); // 6
        m.nav(Nav::Up, &ctx); // 1
        m.nav(Nav::Left, &ctx); // 0
        m.nav(Nav::Left, &ctx); // 9 (wraps)
        assert_eq!(
            (m.slot, m.tab()),
            (9, Section::Game),
            "arrows move in the grid"
        );
        m.nav(Nav::Back, &ctx);
        assert_eq!((m.page, m.open), (Page::Main, true), "back to the tab");
        m.nav(Nav::Activate, &ctx);
        assert_eq!(m.nav(Nav::Activate, &ctx), Command::Save(9));
        assert!(!m.open);
        pick(&mut m, Item::LoadState, &ctx);
        m.nav(Nav::Activate, &ctx);
        m.select_slot(2);
        assert_eq!(m.nav(Nav::Activate, &ctx), Command::None, "slot 2 is empty");
        m.select_slot(3);
        assert_eq!(m.activate(&ctx), Command::Load(3));
        assert_eq!(m.slot, 3, "the slot of F5 / F8 from now on");
    }

    #[test]
    fn activating_items() {
        let ctx = with_game();
        let mut m = Menu::default();
        let mut choose = |item: Item| {
            pick(&mut m, item, &ctx);
            (m.nav(Nav::Activate, &ctx), m.open)
        };
        assert_eq!(choose(Item::Resume), (Command::Resume, false));
        assert_eq!(choose(Item::Reset), (Command::Reset, false));
        assert_eq!(choose(Item::StopGame), (Command::StopGame, false));
        assert_eq!(choose(Item::RomFolder), (Command::ChooseFolder, false));
        assert_eq!(choose(Item::ChooseCover), (Command::ChooseCover, false));
        assert_eq!(choose(Item::ResetCover), (Command::ResetCover, false));
        assert_eq!(choose(Item::Fullscreen), (Command::Fullscreen, false));
        assert_eq!(
            choose(Item::ShowFps),
            (Command::ToggleFps, true),
            "check box stays open"
        );
        assert_eq!(choose(Item::CoverGrid), (Command::ToggleView, true));
        assert_eq!(choose(Item::Quit), (Command::Quit, false));
    }

    #[test]
    fn keys_and_buttons() {
        assert_eq!(nav_from_key("Return"), Some(Nav::Activate));
        assert_eq!(nav_from_key("Escape"), Some(Nav::Back));
        assert_eq!(nav_from_key("A"), None, "game keys are not menu keys");
        assert_eq!(nav_from_button("dpdown"), Some(Nav::Down));
        assert_eq!(nav_from_button("a"), Some(Nav::Activate));
        assert_eq!(nav_from_button("guide"), Some(Nav::Back));
    }

    #[test]
    fn clicking_tabs_opens_and_closes() {
        let ctx = with_game();
        let mut m = Menu::default();
        assert_eq!(m.click_tab(2, &ctx), Command::None);
        assert!(m.open && m.tab() == Section::App);
        assert_eq!(m.selected_item(), Some(Item::Fullscreen));
        m.click_tab(1, &ctx);
        assert_eq!(m.tab(), Section::Library, "another tab: switches");
        assert_eq!(m.click_tab(1, &ctx), Command::Resume, "same tab: closes");
        assert!(!m.open);
        let empty = MenuContext::default();
        assert_eq!(m.click_tab(0, &empty), Command::None);
        assert!(!m.open, "Game tab without a game");
    }

    #[test]
    fn layout_inside_the_library_window() {
        let layout = Layout::new(900, 640, true);
        let strip = crate::library::library_layout(&layout).tabs;
        let window = Rect {
            x: 0,
            y: layout.bar.h as i32,
            w: 900,
            h: 640 - layout.bar.h,
        };
        let inside = |outer: &Rect, r: &Rect| {
            r.x >= outer.x
                && r.y >= outer.y
                && r.x + r.w as i32 <= outer.x + outer.w as i32
                && r.y + r.h as i32 <= outer.y + outer.h as i32
        };
        for tab in 0..TABS.len() {
            let m = menu_layout(&layout, strip, tab);
            assert_eq!(
                (m.tabs.len(), m.rows.len(), m.slots.len()),
                (3, MAX_TAB_ITEMS, SLOTS as usize)
            );
            assert!(
                m.tabs.iter().all(|t| inside(&strip, t)),
                "tabs in their row"
            );
            assert!(inside(&window, &m.panel), "tab {tab}: {:?}", m.panel);
            assert_eq!(
                m.panel.y,
                strip.y + strip.h as i32,
                "dropped below the tabs"
            );
            assert!(
                m.rows
                    .iter()
                    .chain(&m.slots)
                    .chain([&m.subtitle, &m.preview])
                    .all(|r| inside(&m.panel, r))
            );
            assert!(m.preview.h > 50, "{:?}", m.preview);
            assert_eq!(m.tab_at(m.tabs[2].x + 1, m.tabs[2].y + 1), Some(2));
            let r = m.rows[2];
            assert_eq!(m.row_at(r.x + 1, r.y + 1), Some(2));
            let c = m.slots[7];
            assert_eq!(m.slot_at(c.x + 1, c.y + 1), Some(7));
        }
        assert_eq!(
            menu_layout(&layout, strip, 0).panel.x,
            strip.x,
            "under the Game tab"
        );
    }
}

//! Choosing a game's cover among libretro's box art names: the list, a
//! word filter and the selection (unit-tested without SDL; drawn by
//! `sdl::library`).

use std::path::PathBuf;
use std::sync::Arc;

pub struct CoverPicker {
    /// The game whose cover is chosen.
    pub rom: PathBuf,
    /// Its title, for the panel.
    pub title: String,
    names: Arc<Vec<String>>,
    filter: String,
    /// Indices into `names` matching the filter.
    visible: Vec<usize>,
    selected: usize,
    pub scroll: usize,
}

/// The title part of a file name ("Streets of Rage (World) (En,Ja)" gives
/// "Streets of Rage"): the starting filter.
pub fn title_of(name: &str) -> &str {
    name[..name.find(['(', '[']).unwrap_or(name.len())].trim()
}

impl CoverPicker {
    /// Opens for `rom` with the filter set to its title.
    pub fn new(rom: PathBuf, names: Arc<Vec<String>>) -> CoverPicker {
        let name = crate::boxart::libretro_name(&rom);
        let mut picker = CoverPicker {
            title: name.clone(),
            rom,
            names,
            filter: title_of(&name).to_string(),
            visible: Vec::new(),
            selected: 0,
            scroll: 0,
        };
        picker.refilter();
        picker
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.refilter();
    }

    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.refilter();
    }

    /// Every word of the filter appears in the name, case ignored.
    fn refilter(&mut self) {
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.visible = (0..self.names.len())
            .filter(|&i| {
                let name = self.names[i].to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            })
            .collect();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// The matching names, in order.
    pub fn visible(&self) -> impl Iterator<Item = &str> {
        self.visible.iter().map(|&i| self.names[i].as_str())
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.visible
            .get(self.selected)
            .map(|&i| self.names[i].as_str())
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    pub fn select(&mut self, row: usize) {
        if row < self.visible.len() {
            self.selected = row;
        }
    }

    pub fn home(&mut self) {
        self.selected = 0;
    }

    pub fn end(&mut self) {
        self.selected = self.visible.len().saturating_sub(1);
    }

    /// Scrolls so that the selection is among the `rows` shown.
    pub fn keep_selection_visible(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Arc<Vec<String>> {
        Arc::new(
            [
                "Bare Knuckle - Ikari no Tekken ~ Streets of Rage (World)",
                "Sonic The Hedgehog (USA, Europe)",
                "Streets of Rage (USA)",
                "Streets of Rage (World) (Rev A)",
                "Streets of Rage 2 (Europe)",
            ]
            .map(String::from)
            .to_vec(),
        )
    }

    #[test]
    fn opens_on_the_game_title_and_filters_by_words() {
        let mut p = CoverPicker::new(
            PathBuf::from("/r/Streets of Rage (World) (En,Ja).zip"),
            names(),
        );
        assert_eq!(p.filter(), "Streets of Rage");
        assert_eq!(p.visible_len(), 4, "every name holding the three words");
        assert_eq!(
            p.selected_name(),
            Some("Bare Knuckle - Ikari no Tekken ~ Streets of Rage (World)")
        );
        for _ in 0.."Streets of Rage".len() {
            p.pop_filter();
        }
        "rage usa".chars().for_each(|c| p.push_filter(c));
        assert_eq!(
            p.visible().collect::<Vec<_>>(),
            ["Streets of Rage (USA)"],
            "words in any order"
        );
        "zzz".chars().for_each(|c| p.push_filter(c));
        assert_eq!(p.selected_name(), None);
        p.move_by(1); // nothing to move through
    }

    #[test]
    fn navigation_and_scrolling() {
        let mut p = CoverPicker::new(PathBuf::from("Streets.bin"), names());
        p.pop_filter();
        for _ in 0..10 {
            p.pop_filter();
        }
        assert_eq!(p.visible_len(), 5);
        p.move_by(3);
        p.keep_selection_visible(2);
        assert_eq!((p.selected_index(), p.scroll), (3, 2));
        p.end();
        assert_eq!(p.selected_name(), Some("Streets of Rage 2 (Europe)"));
        p.move_by(10);
        assert_eq!(p.selected_index(), 4, "clamped");
        p.home();
        p.keep_selection_visible(2);
        assert_eq!(p.scroll, 0);
        p.select(2);
        assert_eq!(p.selected_name(), Some("Streets of Rage (USA)"));
        assert_eq!(title_of("Game (USA) [!]"), "Game");
    }
}

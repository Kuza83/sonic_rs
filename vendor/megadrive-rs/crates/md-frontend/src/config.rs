//! Settings remembered between runs: `~/.config/megadrive-rs/config`
//! (`$XDG_CONFIG_HOME` honoured), plain `key = value` lines.

use std::path::{Path, PathBuf};

const LIBRARY_KEY: &str = "library";
const GAME_WINDOW_KEY: &str = "game_window";
const LIBRARY_WINDOW_KEY: &str = "library_window";
const FULLSCREEN_WORD: &str = "fullscreen";
const LIBRARY_VIEW_KEY: &str = "library_view";

/// Where a window was when the application closed: its windowed position
/// and size, and whether it was fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowState {
    pub area: crate::chrome::Rect,
    pub fullscreen: bool,
}

impl WindowState {
    /// "x y w h" then "fullscreen" when it was.
    fn parse(text: &str) -> Option<WindowState> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let [x, y, w, h, rest @ ..] = &words[..] else {
            return None;
        };
        let area = crate::chrome::Rect {
            x: x.parse().ok()?,
            y: y.parse().ok()?,
            w: w.parse().ok().filter(|&w: &u32| w > 0)?,
            h: h.parse().ok().filter(|&h: &u32| h > 0)?,
        };
        Some(WindowState {
            area,
            fullscreen: rest.first() == Some(&FULLSCREEN_WORD),
        })
    }

    fn text(&self) -> String {
        let a = self.area;
        let mut text = format!("{} {} {} {}", a.x, a.y, a.w, a.h);
        if self.fullscreen {
            text.push(' ');
            text.push_str(FULLSCREEN_WORD);
        }
        text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Folder of the Library window.
    pub library: Option<PathBuf>,
    /// The windows as they were at the last close.
    pub game_window: Option<WindowState>,
    pub library_window: Option<WindowState>,
    /// List or cover grid.
    pub library_view: crate::library::ViewMode,
}

/// Where the configuration file lives, if a home folder is known.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("megadrive-rs").join("config"))
}

impl Config {
    /// Reads `path`; a missing or unreadable file gives the defaults, and
    /// unknown lines are ignored (a newer version may have written them).
    pub fn load_from(path: &Path) -> Config {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Config::default();
        };
        let mut config = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                LIBRARY_KEY if !value.is_empty() => config.library = Some(PathBuf::from(value)),
                GAME_WINDOW_KEY => config.game_window = WindowState::parse(value),
                LIBRARY_WINDOW_KEY => config.library_window = WindowState::parse(value),
                LIBRARY_VIEW_KEY if value == "covers" => {
                    config.library_view = crate::library::ViewMode::Covers;
                }
                _ => {}
            }
        }
        config
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = String::from("# megadrive-rs settings\n");
        if let Some(library) = &self.library {
            text.push_str(&format!("{LIBRARY_KEY} = {}\n", library.display()));
        }
        if self.library_view == crate::library::ViewMode::Covers {
            text.push_str(&format!("{LIBRARY_VIEW_KEY} = covers\n"));
        }
        for (key, state) in [
            (GAME_WINDOW_KEY, &self.game_window),
            (LIBRARY_WINDOW_KEY, &self.library_window),
        ] {
            if let Some(state) = state {
                text.push_str(&format!("{key} = {}\n", state.text()));
            }
        }
        std::fs::write(path, text)
    }

    pub fn load() -> Config {
        config_path().map_or_else(Config::default, |p| Config::load_from(&p))
    }

    /// Saves to `config_path()`; failures are reported, not fatal.
    pub fn save(&self) {
        if let Some(path) = config_path()
            && let Err(e) = self.save_to(&path)
        {
            eprintln!("cannot save {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_missing_file_and_unknown_lines() {
        let dir = std::env::temp_dir().join(format!("md-rs-config-{}", std::process::id()));
        let path = dir.join("sub/config");
        assert_eq!(Config::load_from(&path), Config::default(), "missing file");
        let config = Config {
            library: Some(PathBuf::from("/home/me/roms")),
            game_window: Some(WindowState {
                area: crate::chrome::Rect {
                    x: 1920,
                    y: 30,
                    w: 960,
                    h: 720,
                },
                fullscreen: true,
            }),
            library_window: Some(WindowState {
                area: crate::chrome::Rect {
                    x: -10,
                    y: 40,
                    w: 900,
                    h: 640,
                },
                fullscreen: false,
            }),
            library_view: crate::library::ViewMode::Covers,
        };
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), config);
        std::fs::write(
            &path,
            "# comment\ncolour = blue\n  library =  /data/md roms \n",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&path).library,
            Some(PathBuf::from("/data/md roms")),
            "spaces trimmed, unknown keys ignored"
        );
        std::fs::write(&path, "game_window = 1 2 0 4\nlibrary_window = 1 2\n").unwrap();
        let broken = Config::load_from(&path);
        assert_eq!(
            (broken.game_window, broken.library_window),
            (None, None),
            "invalid sizes ignored"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

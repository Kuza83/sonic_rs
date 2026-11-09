//! Choosing the Library folder: GNOME's folder dialog (`zenity`), run in a
//! background thread so the game goes on while it is open. No dependency:
//! without zenity, a folder can still be dropped on the Library window or
//! given with --library.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};

/// The folder dialog's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Folder(PathBuf),
    Cancelled,
    /// zenity could not be run.
    Unavailable(String),
}

/// What zenity's exit status and output mean: the chosen folder on its
/// first line, or a cancellation (non-zero status, empty output).
pub fn parse_output(success: bool, stdout: &[u8]) -> Picked {
    let text = String::from_utf8_lossy(stdout);
    match text.lines().next().map(str::trim) {
        Some(line) if success && !line.is_empty() => Picked::Folder(PathBuf::from(line)),
        _ => Picked::Cancelled,
    }
}

/// Opens the folder dialog on `start`; the receiver gets the outcome once.
pub fn pick_folder(start: &Path) -> Receiver<Picked> {
    let (sender, receiver) = channel();
    // A trailing separator makes zenity open inside the folder.
    let filename = format!("{}/", start.display());
    std::thread::spawn(move || {
        let result = Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=megadrive-rs - ROM folder",
            ])
            .arg(format!("--filename={filename}"))
            .output();
        let picked = match result {
            Ok(out) => parse_output(out.status.success(), &out.stdout),
            Err(e) => Picked::Unavailable(e.to_string()),
        };
        let _ = sender.send(picked);
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zenity_output() {
        assert_eq!(
            parse_output(true, b"/home/me/roms\n"),
            Picked::Folder(PathBuf::from("/home/me/roms"))
        );
        assert_eq!(parse_output(false, b""), Picked::Cancelled, "dialog closed");
        assert_eq!(parse_output(true, b"\n"), Picked::Cancelled);
    }
}

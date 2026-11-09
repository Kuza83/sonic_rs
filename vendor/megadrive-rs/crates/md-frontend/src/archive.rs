//! ROM files given as a zip archive.
//!
//! A file starting with the zip local-header signature is opened as an
//! archive and its first entry with a ROM extension is used; any other file
//! is the ROM image itself. The core never sees the archive.

use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, bail};

/// Local file header signature (PKWARE APPNOTE.TXT, section 4.3.7).
const ZIP_SIGNATURE: &[u8; 4] = b"PK\x03\x04";
/// Extensions recognised as ROM images inside an archive.
const ROM_EXTENSIONS: [&str; 4] = ["bin", "md", "gen", "smd"];
/// Largest image accepted: 4 MiB cartridge space plus the Sega mapper's
/// 512 KiB banks give 16 MiB at most; beyond that the entry is rejected
/// instead of being inflated without bound.
const MAX_ROM_SIZE: u64 = 16 * 1024 * 1024;

/// The ROM image stored at `path`, unpacked if it is a zip archive.
pub fn read_rom(path: &Path) -> anyhow::Result<Vec<u8>> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    rom_image(data).with_context(|| format!("archive {}", path.display()))
}

/// `data` itself, or the ROM entry it holds if it is a zip archive.
pub fn rom_image(data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if !data.starts_with(ZIP_SIGNATURE) {
        return Ok(data);
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).context("invalid zip archive")?;
    let index = (0..zip.len())
        .find(|&i| zip.name_for_index(i).is_some_and(is_rom_name))
        .with_context(|| format!("no ROM ({}) in the archive", ROM_EXTENSIONS.join(", ")))?;
    let mut entry = zip.by_index(index)?;
    if entry.size() > MAX_ROM_SIZE {
        bail!(
            "{}: {} bytes, larger than any ROM",
            entry.name(),
            entry.size()
        );
    }
    let mut rom = Vec::with_capacity(entry.size() as usize);
    // The declared size is not trusted: reading stops past the limit too.
    (&mut entry)
        .take(MAX_ROM_SIZE + 1)
        .read_to_end(&mut rom)
        .with_context(|| format!("unpacking {}", entry.name()))?;
    if rom.len() as u64 > MAX_ROM_SIZE {
        bail!("{}: larger than any ROM", entry.name());
    }
    Ok(rom)
}

fn is_rom_name(name: &str) -> bool {
    !name.ends_with('/')
        && Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ROM_EXTENSIONS.iter().any(|r| e.eq_ignore_ascii_case(r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn plain_images_pass_through() {
        let rom = vec![0x12; 1024];
        assert_eq!(rom_image(rom.clone()).unwrap(), rom);
    }

    #[test]
    fn first_rom_entry_is_unpacked() {
        let rom: Vec<u8> = (0..4096u32).map(|i| (i * 7) as u8).collect();
        let archive = zip_of(&[
            ("readme.txt", b"not a rom"),
            ("dir/game.MD", &rom),
            ("other.bin", b"second rom"),
        ]);
        assert_eq!(rom_image(archive).unwrap(), rom);
    }

    #[test]
    fn archive_without_rom_is_rejected() {
        let archive = zip_of(&[("readme.txt", b"text"), ("bin/", b"")]);
        let err = rom_image(archive).unwrap_err();
        assert!(err.to_string().contains("no ROM"), "{err}");
    }

    #[test]
    fn corrupt_archive_is_rejected() {
        let mut archive = zip_of(&[("game.bin", &[0x55; 2048])]);
        archive.truncate(archive.len() / 2);
        assert!(rom_image(archive).is_err());
    }

    #[test]
    fn rom_names() {
        assert!(is_rom_name("Game (World).gen"));
        assert!(is_rom_name("a/b.SMD"));
        assert!(!is_rom_name("game.zip"));
        assert!(!is_rom_name("game.md/"));
        assert!(!is_rom_name("bin"));
    }
}

//! What a ROM's header says about its game, for the Library's info panel.
//! Field formats from Plutiedev, "ROM header" (copyright, serial number,
//! device support, region support, extra memory).

use md_core::Cartridge;

/// Cartridges above 4 MiB switch banks (SSF2 mapper, see md-core).
const BANKED_ROM_SIZE: usize = 0x40_0000;

/// One line of the panel: label and value.
pub type InfoLine = (&'static str, String);

/// The panel's lines for `cart`.
pub fn game_info(cart: &Cartridge) -> Vec<InfoLine> {
    let h = cart.header();
    let mut rom = format!(
        "{} KB, checksum {}",
        cart.rom().len() / 1024,
        if cart.checksum_ok() { "OK" } else { "mismatch" }
    );
    if cart.rom().len() > BANKED_ROM_SIZE {
        rom.push_str(", banked");
    }
    vec![
        ("Publisher", publisher(&h.copyright)),
        ("Serial", serial(&h.serial)),
        ("Regions", regions(cart.region_field())),
        ("Controls", devices(&h.device_support)),
        ("Save", save(&h.extra_memory)),
        ("ROM", rom),
    ]
}

/// "(C)XXXX YYYY.ZZZ": publisher code, year, month. Shown as
/// "SEGA, Apr 1991"; the raw text when it does not follow the format.
pub fn publisher(copyright: &str) -> String {
    let parsed = (|| {
        let rest = copyright.strip_prefix("(C)")?;
        let (code, date) = rest.rsplit_once(' ')?;
        let (year, month) = date.split_once('.').unwrap_or((date, ""));
        if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let code = code.trim();
        let month = month.trim();
        let mut text = if code.is_empty() {
            String::new()
        } else {
            format!("{code}, ")
        };
        if month.len() == 3 {
            text.push_str(&month[..1]);
            text.push_str(&month[1..].to_ascii_lowercase());
            text.push(' ');
        }
        text.push_str(year);
        Some(text)
    })();
    parsed.unwrap_or_else(|| or_unknown(copyright))
}

/// "XX YYYYYYYY-ZZ": software type, product code, revision.
pub fn serial(serial: &str) -> String {
    let parsed = (|| {
        let (kind, rest) = serial.split_at_checked(2)?;
        let kind = match kind {
            "GM" => "Game",
            "AI" => "Aid",
            "OS" | "BR" => "Boot ROM",
            _ => return None,
        };
        let (code, revision) = rest.trim().rsplit_once('-')?;
        Some(format!("{kind} {}, rev. {}", code.trim(), revision.trim()))
    })();
    parsed.unwrap_or_else(|| or_unknown(serial))
}

/// Region support: old style letters J, U, E (in any order, spaces
/// anywhere), or new style one hex digit (bit 0 domestic 60 Hz, bit 1
/// domestic 50 Hz, bit 2 overseas 60 Hz, bit 3 overseas 50 Hz; "E" read as
/// Europe, as Plutiedev advises).
pub fn regions(field: &[u8]) -> String {
    let chars: Vec<char> = field
        .iter()
        .map(|&b| b as char)
        .filter(|c| *c != ' ' && *c != '\0')
        .collect();
    let mut names = Vec::new();
    if !chars.is_empty() && chars.iter().all(|c| matches!(c, 'J' | 'U' | 'E')) {
        for (letter, name) in [('J', "Japan"), ('U', "Americas"), ('E', "Europe")] {
            if chars.contains(&letter) {
                names.push(name);
            }
        }
    } else if let [digit] = chars[..]
        && let Some(bits) = digit.to_digit(16)
    {
        let bits = if digit == 'E' { 8 } else { bits };
        for (bit, name) in [
            (1, "Japan"),
            (2, "Domestic 50 Hz"),
            (4, "Americas"),
            (8, "Europe"),
        ] {
            if bits & bit != 0 {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        or_unknown(&chars.iter().collect::<String>())
    } else {
        names.join(", ")
    }
}

/// Device support letters, one per device.
pub fn devices(field: &str) -> String {
    let names: Vec<&str> = field
        .chars()
        .filter_map(|c| {
            Some(match c {
                'J' => "3-button pad",
                '6' => "6-button pad",
                '0' => "Master System pad",
                'A' => "analog stick",
                '4' => "multitap",
                'G' => "light gun",
                'L' => "Activator",
                'M' => "mouse",
                'B' => "trackball",
                'T' => "tablet",
                'V' => "paddle",
                'K' => "keyboard",
                'R' => "RS-232",
                'P' => "printer",
                'C' => "CD-ROM",
                'F' => "floppy drive",
                _ => return None,
            })
        })
        .collect();
    if names.is_empty() {
        or_unknown(field)
    } else {
        names.join(", ")
    }
}

/// Extra memory: "RA", a type byte, $20 (or $40 for EEPROM), then the
/// start and end addresses.
pub fn save(extra: &[u8; 12]) -> String {
    if &extra[..2] != b"RA" {
        return "none".into();
    }
    let start = u32::from_be_bytes(extra[4..8].try_into().unwrap());
    let end = u32::from_be_bytes(extra[8..12].try_into().unwrap());
    let span = end.saturating_sub(start) as usize + 1;
    let (kind, bytes) = match extra[2] {
        0xE0 => ("battery", span),
        0xF0 | 0xF8 => ("battery", span.div_ceil(2)),
        0xE8 => ("EEPROM", 0),
        0xA0 => ("RAM, not saved", span),
        0xB0 | 0xB8 => ("RAM, not saved", span.div_ceil(2)),
        _ => ("declared", 0),
    };
    match bytes {
        0 => kind.to_string(),
        b if b >= 1024 => format!("{kind}, {} KB", b / 1024),
        b => format!("{kind}, {b} bytes"),
    }
}

fn or_unknown(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        "-".into()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copyright_and_serial() {
        assert_eq!(publisher("(C)SEGA 1991.APR"), "SEGA, Apr 1991");
        assert_eq!(publisher("(C)T-48 1992.JUL"), "T-48, Jul 1992");
        assert_eq!(publisher("(C)SEGA 1993"), "SEGA, 1993");
        assert_eq!(publisher("homebrew"), "homebrew", "raw when not the format");
        assert_eq!(publisher(""), "-");
        assert_eq!(serial("GM 00001009-00"), "Game 00001009, rev. 00");
        assert_eq!(serial("AI 12345678-01"), "Aid 12345678, rev. 01");
        assert_eq!(serial("XYZ"), "XYZ");
    }

    #[test]
    fn region_styles() {
        assert_eq!(regions(b"JUE"), "Japan, Americas, Europe");
        assert_eq!(regions(b" U "), "Americas", "spaces anywhere");
        assert_eq!(regions(b"E  "), "Europe");
        assert_eq!(regions(b"4  "), "Americas", "new style bit 2");
        assert_eq!(regions(b"F  "), "Japan, Domestic 50 Hz, Americas, Europe");
        assert_eq!(regions(b"   "), "-");
    }

    #[test]
    fn devices_and_save() {
        assert_eq!(devices("J6"), "3-button pad, 6-button pad");
        assert_eq!(devices("J  M"), "3-button pad, mouse");
        assert_eq!(devices(""), "-");
        let mut extra = *b"RA\xF8\x20\x00\x20\x00\x01\x00\x20\x3F\xFF";
        assert_eq!(save(&extra), "battery, 8 KB", "odd bytes $200001-$203FFF");
        extra[2] = 0xE8;
        extra[3] = 0x40;
        assert_eq!(save(&extra), "EEPROM");
        assert_eq!(save(&[b' '; 12]), "none");
    }

    #[test]
    fn info_of_a_generated_rom() {
        let cart = Cartridge::from_bytes(&md_core::testrom::showcase()).unwrap();
        let info = game_info(&cart);
        let labels: Vec<&str> = info.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            ["Publisher", "Serial", "Regions", "Controls", "Save", "ROM"]
        );
        assert!(info[5].1.contains("KB, checksum"), "{}", info[5].1);
    }
}

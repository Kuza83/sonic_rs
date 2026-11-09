//! Cheat codes: Game Genie (`ABCD-EFGH`) and raw / Pro Action Replay
//! (`AAAAAA:VVVV`) codes, both meaning "address: 16-bit value".
//!
//! Game Genie decoding: alphabet `ABCDEFGHJKLMNPRSTVWXYZ0123456789` (5 bits
//! per character, no I/O/Q/U), bits scrambled as documented in DGen's
//! decode.c (example SCRA-MBLE = 059C44:B078) and implemented identically in
//! Genesis Plus GX (libretro.c decode_cheat).
//!
//! Effect (GPGX apply_cheats / RAMCheatUpdate):
//! - address ≥ $FF0000: work RAM forced at the start of every frame, as a
//!   word (at address & ~1) when the value's high byte is non-zero, else as
//!   a byte;
//! - address inside the ROM image: the ROM word at address & ~1 reads as the
//!   value (the image itself is not modified);
//! - anything else is rejected.

use std::fmt;

pub const GAME_GENIE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPRSTVWXYZ0123456789";
const GAME_GENIE_LEN: usize = 9; // 4 characters, '-', 4 characters
const RAW_LEN: usize = 11; // 6 hex digits, ':', 4 hex digits
/// Codes at or above this address patch work RAM.
pub const RAM_CHEAT_START: u32 = 0xFF_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cheat {
    /// 24-bit 68000 address.
    pub address: u32,
    pub value: u16,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CheatError {
    #[error("'{0}' is neither a Game Genie code (ABCD-EFGH) nor a raw code (AAAAAA:VVVV)")]
    Format(String),
    #[error("invalid character '{1}' in '{0}'")]
    Character(String, char),
    #[error("{0}: address is neither in the ROM nor in work RAM")]
    Target(Cheat),
}

impl fmt::Display for Cheat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06X}:{:04X}", self.address, self.value)
    }
}

impl Cheat {
    pub fn is_ram(&self) -> bool {
        self.address >= RAM_CHEAT_START
    }
}

/// Parses a Game Genie or raw code (case-insensitive, surrounding spaces
/// ignored).
pub fn parse(code: &str) -> Result<Cheat, CheatError> {
    let text = code.trim().to_ascii_uppercase();
    let bytes = text.as_bytes();
    match (bytes.len(), bytes.get(4), bytes.get(6)) {
        (GAME_GENIE_LEN, Some(b'-'), _) => decode_game_genie(&text),
        (RAW_LEN, _, Some(b':')) => {
            // Digits only: from_str_radix would also take a leading '+'.
            let hex = |s: &str| match s.chars().find(|c| !c.is_ascii_hexdigit()) {
                Some(bad) => Err(CheatError::Character(code.to_string(), bad)),
                None => Ok(u32::from_str_radix(s, 16).expect("hex digits")),
            };
            Ok(Cheat {
                address: hex(&text[..6])?,
                value: hex(&text[7..])? as u16,
            })
        }
        _ => Err(CheatError::Format(code.to_string())),
    }
}

fn decode_game_genie(text: &str) -> Result<Cheat, CheatError> {
    let mut n = [0u32; 8];
    for (i, c) in text.chars().filter(|&c| c != '-').enumerate() {
        n[i] = GAME_GENIE_ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or_else(|| CheatError::Character(text.to_string(), c))? as u32;
    }
    let address = ((n[1] & 3) << 14)
        | (n[2] << 9)
        | ((n[3] & 0xF) << 20)
        | ((n[3] >> 4) << 8)
        | ((n[4] >> 1) << 16)
        | ((n[6] & 7) << 5)
        | n[7];
    let value = (n[0] << 3)
        | (n[1] >> 2)
        | ((n[4] & 1) << 12)
        | ((n[5] & 1) << 15)
        | ((n[5] >> 1) << 8)
        | ((n[6] >> 3) << 13);
    Ok(Cheat {
        address,
        value: value as u16,
    })
}

/// Game Genie spelling of a cheat (inverse of the decoding).
pub fn encode_game_genie(cheat: Cheat) -> String {
    let (a, d) = (cheat.address, cheat.value as u32);
    let n = [
        (d >> 3) & 0x1F,
        ((d & 7) << 2) | ((a >> 14) & 3),
        (a >> 9) & 0x1F,
        ((a >> 20) & 0xF) | (((a >> 8) & 1) << 4),
        ((d >> 12) & 1) | (((a >> 16) & 0xF) << 1),
        ((d >> 15) & 1) | (((d >> 8) & 0xF) << 1),
        ((a >> 5) & 7) | (((d >> 13) & 3) << 3),
        a & 0x1F,
    ];
    let mut out = String::with_capacity(GAME_GENIE_LEN);
    for (i, v) in n.into_iter().enumerate() {
        if i == 4 {
            out.push('-');
        }
        out.push(GAME_GENIE_ALPHABET[v as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_example() {
        // DGen decode.c: "SCRA-MBLE ... which in Hexadecimal is 059C44:B078".
        let cheat = parse("SCRA-MBLE").unwrap();
        assert_eq!(
            cheat,
            Cheat {
                address: 0x05_9C44,
                value: 0xB078
            }
        );
        assert_eq!(encode_game_genie(cheat), "SCRA-MBLE");
        assert_eq!(parse(" scra-mble ").unwrap(), cheat, "case and spaces");
    }

    #[test]
    fn every_bit_round_trips() {
        // One bit at a time through address and value, then mixed patterns.
        let mut cheats: Vec<Cheat> = (0..24)
            .map(|b| Cheat {
                address: 1 << b,
                value: 0,
            })
            .chain((0..16).map(|b| Cheat {
                address: 0,
                value: 1 << b,
            }))
            .collect();
        let mut x: u64 = 0x1234_5678_9ABC_DEF0;
        for _ in 0..1000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            cheats.push(Cheat {
                address: (x as u32) & 0xFF_FFFF,
                value: (x >> 32) as u16,
            });
        }
        for cheat in cheats {
            let code = encode_game_genie(cheat);
            assert_eq!(parse(&code).unwrap(), cheat, "{code}");
        }
    }

    #[test]
    fn raw_codes() {
        assert_eq!(
            parse("FF0123:00ab").unwrap(),
            Cheat {
                address: 0xFF_0123,
                value: 0x00AB
            }
        );
        assert!(parse("FF0123:00AB").unwrap().is_ram());
        assert!(!parse("00020A:5400").unwrap().is_ram());
        assert_eq!(parse("00020A:5400").unwrap().to_string(), "00020A:5400");
    }

    #[test]
    fn invalid_codes() {
        assert_eq!(
            parse("SCRA-MBLU"),
            Err(CheatError::Character("SCRA-MBLU".into(), 'U'))
        );
        assert!(matches!(parse("SCRAMBLE"), Err(CheatError::Format(_))));
        assert!(matches!(parse("SCRA-MBLE1"), Err(CheatError::Format(_))));
        assert!(matches!(
            parse("FF01G3:0000"),
            Err(CheatError::Character(_, 'G'))
        ));
        assert!(matches!(parse(""), Err(CheatError::Format(_))));
        assert!(matches!(
            parse("+F0123:0000"),
            Err(CheatError::Character(_, '+'))
        ));
    }
}

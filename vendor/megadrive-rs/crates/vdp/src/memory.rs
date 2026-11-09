//! VDP memories and access codes.

pub const VRAM_SIZE: usize = 0x1_0000;
pub const CRAM_ENTRIES: usize = 64;
/// 40 entries exist; the address decoder wraps on 64.
pub const VSRAM_ENTRIES: usize = 64;

/// CRAM keeps 9 bits per colour, in the bus layout ----BBB-GGG-RRR-.
pub const CRAM_MASK: u16 = 0x0EEE;

/// Target of the current access, from code bits CD3-CD0 (Plutiedev "VDP commands").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    VramWrite,
    CramWrite,
    VsramWrite,
    VramRead,
    CramRead,
    VsramRead,
    /// Any other code: accesses are ignored.
    Invalid,
}

impl Target {
    pub fn from_code(code: u8) -> Target {
        match code & 0x0F {
            0x1 => Target::VramWrite,
            0x3 => Target::CramWrite,
            0x5 => Target::VsramWrite,
            0x0 => Target::VramRead,
            0x8 => Target::CramRead,
            0x4 => Target::VsramRead,
            _ => Target::Invalid,
        }
    }
}

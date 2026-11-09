//! Debugging helpers: static disassembly of cartridge code.

use crate::cartridge::Cartridge;

/// One disassembled line: address, raw words and Motorola-syntax text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingLine {
    pub addr: u32,
    pub words: Vec<u16>,
    pub text: String,
}

/// Reads a big-endian word from the ROM image; outside the dump reads 0, as
/// the bus does (see docs/decisions.md, J2).
fn rom_word(rom: &[u8], addr: u32) -> u16 {
    let byte = |a: u32| rom.get(a as usize).copied().unwrap_or(0);
    u16::from_be_bytes([byte(addr), byte(addr.wrapping_add(1))])
}

/// Disassembles `count` instructions of the cartridge ROM starting at `from`.
/// Reading the ROM image directly keeps disassembly free of bus side effects.
pub fn disassemble_rom(cart: &Cartridge, from: u32, count: usize) -> Vec<ListingLine> {
    let rom = cart.rom();
    let mut addr = from;
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        let d = m68k::disassemble(addr, |a| rom_word(rom, a));
        let words = (0..d.len / 2)
            .map(|i| rom_word(rom, addr + 2 * i))
            .collect();
        lines.push(ListingLine {
            addr,
            words,
            text: d.text,
        });
        addr = addr.wrapping_add(d.len);
    }
    lines
}

impl std::fmt::Display for ListingLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let words: Vec<String> = self.words.iter().map(|w| format!("{w:04X}")).collect();
        write!(
            f,
            "{:06X}  {:<20}  {}",
            self.addr,
            words.join(" "),
            self.text
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_rom;

    #[test]
    fn lists_reset_code() {
        let mut rom = test_rom::build(0x400);
        let program: [u16; 6] = [0x46FC, 0x2700, 0x4EB9, 0x0000, 0x0300, 0x60FE];
        for (i, w) in program.iter().enumerate() {
            rom[0x200 + 2 * i..0x202 + 2 * i].copy_from_slice(&w.to_be_bytes());
        }
        let cart = Cartridge::from_bytes(&rom).unwrap();
        let lines = disassemble_rom(&cart, 0x200, 3);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            text,
            ["move #$2700,sr", "jsr ($00000300).l", "bra.s $00020A"]
        );
        assert_eq!(lines[1].words, [0x4EB9, 0x0000, 0x0300]);
        assert_eq!(
            lines[0].to_string(),
            format!("000200  {:<20}  move #$2700,sr", "46FC 2700")
        );
    }
}

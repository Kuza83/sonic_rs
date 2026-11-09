//! Sega ROM bank mapper (Super Street Fighter II board).
//!
//! The cartridge area is split into eight 512 KiB windows. Writing page `n`
//! to $A130F3 + 2 × (w − 1) (odd bytes $A130F3-$A130FF) maps ROM page `n`
//! into window `w` = 1-7; window 0 is fixed (ssf2.txt by Bart Trzynadlowski,
//! as implemented in Genesis Plus GX `mapper_ssf2_w` / `mapper_512k_w`; the
//! page number is masked by the ROM size rounded up to a power of two).
//! Every window starts on its own page, so cartridges that never write the
//! registers (all ROMs up to 4 MiB) read linearly as before.

pub const WINDOWS: usize = 8;
pub const WINDOW_SHIFT: u32 = 19;
const WINDOW_MASK: u32 = (1 << WINDOW_SHIFT) - 1;
/// $A130F3-$A130FF: address bits 3-1 select the window.
const REGISTER_WINDOW_SHIFT: u32 = 1;
const REGISTER_WINDOW_MASK: u32 = 0x7;

#[derive(Debug, Clone)]
pub struct Mapper {
    /// ROM page in each window.
    pages: [u8; WINDOWS],
    /// ROM length rounded up to a power of two, minus one (masks written pages).
    rom_mask: usize,
}

impl Mapper {
    pub fn new(rom_len: usize) -> Self {
        Self {
            pages: std::array::from_fn(|w| w as u8),
            rom_mask: rom_len.next_power_of_two() - 1,
        }
    }

    /// ROM offset seen at cartridge address `addr` ($000000-$3FFFFF).
    pub fn rom_offset(&self, addr: u32) -> usize {
        let page = self.pages[(addr >> WINDOW_SHIFT) as usize & (WINDOWS - 1)] as usize;
        (page << WINDOW_SHIFT) | (addr & WINDOW_MASK) as usize
    }

    /// Byte written to $A130F3-$A130FF (odd addresses).
    pub fn write(&mut self, addr: u32, page: u8) {
        let window = ((addr >> REGISTER_WINDOW_SHIFT) & REGISTER_WINDOW_MASK) as usize;
        if window != 0 {
            // Only written pages wrap on the ROM size: unwritten windows stay
            // linear (reads past the dump return 0 as without a mapper).
            self.pages[window] =
                (((page as usize) << WINDOW_SHIFT & self.rom_mask) >> WINDOW_SHIFT) as u8;
        }
    }

    pub fn pages(&self) -> &[u8; WINDOWS] {
        &self.pages
    }

    pub fn set_pages(&mut self, pages: [u8; WINDOWS]) {
        self.pages = pages;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_until_written() {
        let m = Mapper::new(0x40_0000);
        for addr in [0, 0x7_FFFF, 0x8_0000, 0x3F_FFFF] {
            assert_eq!(m.rom_offset(addr), addr as usize);
        }
    }

    #[test]
    fn windows_1_to_7_remap_window_0_is_fixed() {
        let mut m = Mapper::new(0x50_0000); // 5 MiB → mask 8 MiB
        m.write(0xA1_30FF, 9); // window 7 ← page 9
        assert_eq!(m.rom_offset(0x38_0010), 9 * 0x8_0000 + 0x10);
        m.write(0xA1_30F3, 8); // window 1 ← page 8
        assert_eq!(m.rom_offset(0x08_0000), 8 * 0x8_0000);
        m.write(0xA1_30F1, 5); // $A130F1 is the SRAM register, not window 0
        assert_eq!(m.rom_offset(0x1234), 0x1234);
    }

    #[test]
    fn unwritten_windows_do_not_mirror() {
        let m = Mapper::new(0x10_0000);
        assert_eq!(
            m.rom_offset(0x20_0000),
            0x20_0000,
            "past the dump, not mirrored"
        );
    }

    #[test]
    fn page_wraps_on_rom_size() {
        let mut m = Mapper::new(0x10_0000); // 1 MiB: pages 0-1
        m.write(0xA1_30F5, 3); // window 2 ← page 3 → page 1
        assert_eq!(m.rom_offset(0x10_0004), 0x8_0004);
    }
}

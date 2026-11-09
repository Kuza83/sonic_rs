//! System bus as seen by the 68000.

/// The 68000 drives 24 address lines (A23..A1 plus UDS/LDS for byte select),
/// so the upper byte of a 32-bit address is ignored.
pub const ADDRESS_MASK: u32 = 0x00FF_FFFF;

/// 68000 view of the system bus.
///
/// - Addresses may carry garbage in bits 24..31: implementations must mask
///   them with [`ADDRESS_MASK`].
/// - Data is big-endian.
/// - Alignment is the CPU's responsibility: `read16`/`write16` are only called
///   with even addresses (an odd word access raises an address error in the
///   CPU before reaching the bus).
/// - Reads take `&mut self` because some devices have read side effects
///   (e.g. reading the VDP status clears its "write pending" latch).
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn write8(&mut self, addr: u32, value: u8);
    fn write16(&mut self, addr: u32, value: u16);

    /// Long read as two word bus cycles, high word first. Instructions that
    /// need a different order (e.g. `MOVE.L -(An)` writes the low word first)
    /// issue the word accesses themselves.
    fn read32(&mut self, addr: u32) -> u32 {
        let hi = self.read16(addr) as u32;
        let lo = self.read16(addr.wrapping_add(2)) as u32;
        (hi << 16) | lo
    }

    /// Interrupt acknowledge cycle for `level` (1..=7). Returns a vector
    /// number, or `None` to request an autovector (24 + level) — what the
    /// Mega Drive does for every interrupt (M68000 UM §6.3.2).
    fn interrupt_ack(&mut self, level: u8) -> Option<u8> {
        let _ = level;
        None
    }

    /// Long write as two word bus cycles, high word first.
    fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }
}

/// Flat 16 MiB RAM covering the whole 24-bit address space.
/// Used for unit tests and SingleStepTests, where no memory map applies.
pub struct FlatBus {
    mem: Vec<u8>,
}

impl FlatBus {
    pub fn new() -> Self {
        Self {
            mem: vec![0; ADDRESS_MASK as usize + 1],
        }
    }
}

impl Default for FlatBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for FlatBus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.mem[(addr & ADDRESS_MASK) as usize]
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let a = (addr & ADDRESS_MASK) as usize;
        u16::from_be_bytes([self.mem[a], self.mem[(a + 1) & ADDRESS_MASK as usize]])
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.mem[(addr & ADDRESS_MASK) as usize] = value;
    }

    fn write16(&mut self, addr: u32, value: u16) {
        let a = (addr & ADDRESS_MASK) as usize;
        let [hi, lo] = value.to_be_bytes();
        self.mem[a] = hi;
        self.mem[(a + 1) & ADDRESS_MASK as usize] = lo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_access_is_big_endian() {
        let mut bus = FlatBus::new();
        bus.write16(0x1000, 0x1234);
        assert_eq!(bus.read8(0x1000), 0x12);
        assert_eq!(bus.read8(0x1001), 0x34);
        assert_eq!(bus.read16(0x1000), 0x1234);
    }

    #[test]
    fn long_access_is_big_endian_high_word_first() {
        let mut bus = FlatBus::new();
        bus.write32(0x2000, 0xDEAD_BEEF);
        assert_eq!(bus.read16(0x2000), 0xDEAD);
        assert_eq!(bus.read16(0x2002), 0xBEEF);
        assert_eq!(bus.read32(0x2000), 0xDEAD_BEEF);
    }

    #[test]
    fn upper_address_byte_is_ignored() {
        let mut bus = FlatBus::new();
        bus.write8(0xFF12_3456, 0xAB);
        assert_eq!(bus.read8(0x0012_3456), 0xAB);
    }

    #[test]
    fn long_access_wraps_at_top_of_address_space() {
        let mut bus = FlatBus::new();
        bus.write32(0x00FF_FFFE, 0x1122_3344);
        assert_eq!(bus.read16(0x00FF_FFFE), 0x1122);
        assert_eq!(bus.read16(0x0000_0000), 0x3344);
    }
}

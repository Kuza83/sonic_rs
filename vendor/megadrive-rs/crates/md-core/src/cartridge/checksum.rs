//! Header checksum: 16-bit wrapping sum of every big-endian word from $200
//! to the end of the ROM (Plutiedev, "ROM header", ROM checksum).

const CHECKSUM_START: usize = 0x200;

pub fn compute(rom: &[u8]) -> u16 {
    rom.get(CHECKSUM_START..)
        .unwrap_or(&[])
        .chunks(2)
        .map(|w| u16::from_be_bytes([w[0], *w.get(1).unwrap_or(&0)]))
        .fold(0u16, u16::wrapping_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_first_512_bytes() {
        let mut rom = vec![0xFFu8; 0x200];
        rom.extend_from_slice(&[0x00, 0x01, 0x00, 0x02]);
        assert_eq!(compute(&rom), 3);
    }

    #[test]
    fn sum_wraps_to_16_bits() {
        let mut rom = vec![0u8; 0x200];
        rom.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x02]);
        assert_eq!(compute(&rom), 0x0001);
    }

    #[test]
    fn rom_shorter_than_start_sums_to_zero() {
        assert_eq!(compute(&[0xAA; 0x100]), 0);
    }

    #[test]
    fn trailing_odd_byte_is_high_byte() {
        let mut rom = vec![0u8; 0x200];
        rom.push(0x12);
        assert_eq!(compute(&rom), 0x1200);
    }
}

//! ROM image formats: raw `.bin` and interleaved `.smd` (Super Magic Drive copier).
//!
//! SMD layout: a 512-byte copier header followed by 16 KiB blocks. In each block
//! the first 8 KiB hold the odd bytes and the last 8 KiB the even bytes
//! (Genesis Plus GX, core/loadrom.c, `deinterleave_block`).

use super::CartridgeError;

const SMD_HEADER_SIZE: usize = 0x200;
const SMD_BLOCK_SIZE: usize = 0x4000;
const SMD_HALF_BLOCK: usize = SMD_BLOCK_SIZE / 2;
/// Copier header signature bytes at offsets 8 and 9.
const SMD_SIGNATURE_OFFSET: usize = 8;
const SMD_SIGNATURE: [u8; 2] = [0xAA, 0xBB];
/// Linear images carry "SEGA" at the start of the system type field.
const SYSTEM_TYPE_OFFSET: usize = 0x100;
const SEGA_TAG: &[u8; 4] = b"SEGA";

/// Returns a linear (de-interleaved, header-less) copy of the image.
pub fn to_linear(data: &[u8]) -> Result<Vec<u8>, CartridgeError> {
    if !is_smd(data) {
        return Ok(data.to_vec());
    }
    let body = &data[SMD_HEADER_SIZE..];
    if body.is_empty() || !body.len().is_multiple_of(SMD_BLOCK_SIZE) {
        return Err(CartridgeError::InvalidSmdSize(data.len()));
    }
    let mut out = vec![0u8; body.len()];
    let (src_blocks, _) = body.as_chunks::<SMD_BLOCK_SIZE>();
    let (dst_blocks, _) = out.as_chunks_mut::<SMD_BLOCK_SIZE>();
    for (src, dst) in src_blocks.iter().zip(dst_blocks) {
        deinterleave_block(src, dst);
    }
    Ok(out)
}

/// An SMD image either carries the copier signature, or (Genesis Plus GX
/// heuristic) has an odd number of 512-byte blocks and no "SEGA" tag at $100.
fn is_smd(data: &[u8]) -> bool {
    let has_signature =
        data.get(SMD_SIGNATURE_OFFSET..SMD_SIGNATURE_OFFSET + 2) == Some(&SMD_SIGNATURE[..]);
    let odd_512_blocks = data.len().is_multiple_of(SMD_HEADER_SIZE)
        && !(data.len() / SMD_HEADER_SIZE).is_multiple_of(2);
    let has_sega_tag =
        data.get(SYSTEM_TYPE_OFFSET..SYSTEM_TYPE_OFFSET + SEGA_TAG.len()) == Some(&SEGA_TAG[..]);
    has_signature || (odd_512_blocks && !has_sega_tag)
}

fn deinterleave_block(src: &[u8; SMD_BLOCK_SIZE], dst: &mut [u8; SMD_BLOCK_SIZE]) {
    let (odd, even) = src.split_at(SMD_HALF_BLOCK);
    for (i, pair) in dst.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        pair[0] = even[i];
        pair[1] = odd[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_rom;

    /// Inverse of `deinterleave_block`, used to forge SMD images.
    fn interleave(linear: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; SMD_HEADER_SIZE];
        out[SMD_SIGNATURE_OFFSET..SMD_SIGNATURE_OFFSET + 2].copy_from_slice(&SMD_SIGNATURE);
        for block in linear.as_chunks::<SMD_BLOCK_SIZE>().0 {
            let odd: Vec<u8> = block.iter().skip(1).step_by(2).copied().collect();
            let even: Vec<u8> = block.iter().step_by(2).copied().collect();
            out.extend_from_slice(&odd);
            out.extend_from_slice(&even);
        }
        out
    }

    fn patterned_rom(size: usize) -> Vec<u8> {
        let mut rom = test_rom::build(size);
        for (i, b) in rom.iter_mut().enumerate().skip(0x200) {
            *b = (i * 7 + i / 251) as u8;
        }
        rom
    }

    #[test]
    fn raw_image_is_left_untouched() {
        let rom = patterned_rom(0x8000);
        assert_eq!(to_linear(&rom).unwrap(), rom);
    }

    #[test]
    fn smd_with_signature_round_trips() {
        let rom = patterned_rom(3 * SMD_BLOCK_SIZE);
        let smd = interleave(&rom);
        assert_eq!(smd.len(), SMD_HEADER_SIZE + rom.len());
        assert_eq!(to_linear(&smd).unwrap(), rom);
    }

    #[test]
    fn smd_without_signature_detected_by_heuristic() {
        let rom = patterned_rom(2 * SMD_BLOCK_SIZE);
        let mut smd = interleave(&rom);
        smd[SMD_SIGNATURE_OFFSET] = 0;
        smd[SMD_SIGNATURE_OFFSET + 1] = 0;
        assert_eq!(to_linear(&smd).unwrap(), rom);
    }

    #[test]
    fn smd_with_truncated_block_is_rejected() {
        let rom = patterned_rom(SMD_BLOCK_SIZE);
        let mut smd = interleave(&rom);
        smd.truncate(smd.len() - 0x200);
        assert!(matches!(
            to_linear(&smd),
            Err(CartridgeError::InvalidSmdSize(_))
        ));
    }
}

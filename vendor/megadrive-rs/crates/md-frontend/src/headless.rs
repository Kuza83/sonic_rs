//! Modes without a window: frame and audio dumps, ROM header.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::Context;
use md_core::{Cartridge, Console};

pub fn write_png(path: &Path, console: &Console) -> anyhow::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        console.screen_width() as u32,
        console.screen_height() as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(&console.frame_rgba())?;
    Ok(())
}

/// Minimal RIFF/WAVE writer: 16-bit PCM, stereo, `audio::OUTPUT_RATE` Hz.
pub fn write_wav(path: &Path, samples: &[i16]) -> anyhow::Result<()> {
    use std::io::Write;
    const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    let rate = md_core::audio::OUTPUT_RATE;
    let data_len = (samples.len() * 2) as u32;
    let block_align = CHANNELS * BITS / 8;
    let mut out =
        BufWriter::new(File::create(path).with_context(|| format!("creating {}", path.display()))?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_len).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&CHANNELS.to_le_bytes())?;
    out.write_all(&rate.to_le_bytes())?;
    out.write_all(&(rate * block_align as u32).to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&BITS.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        out.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

pub fn print_header(cart: &Cartridge) {
    let h = cart.header();
    println!("System   : {}", h.system_type);
    println!("Title    : {}", h.overseas_title);
    println!("Serial   : {}", h.serial);
    println!("Region   : {}", h.region);
    println!(
        "Reset    : SSP={:#010X} PC={:#010X}",
        h.initial_ssp, h.initial_pc
    );
    println!("ROM size : {:#X} bytes", cart.rom().len());
    if !cart.checksum_ok() {
        eprintln!(
            "warning: checksum mismatch (header {:#06X}, computed {:#06X})",
            h.checksum,
            cart.computed_checksum()
        );
    }
}

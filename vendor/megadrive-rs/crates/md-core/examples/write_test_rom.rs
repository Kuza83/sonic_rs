//! Writes a generated test ROM to disk.
//! Usage: cargo run -p md-core --example write_test_rom -- <name> <out.bin>
//! Names: checkerboard, checkerboard_dma, interrupts, pad_reader, showcase,
//! z80_loader, psg_tone.

use md_core::testrom;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "checkerboard".into());
    let path = args.next().unwrap_or_else(|| format!("{name}.bin"));
    let rom = match name.as_str() {
        "checkerboard" => testrom::checkerboard(),
        "checkerboard_dma" => testrom::checkerboard_dma(),
        "interrupts" => testrom::interrupts(),
        "pad_reader" => testrom::pad_reader(),
        "showcase" => testrom::showcase(),
        "z80_loader" => testrom::z80_loader(),
        "psg_tone" => testrom::psg_tone(),
        other => {
            eprintln!("unknown test ROM `{other}`");
            std::process::exit(2);
        }
    };
    std::fs::write(&path, rom)?;
    println!("wrote {path}");
    Ok(())
}

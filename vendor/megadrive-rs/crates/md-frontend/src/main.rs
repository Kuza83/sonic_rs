use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::Parser;
use md_core::debug::disassemble_rom;
use md_core::{Buttons, Cartridge, Console, MdConfig, PadKind, Region};
use script::InputScript;

#[cfg(any(feature = "sdl", test))]
mod agent;
mod archive;
#[cfg(feature = "sdl")]
mod battery;
mod headless;
// Pure logic of the window mode, unit-tested even without SDL.
#[cfg(any(feature = "sdl", test))]
mod boxart;
#[cfg(any(feature = "sdl", test))]
mod chrome;
#[cfg(any(feature = "sdl", test))]
mod config;
#[cfg(any(feature = "sdl", test))]
mod coverpick;
#[cfg(any(feature = "sdl", test))]
mod gameinfo;
#[cfg(any(feature = "sdl", test))]
mod keymap;
#[cfg(any(feature = "sdl", test))]
mod library;
#[cfg(any(feature = "sdl", test))]
mod menu;
#[cfg(any(feature = "sdl", test))]
mod pacing;
#[cfg(any(feature = "sdl", test))]
mod picker;
mod script;
#[cfg(feature = "sdl")]
mod sdl;

#[derive(Parser)]
#[command(about = "Sega Mega Drive / Genesis emulator")]
struct Args {
    /// ROM image (.bin, .md, .gen, .smd, or a .zip holding one); output path with
    /// --write-test-rom. Without it, the window opens on a welcome screen and a
    /// ROM can be dropped on it
    rom: Option<PathBuf>,

    /// Print the ROM header and exit
    #[arg(long)]
    info: bool,

    /// Write the generated homebrew test ROM NAME to the ROM path and exit
    #[arg(long, value_name = "NAME")]
    write_test_rom: Option<String>,

    /// Disassemble N instructions instead of printing the header
    #[arg(long, value_name = "N")]
    disasm: Option<usize>,

    /// Start address for --disasm, in hex (defaults to the reset PC)
    #[arg(long, value_name = "ADDR", value_parser = parse_hex, requires = "disasm")]
    from: Option<u32>,

    /// Run N frames, then save the last one (with --out) and/or print its hash
    #[arg(long, value_name = "N")]
    dump_frame: Option<u64>,

    /// PNG file written by --dump-frame or --screenshot-at
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,

    /// Print the FNV-1a hash of the frame dumped by --dump-frame or --screenshot-at
    #[arg(long)]
    hash: bool,

    /// Run N frames and write their audio to the --wav file
    #[arg(long, value_name = "N", requires = "wav")]
    dump_audio: Option<u64>,

    /// WAV file written by --dump-audio (16-bit stereo PCM)
    #[arg(long, value_name = "PATH")]
    wav: Option<PathBuf>,

    /// Cheat code: Game Genie (ABCD-EFGH) or raw (AAAAAA:VVVV); repeatable
    #[arg(long = "cheat", value_name = "CODE")]
    cheats: Vec<String>,

    /// File of cheat codes, one per line ('#' starts a comment)
    #[arg(long, value_name = "PATH")]
    cheat_file: Option<PathBuf>,

    /// Input script: buttons held on given frames ('FRAMES [PORT] BUTTONS' per line)
    #[arg(long, value_name = "PATH")]
    input: Option<PathBuf>,

    /// One input script line, e.g. "100-104 down" or "130 2 a+start"; repeatable
    #[arg(long = "press", value_name = "SPEC")]
    presses: Vec<String>,

    /// Console region: from the ROM header (auto), or forced
    #[arg(long, value_enum, default_value_t = RegionArg::Auto)]
    region: RegionArg,

    #[command(flatten)]
    window: WindowArgs,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum RegionArg {
    /// From the header country field (Region::detect)
    Auto,
    /// Japan, NTSC 60 Hz
    Jp,
    /// Japan, PAL 50 Hz
    JpPal,
    /// USA, NTSC 60 Hz
    Us,
    /// Europe, PAL 50 Hz
    Eu,
}

/// Region of the console for `cart`, and whether it was detected.
fn console_region(arg: RegionArg, cart: &Cartridge) -> (Region, bool) {
    match arg {
        RegionArg::Auto => (Region::detect(cart.region_field()), true),
        RegionArg::Jp => (Region::JapanNtsc, false),
        RegionArg::JpPal => (Region::JapanPal, false),
        RegionArg::Us => (Region::Usa, false),
        RegionArg::Eu => (Region::Europe, false),
    }
}

/// "Europe (PAL 50 Hz)", for messages.
fn region_label(region: Region) -> String {
    let name = match region {
        Region::JapanNtsc | Region::JapanPal => "Japan",
        Region::Usa => "USA",
        Region::Europe => "Europe",
    };
    let video = if region.is_pal() {
        "PAL 50 Hz"
    } else {
        "NTSC 60 Hz"
    };
    format!("{name} ({video})")
}

/// Options of the window (SDL2) mode.
#[derive(clap::Args, Debug, Clone)]
pub struct WindowArgs {
    /// Integer window scale
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=8))]
    pub scale: u32,

    /// Synchronise presentation with the display (default: automatic, when the
    /// display refresh matches the console's 60 or 50 Hz)
    #[arg(long, conflicts_with = "no_vsync")]
    pub vsync: bool,

    /// Never synchronise presentation with the display
    #[arg(long)]
    pub no_vsync: bool,

    /// No audio output; frames are paced by the clock instead
    #[arg(long)]
    pub mute: bool,

    /// Folder listed in the Library window (remembered for the next runs;
    /// default ~/roms)
    #[arg(long, value_name = "DIR")]
    pub library: Option<PathBuf>,

    /// Do not download box art (own pictures next to the ROMs and the
    /// cache are still shown)
    #[arg(long)]
    pub no_covers: bool,

    /// Show the frames per second (F10 toggles it)
    #[arg(long)]
    pub show_fps: bool,

    /// A trained model that can play Sonic 1 (runs/NAME/model.zip of
    /// python/train_ppo.py, run by python/agent_server.py in .venv): F4 or
    /// menu Game > AI player gives it pad 1
    #[arg(long, value_name = "MODEL")]
    pub agent: Option<PathBuf>,

    /// The AI always takes its most likely action (by default it samples
    /// them, as in training)
    #[arg(long)]
    pub agent_deterministic: bool,

    /// The AI takes pad 1 as soon as the game runs (instead of on F4)
    #[arg(long)]
    pub agent_on: bool,

    /// After a finished act the AI plays on into the next one (by default
    /// the act starts over, as in training); a death or a stall then starts
    /// over the current act
    #[arg(long)]
    pub agent_continue: bool,

    /// Use the system window decorations instead of our own title bar
    #[arg(long)]
    pub decorated: bool,

    /// Plug 6-button pads instead of 3-button pads (all modes)
    #[arg(long)]
    pub six_button: bool,

    /// Quit after N frames
    #[arg(long, value_name = "N")]
    pub max_frames: Option<u64>,

    /// After N frames, save the frame (--out) and/or print its hash (--hash), then quit
    #[arg(long, value_name = "N")]
    pub screenshot_at: Option<u64>,
}

fn parse_hex(s: &str) -> Result<u32, String> {
    let digits = s.trim_start_matches("0x").trim_start_matches('$');
    u32::from_str_radix(digits, 16).map_err(|e| format!("invalid hex address '{s}': {e}"))
}

/// Generated homebrew test ROMs (`md_core::testrom`) that can be written out.
fn test_rom(name: &str) -> Option<Vec<u8>> {
    use md_core::testrom::*;
    Some(match name {
        "checkerboard" => checkerboard(),
        "v30_checkerboard" => v30_checkerboard(),
        "checkerboard_dma" => checkerboard_dma(),
        "interrupts" => interrupts(),
        "pad_reader" => pad_reader(),
        "pad_display" => pad_display(),
        "showcase" => showcase(),
        "z80_loader" => z80_loader(),
        "psg_tone" => psg_tone(),
        "ym_tone" => ym_tone(),
        "sram_counter" => sram_counter(),
        "bank_switcher" => bank_switcher(),
        _ => return None,
    })
}

const TEST_ROM_NAMES: &str = "checkerboard, v30_checkerboard, checkerboard_dma, interrupts, pad_reader, pad_display, showcase, z80_loader, psg_tone, ym_tone, \
     sram_counter, bank_switcher";

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if let Some(name) = &args.write_test_rom {
        let out = required_rom(&args, "--write-test-rom")?;
        let Some(rom) = test_rom(name) else {
            bail!("unknown test ROM '{name}' (available: {TEST_ROM_NAMES})");
        };
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(out, rom).with_context(|| format!("writing {}", out.display()))?;
        println!("wrote {}", out.display());
        return Ok(());
    }

    let script = input_script(&args)?;
    let headless = args.info
        || args.disasm.is_some()
        || args.dump_audio.is_some()
        || args.dump_frame.is_some();
    if !headless {
        return run_window(&args, script);
    }
    let rom = required_rom(&args, "this mode")?;
    let cart = load_cartridge(rom)?;

    if args.info {
        headless::print_header(&cart);
        let (region, _) = console_region(RegionArg::Auto, &cart);
        println!("Console  : {} (detected)", region_label(region));
        return Ok(());
    }

    if let Some(count) = args.disasm {
        let from = args.from.unwrap_or(cart.header().initial_pc);
        for line in disassemble_rom(&cart, from, count) {
            println!("{line}");
        }
        return Ok(());
    }

    if let (Some(frames), Some(path)) = (args.dump_audio, &args.wav) {
        let mut console = new_console(cart, &args)?;
        let mut samples = Vec::new();
        for _ in 0..frames {
            run_scripted_frame(&mut console, &script, |_| Buttons::NONE);
            samples.extend_from_slice(console.audio_samples());
        }
        headless::write_wav(path, &samples)?;
        println!(
            "wrote {} ({} frames, {} samples)",
            path.display(),
            frames,
            samples.len() / 2
        );
        return Ok(());
    }

    if let Some(frames) = args.dump_frame {
        if args.out.is_none() && !args.hash {
            bail!("--dump-frame needs --out PATH and/or --hash");
        }
        let mut console = new_console(cart, &args)?;
        for _ in 0..frames {
            run_scripted_frame(&mut console, &script, |_| Buttons::NONE);
        }
        save_frame(&console, args.out.as_deref(), args.hash)?;
        return Ok(());
    }
    unreachable!("every headless mode returns above")
}

fn required_rom<'a>(args: &'a Args, mode: &str) -> anyhow::Result<&'a std::path::Path> {
    args.rom
        .as_deref()
        .with_context(|| format!("{mode} needs a ROM path"))
}

fn load_cartridge(rom: &std::path::Path) -> anyhow::Result<Cartridge> {
    Cartridge::from_bytes(&archive::read_rom(rom)?)
        .with_context(|| format!("loading {}", rom.display()))
}

/// A game ready to run in the window: its console (region, cheats, pads
/// from the options), its title and its file (for `.srm` / `.state`).
pub struct Game {
    pub console: Console,
    pub title: String,
    pub rom: PathBuf,
}

/// Loads the ROM (or archive) at `rom` with the command-line options: used
/// for the ROM given at launch and for files dropped on the window.
#[cfg(any(feature = "sdl", test))]
fn load_game(rom: &std::path::Path, args: &Args) -> anyhow::Result<Game> {
    let cart = load_cartridge(rom)?;
    let header = cart.header();
    let mut title = chrome::game_title(&header.overseas_title, &header.domestic_title, rom);
    if console_region(args.region, &cart).0.is_pal() {
        title.push_str(" (PAL)");
    }
    Ok(Game {
        console: new_console(cart, args)?,
        title,
        rom: rom.to_path_buf(),
    })
}

/// The script of `--input` followed by the `--press` lines.
fn input_script(args: &Args) -> anyhow::Result<InputScript> {
    let mut script = match &args.input {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            InputScript::parse(&text).with_context(|| path.display().to_string())?
        }
        None => InputScript::default(),
    };
    for (i, press) in args.presses.iter().enumerate() {
        script
            .add_line(press, i + 1)
            .with_context(|| format!("--press '{press}'"))?;
    }
    Ok(script)
}

/// Runs the next frame with the script's buttons for it, combined with the
/// buttons `held` reports per port (keyboard and pads in the window).
pub fn run_scripted_frame(
    console: &mut Console,
    script: &InputScript,
    held: impl Fn(usize) -> Buttons,
) {
    let frame = console.frame_count() + 1;
    for port in 0..script::PORTS {
        console.set_buttons(port, held(port) | script.buttons(frame, port));
    }
    console.run_frame();
}

/// The console with the requested cheats active.
fn new_console(cart: Cartridge, args: &Args) -> anyhow::Result<Console> {
    let mut codes = args.cheats.clone();
    if let Some(path) = &args.cheat_file {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        codes.extend(cheat_lines(&text).map(str::to_string));
    }
    let cheats = codes
        .iter()
        .map(|code| md_core::cheats::parse(code))
        .collect::<Result<Vec<_>, _>>()?;
    let (region, detected) = console_region(args.region, &cart);
    eprintln!(
        "region: {}{}",
        region_label(region),
        if detected {
            ", from the header"
        } else {
            ", forced"
        }
    );
    let mut console = Console::new(
        cart,
        MdConfig {
            region,
            ..MdConfig::default()
        },
    );
    console.set_cheats(&cheats)?;
    if args.window.six_button {
        for port in 0..script::PORTS {
            console.connect_pad(port, Some(PadKind::SixButton));
        }
    }
    for cheat in &cheats {
        eprintln!("cheat {cheat}");
    }
    Ok(console)
}

/// Codes of a cheat file: one per line, `#` comments, blank lines ignored.
fn cheat_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
}

/// Writes the current frame to `out` and/or prints its hash.
fn save_frame(console: &Console, out: Option<&std::path::Path>, hash: bool) -> anyhow::Result<()> {
    if let Some(out) = out {
        headless::write_png(out, console)?;
        println!("wrote {} (frame {})", out.display(), console.frame_count());
    }
    if hash {
        println!("{:016x}", console.frame_hash());
    }
    Ok(())
}

#[cfg(feature = "sdl")]
fn run_window(args: &Args, script: InputScript) -> anyhow::Result<()> {
    if args.window.screenshot_at.is_some() && args.out.is_none() && !args.hash {
        bail!("--screenshot-at needs --out PATH and/or --hash");
    }
    let game = args
        .rom
        .as_deref()
        .map(|rom| load_game(rom, args))
        .transpose()?;
    let output = sdl::Output {
        path: args.out.clone(),
        hash: args.hash,
        script,
    };
    sdl::run(
        game,
        &args.window,
        output,
        &|rom| load_game(rom, args),
        library_dir(args),
    )
}

/// Folder of the Library window: `--library` (then remembered), else the
/// remembered one, else ~/roms.
#[cfg(feature = "sdl")]
fn library_dir(args: &Args) -> PathBuf {
    let mut config = config::Config::load();
    if let Some(dir) = &args.window.library {
        if config.library.as_ref() != Some(dir) {
            config.library = Some(dir.clone());
            config.save();
        }
        return dir.clone();
    }
    config.library.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("roms")
    })
}

#[cfg(not(feature = "sdl"))]
fn run_window(args: &Args, _script: InputScript) -> anyhow::Result<()> {
    let rom = required_rom(args, "the window mode (built without `sdl`)")?;
    eprintln!("built without the `sdl` feature: no window; ROM header follows");
    headless::print_header(&load_cartridge(rom)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loader used for the launch ROM and dropped files: a generated
    /// ROM loads with its header title; a non-ROM file is an error (the
    /// window shows a message and keeps the current game).
    #[test]
    fn load_game_for_dropped_files() {
        let dir = std::env::temp_dir().join(format!("md-rs-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("showcase.bin");
        std::fs::write(&rom, md_core::testrom::showcase()).unwrap();
        let args = Args::parse_from(["md-frontend"]);
        let game = load_game(&rom, &args).unwrap();
        assert_eq!(game.title, "MD-RS TEST ROM");
        assert_eq!(game.rom, rom);
        let eu = Args::parse_from(["md-frontend", "--region", "eu"]);
        assert_eq!(load_game(&rom, &eu).unwrap().title, "MD-RS TEST ROM (PAL)");
        let text = dir.join("notes.txt");
        std::fs::write(&text, b"not a ROM").unwrap();
        assert!(load_game(&text, &args).is_err());
        assert!(load_game(&dir.join("missing.bin"), &args).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cheat_file_lines() {
        let text = "# Sonic-style list\nSCRA-MBLE\n\n  FF0000:0042   # lives\n#\n";
        assert_eq!(
            cheat_lines(text).collect::<Vec<_>>(),
            ["SCRA-MBLE", "FF0000:0042"]
        );
    }
}

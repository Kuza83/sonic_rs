//! Without YM2612 synthesis the game must run exactly the same: on the
//! generated ROMs that use the chip (`ym_tone` loads a Z80 driver that waits
//! on the busy flag before every write), everything the game can change is
//! identical after the same frames; only the sound differs.

use md_core::testrom::{ym_tone, z80_loader};
use md_core::{Cartridge, Console, MdConfig};

/// Everything the game can see or change, for comparison.
#[derive(PartialEq)]
struct Visible {
    work_ram: Vec<u8>,
    z80_ram: Vec<u8>,
    vram: Vec<u8>,
    cram: Vec<u16>,
    vsram: Vec<u16>,
    cpu: String,
    z80: String,
    picture: u64,
}

fn visible(console: &Console) -> Visible {
    let bus = console.bus();
    Visible {
        work_ram: bus.work_ram().to_vec(),
        z80_ram: bus.z80_space().ram().to_vec(),
        vram: bus.vdp().vram().to_vec(),
        cram: bus.vdp().cram().to_vec(),
        vsram: bus.vdp().vsram().to_vec(),
        cpu: format!("{:?}", console.cpu()),
        z80: format!("{:?}", console.z80()),
        picture: console.frame_hash(),
    }
}

fn compare(name: &str, rom: Vec<u8>, frames: usize) -> (u64, u64) {
    let new = || Console::new(Cartridge::from_bytes(&rom).unwrap(), MdConfig::default());
    let (mut full, mut silent) = (new(), new());
    silent.set_fm_synthesis(false);
    assert!(full.fm_synthesis() && !silent.fm_synthesis());
    let (mut full_energy, mut silent_energy) = (0u64, 0u64);
    for frame in 0..frames {
        full.run_frame();
        silent.run_frame();
        assert!(
            visible(&full) == visible(&silent),
            "{name}: frame {frame} differs"
        );
        assert_eq!(
            full.audio_samples().len(),
            silent.audio_samples().len(),
            "{name}: same number of samples"
        );
        full_energy += full
            .audio_samples()
            .iter()
            .map(|s| s.unsigned_abs() as u64)
            .sum::<u64>();
        silent_energy += silent
            .audio_samples()
            .iter()
            .map(|s| s.unsigned_abs() as u64)
            .sum::<u64>();
    }
    (full_energy, silent_energy)
}

#[test]
fn ym_tone_runs_the_same_but_silent() {
    let (full, silent) = compare("ym_tone", ym_tone(), 120);
    assert!(full > 0, "the full chip plays the tone");
    assert_eq!(silent, 0, "nothing else sounds in this ROM");
}

#[test]
fn z80_program_runs_the_same() {
    compare("z80_loader", z80_loader(), 60);
}

#[test]
fn the_setting_survives_resets_and_states() {
    let mut console = Console::new(
        Cartridge::from_bytes(&ym_tone()).unwrap(),
        MdConfig::default(),
    );
    console.set_fm_synthesis(false);
    console.run_frame();
    let state = console.save_state();
    console.power_cycle();
    assert!(!console.fm_synthesis(), "kept by a power cycle");
    console.reset_soft();
    assert!(!console.fm_synthesis(), "kept by a soft reset");
    console.load_state(&state).unwrap();
    assert!(!console.fm_synthesis(), "not part of the state");
    let mut other = Console::new(
        Cartridge::from_bytes(&ym_tone()).unwrap(),
        MdConfig::default(),
    );
    other.load_state(&state).unwrap();
    assert!(
        other.fm_synthesis(),
        "a state made without synthesis loads with sound"
    );
}

//! End-to-end input: a generated ROM reads port 1 through TH pulses; the
//! values are checked against the hand-derived tables of the hardware docs.

use md_core::testrom::{PAD_READS, pad_reader};
use md_core::{Buttons, Cartridge, Console, MdConfig, PadKind};

/// Runs the reader with a pad of `kind` holding `buttons`; returns the 8
/// bytes stored by the ROM (TH low, high, low, high, ...).
fn read_pad(kind: PadKind, buttons: Buttons) -> [u8; 8] {
    let cart = Cartridge::from_bytes(&pad_reader()).unwrap();
    let mut console = Console::new(cart, MdConfig::default());
    console.connect_pad(0, Some(kind));
    console.set_buttons(0, buttons);
    for _ in 0..3 {
        console.run_frame();
    }
    let ram = console.bus().work_ram();
    let at = (PAD_READS & 0xFFFF) as usize;
    ram[at..at + 8].try_into().unwrap()
}

#[test]
fn three_button_pad_through_the_cpu() {
    let reads = read_pad(PadKind::ThreeButton, Buttons::A | Buttons::UP | Buttons::C);
    // TH = 0: ?0SA00DU with A, U pressed -> 0010_0010 ; TH = 1: ?1CBRLDU with C, U -> 0101_1110
    assert_eq!(reads, [0x22, 0x5E, 0x22, 0x5E, 0x22, 0x5E, 0x22, 0x5E]);
}

#[test]
fn six_button_pad_through_the_cpu() {
    let reads = read_pad(
        PadKind::SixButton,
        Buttons::START | Buttons::X | Buttons::MODE,
    );
    assert_eq!(
        reads,
        [
            0x13, // ?0SA00DU (START)
            0x7F, // ?1CBRLDU
            0x13, 0x7F, 0x10, // D3-D0 forced to 0: 6-button signature
            0x73, // ?1CBMXYZ (X, MODE)
            0x1F, // ?0SA1111
            0x7F,
        ]
    );
}

#[test]
fn unplugged_port_reads_pulled_up_inputs() {
    let cart = Cartridge::from_bytes(&pad_reader()).unwrap();
    let mut console = Console::new(cart, MdConfig::default());
    console.connect_pad(0, None);
    console.run_frame();
    let at = (PAD_READS & 0xFFFF) as usize;
    let ram = console.bus().work_ram();
    // D7 latch 0, D6 = TH as driven by the I/O chip, D5-D0 pulled high.
    assert_eq!(&ram[at..at + 2], &[0x3F, 0x7F]);
}

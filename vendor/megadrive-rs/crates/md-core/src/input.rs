//! Controllers plugged into the I/O ports: 3-button and 6-button pads.
//!
//! Behaviour from Genesis Plus GX core/input_hw/gamepad.c (itself based on
//! gen-hw.txt) and Plutiedev "Controllers". The pad answers on D5-D0 (active
//! low) according to the TH line:
//! - TH = 1: `?1CB RLDU`; TH = 0: `?0SA 00DU`;
//! - 6-button pads count TH 0→1 transitions: on the 3rd low phase D3-D0 read
//!   0, on the following high phase `?1CB MXYZ`, then `?0SA 1111`; the count
//!   resets on the 27th scanline without a 0→1 transition (`gamepad_refresh`:
//!   `if (Timeout++ > 25)`).

use std::ops::BitOr;

/// Pressed buttons, one bit each (GPGX input.h layout).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Buttons(u16);

impl Buttons {
    pub const NONE: Buttons = Buttons(0);
    pub const UP: Buttons = Buttons(1 << 0);
    pub const DOWN: Buttons = Buttons(1 << 1);
    pub const LEFT: Buttons = Buttons(1 << 2);
    pub const RIGHT: Buttons = Buttons(1 << 3);
    pub const B: Buttons = Buttons(1 << 4);
    pub const C: Buttons = Buttons(1 << 5);
    pub const A: Buttons = Buttons(1 << 6);
    pub const START: Buttons = Buttons(1 << 7);
    pub const Z: Buttons = Buttons(1 << 8);
    pub const Y: Buttons = Buttons(1 << 9);
    pub const X: Buttons = Buttons(1 << 10);
    pub const MODE: Buttons = Buttons(1 << 11);

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, other: Buttons) -> bool {
        self.0 & other.0 == other.0
    }
}

impl Buttons {
    /// Both sets of buttons together (`|` for constants).
    pub const fn union(self, other: Buttons) -> Buttons {
        Buttons(self.0 | other.0)
    }
}

impl BitOr for Buttons {
    type Output = Buttons;
    fn bitor(self, rhs: Buttons) -> Buttons {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    ThreeButton,
    SixButton,
}

/// TH line bit (D6) of the data and control registers.
pub const TH: u8 = 0x40;
/// D5-D0: pad inputs.
const INPUT_BITS: u8 = 0x3F;
/// The 6-button pulse counter advances by 2 per TH 0→1 edge and stops at 8.
const COUNTER_STEP: u8 = 2;
const COUNTER_MAX: u8 = 8;
/// Idle scanlines tolerated before the 6-button sequence restarts.
const SIX_BUTTON_TIMEOUT_LINES: u8 = 26;

/// Sequence steps (counter | TH) with a special 6-button meaning.
const STEP_THIRD_LOW: u8 = 4;
const STEP_FOURTH_LOW: u8 = 6;
const STEP_FOURTH_HIGH: u8 = 7;

#[derive(Debug, Clone)]
pub struct Gamepad {
    kind: PadKind,
    buttons: Buttons,
    /// TH level seen by the pad (pulled high when TH is an input).
    th_high: bool,
    counter: u8,
    idle_lines: u8,
}

impl Gamepad {
    pub fn new(kind: PadKind) -> Self {
        Self {
            kind,
            buttons: Buttons::NONE,
            th_high: true,
            counter: 0,
            idle_lines: 0,
        }
    }

    pub fn kind(&self) -> PadKind {
        self.kind
    }

    pub fn set_buttons(&mut self, buttons: Buttons) {
        self.buttons = buttons;
    }

    /// D6-D0 as driven by the pad (D7 is not connected and reads 0 here;
    /// the I/O chip substitutes its data latch for output bits).
    pub fn read(&self) -> u8 {
        let pad = self.buttons.bits();
        let th = if self.th_high { TH } else { 0 };
        let step = self.counter | self.th_high as u8;
        // Bits to pull low (pressed buttons, or forced zeros).
        let start_a = ((pad >> 2) & 0x30) as u8; // START -> D5, A -> D4
        let low = match step {
            STEP_THIRD_LOW => start_a | 0x0F,
            STEP_FOURTH_HIGH => (pad & 0x30) as u8 | ((pad >> 8) & 0x0F) as u8,
            STEP_FOURTH_LOW => start_a,
            _ if self.th_high => (pad & INPUT_BITS as u16) as u8,
            _ => (pad & 0x03) as u8 | start_a | 0x0C,
        };
        (th | INPUT_BITS) & !low
    }

    /// The I/O chip drives the port: `data` is its data latch, `ctrl` its
    /// direction register (1 = output). Only TH matters to a pad.
    pub fn write(&mut self, data: u8, ctrl: u8) {
        let th_high = if ctrl & TH != 0 {
            data & TH != 0
        } else {
            // TH is pulled high when not driven. (GPGX also models a 172-cycle
            // switching latency here, not reproduced yet.)
            true
        };
        if self.kind == PadKind::SixButton
            && ctrl & TH != 0
            && th_high
            && !self.th_high
            && self.counter < COUNTER_MAX
        {
            self.counter += COUNTER_STEP;
            self.idle_lines = 0;
        }
        self.th_high = th_high;
    }

    /// Called once per scanline: restarts the 6-button sequence after a pause.
    pub fn refresh_line(&mut self) {
        if self.idle_lines > SIX_BUTTON_TIMEOUT_LINES - 1 {
            self.counter = 0;
            self.idle_lines = 0;
        } else {
            self.idle_lines += 1;
        }
    }
}

impl savestate::State for Gamepad {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        savestate::enum_field(
            v,
            &mut self.kind,
            "pad kind",
            |k| k as u8,
            |c| match c {
                0 => Some(PadKind::ThreeButton),
                1 => Some(PadKind::SixButton),
                _ => None,
            },
        )?;
        v.field(&mut self.buttons.0)?;
        v.field(&mut self.th_high)?;
        v.field(&mut self.counter)?;
        v.field(&mut self.idle_lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives TH as an output with the given level and reads the pad.
    fn th(pad: &mut Gamepad, high: bool) -> u8 {
        pad.write(if high { TH } else { 0 }, TH);
        pad.read()
    }

    #[test]
    fn three_button_pad_tables() {
        let mut pad = Gamepad::new(PadKind::ThreeButton);
        pad.set_buttons(Buttons::A | Buttons::UP | Buttons::C);
        // ?1CBRLDU with C and U pressed (active low)
        assert_eq!(th(&mut pad, true), 0b0101_1110);
        // ?0SA00DU with A and U pressed
        assert_eq!(th(&mut pad, false), 0b0010_0010);
    }

    #[test]
    fn released_pad_reads_all_ones_on_inputs() {
        let mut pad = Gamepad::new(PadKind::ThreeButton);
        assert_eq!(th(&mut pad, true), 0x7F);
        assert_eq!(th(&mut pad, false), 0x33);
    }

    #[test]
    fn six_button_sequence() {
        let mut pad = Gamepad::new(PadKind::SixButton);
        pad.set_buttons(Buttons::START | Buttons::X | Buttons::MODE);
        let reads: Vec<u8> = [false, true, false, true, false, true, false, true]
            .into_iter()
            .map(|level| th(&mut pad, level))
            .collect();
        assert_eq!(
            reads,
            [
                0x13, // ?0SA00DU, START pressed
                0x7F, // ?1CBRLDU
                0x13, 0x7F, 0x10, // third low: D3-D0 forced to 0
                0x73, // ?1CBMXYZ, X and MODE pressed
                0x1F, // ?0SA1111
                0x7F, // back to 3-button values
            ]
        );
    }

    #[test]
    fn six_button_sequence_restarts_after_timeout() {
        let mut pad = Gamepad::new(PadKind::SixButton);
        pad.set_buttons(Buttons::X);
        for level in [false, true, false, true, false] {
            th(&mut pad, level);
        }
        for _ in 0..SIX_BUTTON_TIMEOUT_LINES {
            pad.refresh_line();
        }
        // Not yet: the next high phase still returns the extra buttons.
        let mut early = pad.clone();
        assert_eq!(th(&mut early, true), 0x7B);
        pad.refresh_line();
        // Counter reset: the next high phase is a plain 3-button read.
        assert_eq!(th(&mut pad, true), 0x7F);
    }

    #[test]
    fn three_button_pad_ignores_the_sequence() {
        let mut pad = Gamepad::new(PadKind::ThreeButton);
        pad.set_buttons(Buttons::X);
        for _ in 0..3 {
            th(&mut pad, false);
            th(&mut pad, true);
        }
        assert_eq!(th(&mut pad, false), 0x33);
    }

    #[test]
    fn undriven_th_is_pulled_high() {
        let mut pad = Gamepad::new(PadKind::ThreeButton);
        pad.write(0x00, 0x00);
        assert_eq!(pad.read() & TH, TH);
    }
}

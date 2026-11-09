//! Host inputs to Mega Drive buttons, independent of SDL.
//!
//! Inputs are identified by SDL's stable names (`SDL_GetKeyName` for keys,
//! `SDL_GameControllerGetStringForButton` for controller buttons) so the
//! mapping can be tested without a window.

use md_core::Buttons;

/// Keyboard layout: arrows, A/S/D = A/B/C, Q/W/E = X/Y/Z, Return = Start,
/// right Shift = Mode.
pub fn keyboard_button(key: &str) -> Option<Buttons> {
    Some(match key {
        "Up" => Buttons::UP,
        "Down" => Buttons::DOWN,
        "Left" => Buttons::LEFT,
        "Right" => Buttons::RIGHT,
        "A" => Buttons::A,
        "S" => Buttons::B,
        "D" => Buttons::C,
        "Q" => Buttons::X,
        "W" => Buttons::Y,
        "E" => Buttons::Z,
        "Return" => Buttons::START,
        "Right Shift" => Buttons::MODE,
        _ => return None,
    })
}

/// Save state slot chosen with the digit keys 0-9.
pub fn slot_key(key: &str) -> Option<u8> {
    match key.as_bytes() {
        [digit @ b'0'..=b'9'] => Some(digit - b'0'),
        _ => None,
    }
}

/// Controller layout (positions of the 6-button pad): the bottom face row
/// X/A/B is A/B/C, LB/Y/RB is X/Y/Z, Back is Mode.
pub fn controller_button(button: &str) -> Option<Buttons> {
    Some(match button {
        "dpup" => Buttons::UP,
        "dpdown" => Buttons::DOWN,
        "dpleft" => Buttons::LEFT,
        "dpright" => Buttons::RIGHT,
        "x" => Buttons::A,
        "a" => Buttons::B,
        "b" => Buttons::C,
        "leftshoulder" => Buttons::X,
        "y" => Buttons::Y,
        "rightshoulder" => Buttons::Z,
        "start" => Buttons::START,
        "back" => Buttons::MODE,
        _ => return None,
    })
}

/// Stick deflection needed to register a direction (half of full scale).
pub const STICK_DEADZONE: i16 = i16::MAX / 2;

/// Directions of the left stick (SDL axes: +x right, +y down).
pub fn stick_directions(x: i16, y: i16) -> Buttons {
    let mut b = Buttons::NONE;
    if x <= -STICK_DEADZONE {
        b = b | Buttons::LEFT;
    } else if x >= STICK_DEADZONE {
        b = b | Buttons::RIGHT;
    }
    if y <= -STICK_DEADZONE {
        b = b | Buttons::UP;
    } else if y >= STICK_DEADZONE {
        b = b | Buttons::DOWN;
    }
    b
}

/// Held buttons of one source (keyboard or a controller), as a set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Held(u16);

impl Held {
    pub fn set(&mut self, button: Buttons, pressed: bool) {
        if pressed {
            self.0 |= button.bits();
        } else {
            self.0 &= !button.bits();
        }
    }

    pub fn buttons(self) -> u16 {
        self.0
    }
}

/// Union of several sources' held buttons.
pub fn combine(sources: &[u16]) -> Buttons {
    let bits = sources.iter().fold(0, |acc, s| acc | s);
    ALL.iter()
        .copied()
        .filter(|b| bits & b.bits() != 0)
        .fold(Buttons::NONE, |acc, b| acc | b)
}

const ALL: [Buttons; 12] = [
    Buttons::UP,
    Buttons::DOWN,
    Buttons::LEFT,
    Buttons::RIGHT,
    Buttons::A,
    Buttons::B,
    Buttons::C,
    Buttons::START,
    Buttons::X,
    Buttons::Y,
    Buttons::Z,
    Buttons::MODE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_layout() {
        let expected = [
            ("Up", Buttons::UP),
            ("Down", Buttons::DOWN),
            ("Left", Buttons::LEFT),
            ("Right", Buttons::RIGHT),
            ("A", Buttons::A),
            ("S", Buttons::B),
            ("D", Buttons::C),
            ("Q", Buttons::X),
            ("W", Buttons::Y),
            ("E", Buttons::Z),
            ("Return", Buttons::START),
            ("Right Shift", Buttons::MODE),
        ];
        for (key, button) in expected {
            assert_eq!(keyboard_button(key), Some(button), "{key}");
        }
        assert_eq!(
            keyboard_button("Escape"),
            None,
            "hotkeys are not pad buttons"
        );
        assert_eq!(keyboard_button("Left Shift"), None);
    }

    #[test]
    fn controller_layout() {
        let expected = [
            ("dpup", Buttons::UP),
            ("dpdown", Buttons::DOWN),
            ("dpleft", Buttons::LEFT),
            ("dpright", Buttons::RIGHT),
            ("x", Buttons::A),
            ("a", Buttons::B),
            ("b", Buttons::C),
            ("leftshoulder", Buttons::X),
            ("y", Buttons::Y),
            ("rightshoulder", Buttons::Z),
            ("start", Buttons::START),
            ("back", Buttons::MODE),
        ];
        for (name, button) in expected {
            assert_eq!(controller_button(name), Some(button), "{name}");
        }
        assert_eq!(controller_button("guide"), None);
    }

    #[test]
    fn every_button_is_reachable_from_both_layouts() {
        let all = ALL.iter().fold(Buttons::NONE, |acc, &b| acc | b);
        let keys = [
            "Up",
            "Down",
            "Left",
            "Right",
            "A",
            "S",
            "D",
            "Q",
            "W",
            "E",
            "Return",
            "Right Shift",
        ];
        let from_keys = keys
            .iter()
            .filter_map(|k| keyboard_button(k))
            .fold(Buttons::NONE, |acc, b| acc | b);
        assert_eq!(from_keys, all);
    }

    #[test]
    fn digit_keys_select_slots() {
        assert_eq!(slot_key("0"), Some(0));
        assert_eq!(slot_key("7"), Some(7));
        assert_eq!(slot_key("A"), None);
        assert_eq!(slot_key("Keypad 1"), None);
        for key in ["0", "5", "9"] {
            assert_eq!(keyboard_button(key), None, "slots do not shadow pad keys");
        }
    }

    #[test]
    fn stick_deadzone_and_diagonals() {
        assert_eq!(stick_directions(0, 0), Buttons::NONE);
        assert_eq!(stick_directions(STICK_DEADZONE - 1, 0), Buttons::NONE);
        assert_eq!(stick_directions(STICK_DEADZONE, 0), Buttons::RIGHT);
        assert_eq!(stick_directions(i16::MIN, 0), Buttons::LEFT);
        assert_eq!(stick_directions(0, i16::MIN), Buttons::UP);
        assert_eq!(
            stick_directions(i16::MAX, i16::MAX),
            Buttons::RIGHT | Buttons::DOWN
        );
    }

    #[test]
    fn held_sets_combine() {
        let mut keyboard = Held::default();
        let mut pad = Held::default();
        keyboard.set(Buttons::A, true);
        pad.set(Buttons::START, true);
        pad.set(Buttons::A, true);
        assert_eq!(
            combine(&[keyboard.buttons(), pad.buttons()]),
            Buttons::A | Buttons::START
        );
        pad.set(Buttons::A, false);
        assert_eq!(
            combine(&[keyboard.buttons(), pad.buttons()]),
            Buttons::A | Buttons::START,
            "A still held on the keyboard"
        );
        keyboard.set(Buttons::A, false);
        assert_eq!(
            combine(&[keyboard.buttons(), pad.buttons()]),
            Buttons::START
        );
    }
}

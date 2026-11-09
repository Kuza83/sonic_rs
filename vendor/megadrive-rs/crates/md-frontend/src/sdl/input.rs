//! Keyboard and game controllers to pad buttons and hotkeys.
//!
//! Port 1 takes the keyboard and the first controller, port 2 the second
//! controller (in connection order; hot-plugging is followed).

use md_core::Buttons;
use sdl2::GameControllerSubsystem;
use sdl2::controller::{Axis, GameController};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;

use super::sdl_error;
use crate::keymap::{
    Held, combine, controller_button, keyboard_button, slot_key, stick_directions,
};

/// What the loop must do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    TogglePause,
    FastForward(bool),
    ToggleFullscreen,
    Screenshot,
    SaveState,
    LoadState,
    SelectSlot(u8),
    Reset,
    ToggleFps,
    ToggleMenu,
    /// Back to the Library window (keyboard and controller to it).
    ShowLibrary,
    /// The AI player takes pad 1, or gives it back (F4).
    ToggleAgent,
}

struct Pad {
    controller: GameController,
    held: Held,
    stick: (i16, i16),
}

impl Pad {
    fn buttons(&self) -> u16 {
        self.held.buttons() | stick_directions(self.stick.0, self.stick.1).bits()
    }
}

pub struct Input {
    subsystem: GameControllerSubsystem,
    keyboard: Held,
    pads: Vec<Pad>,
}

impl Input {
    pub fn new(context: &sdl2::Sdl) -> anyhow::Result<Self> {
        Ok(Self {
            subsystem: context.game_controller().map_err(sdl_error)?,
            keyboard: Held::default(),
            pads: Vec::new(),
        })
    }

    pub fn handle(&mut self, event: &Event) -> Action {
        match *event {
            Event::Quit { .. } => Action::Quit,
            Event::KeyDown {
                keycode: Some(key),
                repeat,
                ..
            } => match key {
                Keycode::Escape if !repeat => Action::ToggleMenu,
                Keycode::Space => Action::FastForward(true),
                Keycode::Tab if !repeat => Action::ShowLibrary,
                Keycode::P if !repeat => Action::TogglePause,
                Keycode::F11 if !repeat => Action::ToggleFullscreen,
                Keycode::F12 if !repeat => Action::Screenshot,
                Keycode::F10 if !repeat => Action::ToggleFps,
                Keycode::F4 if !repeat => Action::ToggleAgent,
                Keycode::F5 if !repeat => Action::SaveState,
                Keycode::F8 if !repeat => Action::LoadState,
                Keycode::Backspace if !repeat => Action::Reset,
                _ => match slot_key(&key.name()) {
                    Some(slot) => Action::SelectSlot(slot),
                    None => self.key(key, true),
                },
            },
            Event::KeyUp {
                keycode: Some(key), ..
            } => match key {
                Keycode::Space => Action::FastForward(false),
                _ => self.key(key, false),
            },
            Event::ControllerDeviceAdded { which, .. } => {
                // A controller that fails to open is ignored (keyboard still works).
                match self.subsystem.open(which) {
                    Ok(controller) => self.pads.push(Pad {
                        controller,
                        held: Held::default(),
                        stick: (0, 0),
                    }),
                    Err(e) => eprintln!("controller {which}: {e}"),
                }
                Action::None
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                self.pads.retain(|p| p.controller.instance_id() != which);
                Action::None
            }
            Event::ControllerButtonDown { which, button, .. } => {
                self.pad_button(which, &button.string(), true)
            }
            Event::ControllerButtonUp { which, button, .. } => {
                self.pad_button(which, &button.string(), false)
            }
            Event::ControllerAxisMotion {
                which, axis, value, ..
            } => {
                if let Some(pad) = self.pad(which) {
                    match axis {
                        Axis::LeftX => pad.stick.0 = value,
                        Axis::LeftY => pad.stick.1 = value,
                        _ => {}
                    }
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Buttons held for `port` (0 = port 1).
    pub fn buttons(&self, port: usize) -> Buttons {
        let pad = self.pads.get(port).map_or(0, Pad::buttons);
        if port == 0 {
            combine(&[self.keyboard.buttons(), pad])
        } else {
            combine(&[pad])
        }
    }

    /// Releases the keyboard's game buttons (the Game window lost the
    /// focus: its key-up events will go elsewhere).
    pub fn release_keyboard(&mut self) {
        self.keyboard = Held::default();
    }

    fn key(&mut self, key: Keycode, pressed: bool) -> Action {
        if let Some(button) = keyboard_button(&key.name()) {
            self.keyboard.set(button, pressed);
        }
        Action::None
    }

    fn pad(&mut self, instance: u32) -> Option<&mut Pad> {
        self.pads
            .iter_mut()
            .find(|p| p.controller.instance_id() == instance)
    }

    fn pad_button(&mut self, instance: u32, name: &str, pressed: bool) -> Action {
        if name == "guide" {
            return if pressed {
                Action::ToggleMenu
            } else {
                Action::None
            };
        }
        if let (Some(button), Some(pad)) = (controller_button(name), self.pad(instance)) {
            pad.held.set(button, pressed);
        }
        Action::None
    }
}

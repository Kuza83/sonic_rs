//! Input scripts: buttons held on given frames, for reproducible runs.
//!
//! One press per line, `#` starts a comment:
//! ```text
//! # frames  [port]  buttons
//! 100-104           down        # held on frames 100 to 104 inclusive
//! 130               a+start     # a single frame
//! 200-210   2       right       # port 2 (port 1 by default)
//! ```
//! Frame n is the n-th `run_frame` call (counting from 1, like
//! `--dump-frame`); its buttons are set just before it runs. Lines covering
//! the same frame and port combine.

use md_core::Buttons;

/// Ports a script can drive (1 and 2 in the text, 0 and 1 here).
pub const PORTS: usize = 2;

const BUTTON_NAMES: [(&str, Buttons); 12] = [
    ("up", Buttons::UP),
    ("down", Buttons::DOWN),
    ("left", Buttons::LEFT),
    ("right", Buttons::RIGHT),
    ("a", Buttons::A),
    ("b", Buttons::B),
    ("c", Buttons::C),
    ("start", Buttons::START),
    ("x", Buttons::X),
    ("y", Buttons::Y),
    ("z", Buttons::Z),
    ("mode", Buttons::MODE),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Press {
    first: u64,
    last: u64,
    port: usize,
    buttons: Buttons,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputScript {
    presses: Vec<Press>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("input script line {line}: {message}")]
pub struct ScriptError {
    pub line: usize,
    pub message: String,
}

impl InputScript {
    /// Parses a whole script.
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let mut script = Self::default();
        for (i, line) in text.lines().enumerate() {
            script.add_line(line, i + 1)?;
        }
        Ok(script)
    }

    /// Adds one line of script syntax (`--press`), numbered `line` in errors.
    pub fn add_line(&mut self, line: &str, number: usize) -> Result<(), ScriptError> {
        let error = |message: String| ScriptError {
            line: number,
            message,
        };
        let text = line.split('#').next().unwrap_or("");
        let fields: Vec<&str> = text.split_whitespace().collect();
        let (frames, port, buttons) = match fields[..] {
            [] => return Ok(()),
            [frames, buttons] => (frames, "1", buttons),
            [frames, port, buttons] => (frames, port, buttons),
            _ => {
                return Err(error(format!(
                    "expected 'FRAMES [PORT] BUTTONS', got '{}'",
                    text.trim()
                )));
            }
        };
        let frame = |s: &str| {
            s.parse::<u64>()
                .ok()
                .filter(|&f| f > 0)
                .ok_or_else(|| error(format!("invalid frame '{s}' (frames start at 1)")))
        };
        let (first, last) = match frames.split_once('-') {
            Some((a, b)) => (frame(a)?, frame(b)?),
            None => (frame(frames)?, frame(frames)?),
        };
        if last < first {
            return Err(error(format!(
                "frame range '{frames}' ends before it starts"
            )));
        }
        let port = match port.parse::<usize>() {
            Ok(p @ 1..=PORTS) => p - 1,
            _ => return Err(error(format!("invalid port '{port}' (1 or 2)"))),
        };
        let mut held = Buttons::NONE;
        for name in buttons.split('+') {
            held = held
                | BUTTON_NAMES
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|&(_, b)| b)
                    .ok_or_else(|| error(format!("unknown button '{name}'")))?;
        }
        self.presses.push(Press {
            first,
            last,
            port,
            buttons: held,
        });
        Ok(())
    }

    /// Buttons held on `port` (0-based) during frame `frame` (1-based).
    pub fn buttons(&self, frame: u64, port: usize) -> Buttons {
        self.presses
            .iter()
            .filter(|p| p.port == port && (p.first..=p.last).contains(&frame))
            .fold(Buttons::NONE, |held, p| held | p.buttons)
    }

    pub fn is_empty(&self) -> bool {
        self.presses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_single_frames_and_ports() {
        let script = InputScript::parse("100-102 down\n130 1 a\n200 2 RIGHT\n").unwrap();
        assert_eq!(script.buttons(99, 0), Buttons::NONE);
        for frame in 100..=102 {
            assert_eq!(script.buttons(frame, 0), Buttons::DOWN, "frame {frame}");
        }
        assert_eq!(script.buttons(103, 0), Buttons::NONE);
        assert_eq!(script.buttons(130, 0), Buttons::A);
        assert_eq!(script.buttons(131, 0), Buttons::NONE);
        assert_eq!(script.buttons(200, 0), Buttons::NONE, "port 2 only");
        assert_eq!(script.buttons(200, 1), Buttons::RIGHT);
    }

    #[test]
    fn combinations_and_overlaps() {
        let script = InputScript::parse("10-20 a+Start\n15 c\n").unwrap();
        assert_eq!(script.buttons(10, 0), Buttons::A | Buttons::START);
        assert_eq!(
            script.buttons(15, 0),
            Buttons::A | Buttons::START | Buttons::C
        );
        let all = BUTTON_NAMES.map(|(n, _)| n).join("+");
        let every = InputScript::parse(&format!("1 {all}")).unwrap();
        assert_eq!(every.buttons(1, 0).bits(), 0x0FFF);
    }

    #[test]
    fn comments_and_blank_lines() {
        let script = InputScript::parse("# menu\n\n  5 b   # confirm\n#\n").unwrap();
        assert_eq!(script.buttons(5, 0), Buttons::B);
        assert!(InputScript::parse("# nothing\n").unwrap().is_empty());
    }

    #[test]
    fn errors_name_the_line() {
        let err = |text| InputScript::parse(text).unwrap_err();
        assert_eq!(err("1 a\n2 jump").line, 2);
        assert!(err("2 jump").message.contains("jump"));
        assert!(err("10-5 a").message.contains("ends before"));
        assert!(err("0 a").message.contains("frames start at 1"));
        assert!(err("5 3 a").message.contains("port"));
        assert!(err("5 a+").message.contains("unknown button ''"));
        assert!(err("5").message.contains("expected"));
        assert!(err("x a").message.contains("invalid frame"));
    }
}

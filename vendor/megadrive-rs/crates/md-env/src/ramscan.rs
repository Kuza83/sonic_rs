//! Finding in work RAM what changes when something happens in the game:
//! given a copy of the RAM after each step of an episode, the bytes that
//! stayed still and then changed once (or a few times) are candidates for a
//! flag or a state number marking the event. The earliest is the best
//! early signal.

/// A byte that changed during the episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// 68000 address (`0xFF0000`-based).
    pub addr: u32,
    /// Index of the first copy where it differs from the previous one.
    pub step: usize,
    pub before: u8,
    pub after: u8,
    /// Changes from `step` to the end, the first included.
    pub changes: usize,
}

/// Work RAM base in the 68000 address space.
const RAM_BASE: u32 = 0xFF_0000;

/// The bytes constant in `snapshots[..t]` that change at `t` (with
/// `t >= late_from`) and change at most `max_changes` times in all, earliest
/// first (then by address). All copies have the same length.
pub fn late_changes<S: AsRef<[u8]>>(
    snapshots: &[S],
    late_from: usize,
    max_changes: usize,
) -> Vec<Candidate> {
    let Some(first) = snapshots.first() else {
        return Vec::new();
    };
    let size = first.as_ref().len();
    let mut found = Vec::new();
    for i in 0..size {
        let value = |t: usize| snapshots[t].as_ref()[i];
        let Some(step) = (1..snapshots.len()).find(|&t| value(t) != value(t - 1)) else {
            continue;
        };
        if step < late_from {
            continue;
        }
        let changes = (step..snapshots.len())
            .filter(|&t| value(t) != value(t - 1))
            .count();
        if changes <= max_changes {
            found.push(Candidate {
                addr: RAM_BASE + i as u32,
                step,
                before: value(step - 1),
                after: value(step),
                changes,
            });
        }
    }
    found.sort_by_key(|c| (c.step, c.addr));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `steps` copies of a 16-byte RAM, changed by `f(step, ram)`.
    fn episode(steps: usize, f: impl Fn(usize, &mut [u8])) -> Vec<Vec<u8>> {
        let mut ram = vec![0u8; 16];
        (0..steps)
            .map(|t| {
                f(t, &mut ram);
                ram.clone()
            })
            .collect()
    }

    #[test]
    fn a_flag_is_found_at_its_step_and_counters_are_left_out() {
        let snapshots = episode(10, |t, ram| {
            ram[0] = t as u8; // a counter: changes every step
            if t >= 6 {
                ram[3] = 6; // a state number set at step 6
            }
            if t >= 4 {
                ram[9] = 1; // a flag set at step 4
            }
            ram[12] = if t == 8 { 0x80 } else { 0 }; // set at 8, cleared at 9: two changes
        });
        let found = late_changes(&snapshots, 0, 2);
        let summary: Vec<(u32, usize, u8, u8, usize)> = found
            .iter()
            .map(|c| (c.addr, c.step, c.before, c.after, c.changes))
            .collect();
        assert_eq!(
            summary,
            [
                (0xFF_0009, 4, 0, 1, 1),
                (0xFF_0003, 6, 0, 6, 1),
                (0xFF_000C, 8, 0, 0x80, 2),
            ],
            "earliest first; the counter is not there"
        );
        assert_eq!(
            late_changes(&snapshots, 0, 1).len(),
            2,
            "max_changes 1 drops the pulse"
        );
        let late = late_changes(&snapshots, 5, 2);
        assert!(
            late.iter().all(|c| c.step >= 5),
            "changes before late_from ignored"
        );
        assert_eq!(late.len(), 2);
    }

    #[test]
    fn empty_or_still_episodes_give_nothing() {
        assert!(late_changes::<Vec<u8>>(&[], 0, 2).is_empty());
        assert!(late_changes(&episode(5, |_, _| {}), 0, 2).is_empty());
    }
}

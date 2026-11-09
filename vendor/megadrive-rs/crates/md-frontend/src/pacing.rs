//! Frame pacing, independent of SDL.
//!
//! Frames follow the wall clock at the console's rate (59.92 fps NTSC,
//! 49.70 fps PAL): an even 16.69 ms between frames. With audio, the queue
//! only corrects the slow drift between that clock and the sound card's:
//! pacing on the queue itself made frames come in bursts, because SDL drains
//! it a whole device period (20-40 ms) at a time (docs/decisions.md).

use std::time::Duration;

use md_core::audio::{MASTER_CLOCK_NTSC, MASTER_CLOCK_PAL, OUTPUT_RATE};

/// Lines of 3420 master cycles per frame: 262 NTSC (CLAUDE.md, Timing),
/// 313 PAL (vdp LINES_PAL).
const MASTER_CYCLES_PER_LINE: u64 = 3420;
const LINES_NTSC: u64 = 262;
const LINES_PAL: u64 = 313;

/// (master cycles per frame, master clock) of an NTSC or PAL console.
fn frame_cycles(pal: bool) -> (u64, u64) {
    if pal {
        (LINES_PAL * MASTER_CYCLES_PER_LINE, MASTER_CLOCK_PAL as u64)
    } else {
        (
            LINES_NTSC * MASTER_CYCLES_PER_LINE,
            MASTER_CLOCK_NTSC as u64,
        )
    }
}

/// Duration of a frame: ≈ 16.688 ms NTSC, ≈ 20.120 ms PAL.
pub fn frame_duration(pal: bool) -> Duration {
    let (cycles, clock) = frame_cycles(pal);
    Duration::from_nanos(cycles * 1_000_000_000 / clock)
}

/// Stereo sample pairs in one frame (≈ 736 NTSC, ≈ 887 PAL).
pub fn pairs_per_frame(pal: bool) -> u32 {
    let (cycles, clock) = frame_cycles(pal);
    (OUTPUT_RATE as u64 * cycles / clock) as u32
}

/// Frame rate of the console: ≈ 59.92 Hz NTSC, ≈ 49.70 Hz PAL.
pub fn console_rate(pal: bool) -> f64 {
    1.0 / frame_duration(pal).as_secs_f64()
}

/// A display refresh this close to the console rate is followed with vsync
/// (one emulated frame per refresh, the audio queue absorbing the small
/// difference); farther, frames follow the clock instead.
const VSYNC_TOLERANCE_HZ: f64 = 1.0;

pub fn display_matches(refresh_hz: i32, pal: bool) -> bool {
    (refresh_hz as f64 - console_rate(pal)).abs() <= VSYNC_TOLERANCE_HZ
}

/// `--vsync` / `--no-vsync`, or the automatic choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VsyncMode {
    Auto,
    On,
    Off,
}

/// Whether to synchronise presentation with a display of `refresh_hz`.
pub fn use_vsync(mode: VsyncMode, refresh_hz: i32, pal: bool) -> bool {
    match mode {
        VsyncMode::On => true,
        VsyncMode::Off => false,
        VsyncMode::Auto => display_matches(refresh_hz, pal),
    }
}

/// Emulated frames for one display refresh when pacing on vsync: none when
/// the sound is ahead (the display runs a little fast: the last picture is
/// shown again), two when it runs low (the display is a little slow).
pub fn frames_this_refresh(drift: Drift) -> u32 {
    match drift {
        Drift::Later => 0,
        Drift::OnTime => 1,
        Drift::Sooner => 2,
    }
}

/// Audio queue bounds, in frames of sound around the target: beyond them
/// the clock schedule is nudged by `DRIFT_NUDGE` per frame.
const QUEUE_HIGH_MARGIN: u32 = 2;
const QUEUE_LOW_MARGIN: u32 = 1;
/// Small enough to be invisible, large enough to absorb a sound card a few
/// hundred ppm off.
const DRIFT_NUDGE: Duration = Duration::from_millis(1);

/// Drift correction from the audio queue: `Later` when sound piles up (the
/// emulator runs ahead of the sound card), `Sooner` when it runs low.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    Sooner,
    OnTime,
    Later,
}

pub fn audio_drift(queued_pairs: u32, target_frames: u32, pal: bool) -> Drift {
    let frame = pairs_per_frame(pal);
    if queued_pairs > (target_frames + QUEUE_HIGH_MARGIN) * frame {
        Drift::Later
    } else if queued_pairs < target_frames.saturating_sub(QUEUE_LOW_MARGIN) * frame {
        Drift::Sooner
    } else {
        Drift::OnTime
    }
}

/// Fixed-period pacing: frame `n` is due at `n` periods after the start.
/// When too far behind (the host stalled), the schedule restarts from now
/// instead of running frames back to back to catch up.
#[derive(Debug, Clone)]
pub struct ClockPacer {
    period: Duration,
    /// Time since start at which the schedule began, and its frame number.
    origin: (Duration, u64),
    /// Drift corrections so far, in nanoseconds: positive delays the
    /// schedule, negative brings it forward (possibly before the start).
    shift_ns: i64,
}

/// Lateness beyond which the clock schedule is restarted.
const MAX_LAG_FRAMES: u32 = 5;

impl ClockPacer {
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            origin: (Duration::ZERO, 0),
            shift_ns: 0,
        }
    }

    /// Shifts the schedule by `DRIFT_NUDGE` (audio drift correction).
    pub fn nudge(&mut self, drift: Drift) {
        let step = DRIFT_NUDGE.as_nanos() as i64;
        match drift {
            Drift::Later => self.shift_ns += step,
            Drift::Sooner => self.shift_ns -= step,
            Drift::OnTime => {}
        }
    }

    /// Wait before frame `frame` when `elapsed` has passed since the start.
    pub fn wait(&mut self, elapsed: Duration, frame: u64) -> Duration {
        let scheduled = self.origin.0 + self.period * (frame - self.origin.1) as u32;
        let due_ns = (scheduled.as_nanos() as i64 + self.shift_ns).max(0);
        let due = Duration::from_nanos(due_ns as u64);
        if due >= elapsed {
            return due - elapsed;
        }
        if elapsed - due > self.period * MAX_LAG_FRAMES {
            self.origin = (elapsed, frame);
            self.shift_ns = 0;
        }
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_rates() {
        // 53 693 175 / 896 040 = 59.922 fps.
        assert_eq!(frame_duration(false).as_micros(), 16_688);
        // 44 100 / 59.922 = 735.9 pairs.
        assert_eq!(pairs_per_frame(false), 735);
    }

    #[test]
    fn pal_rates() {
        // 53 203 424 / (313 × 3420) = 49.701 fps.
        assert_eq!(frame_duration(true).as_micros(), 20_120);
        // 44 100 / 49.701 = 887.3 pairs.
        assert_eq!(pairs_per_frame(true), 887);
    }

    #[test]
    fn vsync_follows_matching_displays_only() {
        assert!(display_matches(60, false), "60 Hz display, NTSC console");
        assert!(!display_matches(75, false));
        assert!(!display_matches(144, false));
        assert!(!display_matches(60, true), "PAL on a 60 Hz display: clock");
        assert!(display_matches(50, true));
        assert!(use_vsync(VsyncMode::Auto, 60, false));
        assert!(!use_vsync(VsyncMode::Auto, 75, false));
        assert!(use_vsync(VsyncMode::On, 75, false));
        assert!(!use_vsync(VsyncMode::Off, 60, false));
        assert_eq!(frames_this_refresh(Drift::Later), 0);
        assert_eq!(frames_this_refresh(Drift::OnTime), 1);
        assert_eq!(frames_this_refresh(Drift::Sooner), 2);
    }

    #[test]
    fn audio_queue_only_corrects_drift() {
        // Target 3 frames of sound: on time from 2 to 5 frames queued.
        let frame = pairs_per_frame(false);
        assert_eq!(audio_drift(3 * frame, 3, false), Drift::OnTime);
        assert_eq!(audio_drift(2 * frame, 3, false), Drift::OnTime);
        assert_eq!(audio_drift(5 * frame, 3, false), Drift::OnTime);
        assert_eq!(
            audio_drift(5 * frame + 1, 3, false),
            Drift::Later,
            "piling up"
        );
        assert_eq!(
            audio_drift(2 * frame - 1, 3, false),
            Drift::Sooner,
            "running low"
        );
        assert_eq!(audio_drift(0, 3, false), Drift::Sooner);
    }

    #[test]
    fn frames_are_evenly_spaced_and_nudged() {
        // The clock gives every frame the same period, whatever the audio
        // queue does (the queue drains in device-period steps).
        let period = frame_duration(false);
        let mut p = ClockPacer::new(period);
        for frame in 1..100u64 {
            let due = period * frame as u32;
            assert_eq!(
                p.wait(due - Duration::from_micros(500), frame).as_micros(),
                500
            );
        }
        // A nudge moves the whole schedule by 1 ms.
        p.nudge(Drift::Later);
        let due = period * 100;
        assert_eq!(p.wait(due, 100), DRIFT_NUDGE);
        p.nudge(Drift::Sooner);
        p.nudge(Drift::Sooner);
        assert_eq!(
            p.wait(due - DRIFT_NUDGE, 100),
            Duration::ZERO,
            "1 ms early now"
        );
        // Bringing frames forward works from the very start too (empty
        // audio queue at launch).
        let mut p = ClockPacer::new(period);
        p.nudge(Drift::Sooner);
        assert_eq!(p.wait(Duration::ZERO, 1), period - DRIFT_NUDGE);
    }

    #[test]
    fn clock_pacer_schedule_and_resync() {
        let period = Duration::from_millis(10);
        let mut p = ClockPacer::new(period);
        assert_eq!(p.wait(Duration::ZERO, 0), Duration::ZERO);
        assert_eq!(
            p.wait(Duration::from_millis(4), 1),
            Duration::from_millis(6)
        );
        // Slightly late: no wait, schedule kept.
        assert_eq!(p.wait(Duration::from_millis(23), 2), Duration::ZERO);
        assert_eq!(
            p.wait(Duration::from_millis(25), 3),
            Duration::from_millis(5)
        );
        // Stalled far beyond the schedule: restart from now.
        assert_eq!(p.wait(Duration::from_millis(200), 4), Duration::ZERO);
        assert_eq!(
            p.wait(Duration::from_millis(203), 5),
            Duration::from_millis(7)
        );
    }
}

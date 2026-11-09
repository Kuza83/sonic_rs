//! Without synthesis, the chip must look the same to the CPU: the status
//! register (busy flag, timer flags, held value on the other ports) read at
//! every cycle matches the full chip's, whatever is written.

use ym2612::Ym2612;

/// xorshift64, deterministic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A register and value, weighted towards what drives the status: the
/// timers ($24-$27, with loads, enables, resets and the channel 3 / CSM
/// mode), the fast-timer test bit ($21 bit 2), key-on ($28, for CSM), and
/// anything else of the FM part.
fn random_write(rng: &mut Rng) -> (u8, u8, u8) {
    let data = rng.next() as u8;
    let (part, reg, data) = match rng.below(10) {
        0 => (0, 0x21, data & 0x04),
        1 => (0, 0x24, data),
        2 => (0, 0x25, data & 0x03),
        3 => (0, 0x26, data),
        4 | 5 => (0, 0x27, data),
        6 => (0, 0x28, data),
        7 => (0, 0x2A + (rng.below(2) as u8), data),
        _ => (rng.below(2) as u8, 0x30 + rng.below(0x87) as u8, data),
    };
    (part, reg, data)
}

#[test]
fn status_is_the_same_without_synthesis() {
    let mut full = Ym2612::new();
    let mut silent = Ym2612::new();
    silent.set_synthesis(false);
    assert!(full.synthesis() && !silent.synthesis());
    let mut rng = Rng(0x2612_0000_ABCD_0001);
    let (mut busy, mut flag_a, mut flag_b, mut sound) = (0u32, 0u32, 0u32, 0u32);
    let mut next_write = 0u32;
    for cycle in 0..400_000u32 {
        if cycle == next_write {
            let (part, reg, data) = random_write(&mut rng);
            for chip in [&mut full, &mut silent] {
                chip.write(part * 2, reg);
                chip.write(part * 2 + 1, data);
            }
            // Sometimes too close for the busy flag to clear, sometimes not.
            next_write = cycle + 1 + rng.below(96) as u32;
        }
        let (fl, fr) = full.clock();
        let silence = silent.clock();
        assert_eq!(silence, (0, 0), "no output without synthesis");
        sound += (fl != 0 || fr != 0) as u32;
        let port = if rng.below(4) == 0 {
            1 + rng.below(3) as u8
        } else {
            0
        };
        let (a, b) = (full.read(port), silent.read(port));
        assert_eq!(a, b, "status of port {port} at cycle {cycle}");
        busy += (a & 0x80 != 0) as u32;
        flag_a += (a & 0x01 != 0) as u32;
        flag_b += (a & 0x02 != 0) as u32;
    }
    // The run went through what it is meant to check.
    assert!(busy > 1000, "busy seen {busy} times");
    assert!(flag_a > 1000, "timer A flag seen {flag_a} times");
    assert!(flag_b > 1000, "timer B flag seen {flag_b} times");
    assert!(sound > 0, "the full chip made sound");
}

#[test]
fn reset_keeps_a_fresh_chip_synthesising() {
    assert!(Ym2612::new().synthesis(), "sound on by default");
}

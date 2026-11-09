//! Audio measurements robust to resampling and gain differences between
//! emulators: pitch (Goertzel), onset, normalised RMS envelope, left/right
//! balance. Used to compare our audio with Genesis Plus GX (sample-exact
//! equality is impossible: different resamplers and mixing gains).

#![allow(dead_code)] // each test binary uses a subset

use std::path::Path;

/// A 16-bit PCM WAV file, channels as floats in [-1, 1].
pub struct Wav {
    pub rate: u32,
    pub left: Vec<f64>,
    pub right: Vec<f64>,
}

impl Wav {
    /// Reads a 16-bit PCM WAV (mono or stereo), walking the RIFF chunks.
    pub fn read(path: &Path) -> Wav {
        let data = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE",
            "{}: not a WAV",
            path.display()
        );
        let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let (mut channels, mut rate, mut pos) = (0, 0, 12);
        while pos + 8 <= data.len() {
            let (id, len) = (&data[pos..pos + 4], u32_at(pos + 4) as usize);
            let body = pos + 8;
            if id == b"fmt " {
                assert_eq!(u16_at(body), 1, "PCM only");
                channels = u16_at(body + 2) as usize;
                rate = u32_at(body + 4);
                assert_eq!(u16_at(body + 14), 16, "16-bit only");
            } else if id == b"data" {
                assert!(channels == 1 || channels == 2, "mono or stereo");
                let samples: Vec<f64> = data[body..(body + len).min(data.len())]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&b| i16::from_le_bytes(b) as f64 / 32768.0)
                    .collect();
                let channel =
                    |c: usize| samples.iter().skip(c).step_by(channels).copied().collect();
                let left: Vec<f64> = channel(0);
                let right = if channels == 2 {
                    channel(1)
                } else {
                    left.clone()
                };
                return Wav { rate, left, right };
            }
            pos = body + len + (len & 1);
        }
        panic!("{}: no data chunk", path.display());
    }

    /// (left + right) / 2.
    pub fn mono(&self) -> Vec<f64> {
        self.left
            .iter()
            .zip(&self.right)
            .map(|(l, r)| (l + r) / 2.0)
            .collect()
    }

    /// Samples from `start_s` seconds, `len_s` long (clamped to the file).
    pub fn slice(signal: &[f64], rate: u32, start_s: f64, len_s: f64) -> &[f64] {
        let a = ((start_s * rate as f64) as usize).min(signal.len());
        let b = (((start_s + len_s) * rate as f64) as usize).min(signal.len());
        &signal[a..b]
    }
}

/// Power of `x` at frequency `f` (Goertzel algorithm, Hann window).
pub fn goertzel_power(x: &[f64], rate: u32, f: f64) -> f64 {
    let n = x.len() as f64;
    let coeff = 2.0 * (2.0 * std::f64::consts::PI * f / rate as f64).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1.0)).cos();
        let s = v * w + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Frequency of the strongest component of `x` between `lo` and `hi` Hz:
/// a 0.5 Hz scan, then a parabolic fit around the peak.
pub fn dominant_frequency(x: &[f64], rate: u32, lo: f64, hi: f64) -> f64 {
    const STEP: f64 = 0.5;
    let steps = ((hi - lo) / STEP) as usize;
    let powers: Vec<f64> = (0..=steps)
        .map(|i| goertzel_power(x, rate, lo + i as f64 * STEP))
        .collect();
    let best = (0..powers.len())
        .max_by(|&a, &b| powers[a].total_cmp(&powers[b]))
        .unwrap();
    let f = lo + best as f64 * STEP;
    if best == 0 || best == powers.len() - 1 {
        return f;
    }
    let (a, b, c) = (
        powers[best - 1].ln(),
        powers[best].ln(),
        powers[best + 1].ln(),
    );
    let denom = a - 2.0 * b + c;
    if denom == 0.0 {
        f
    } else {
        f + STEP * 0.5 * (a - c) / denom
    }
}

/// `dominant_frequency` over a wide range: a 5 Hz scan first, then the
/// fine search around its peak.
pub fn dominant_frequency_wide(x: &[f64], rate: u32, lo: f64, hi: f64) -> f64 {
    const COARSE: f64 = 5.0;
    let steps = ((hi - lo) / COARSE) as usize;
    let best = (0..=steps)
        .map(|i| lo + i as f64 * COARSE)
        .max_by(|&a, &b| goertzel_power(x, rate, a).total_cmp(&goertzel_power(x, rate, b)))
        .unwrap();
    dominant_frequency(
        x,
        rate,
        (best - 2.0 * COARSE).max(lo),
        (best + 2.0 * COARSE).min(hi),
    )
}

/// RMS per window of `window_ms`, divided by the largest window RMS.
pub fn envelope(x: &[f64], rate: u32, window_ms: f64) -> Vec<f64> {
    let window = ((rate as f64 * window_ms / 1000.0) as usize).max(1);
    let rms: Vec<f64> = x
        .chunks(window)
        .map(|c| (c.iter().map(|v| v * v).sum::<f64>() / c.len() as f64).sqrt())
        .collect();
    let max = rms.iter().cloned().fold(0.0, f64::max);
    if max == 0.0 {
        rms
    } else {
        rms.iter().map(|v| v / max).collect()
    }
}

/// Start (in seconds) of a sync pulse train beginning after `after_s`: the
/// first 1 ms window reaching 30 % of the loudest window of the 0.5 s that
/// follow the first sound past `after_s`.
pub fn pulse_train_start(x: &[f64], rate: u32, after_s: f64) -> Option<f64> {
    const WINDOW_MS: f64 = 1.0;
    let from = Wav::slice(x, rate, after_s, f64::MAX);
    let window = (rate as f64 * WINDOW_MS / 1000.0) as usize;
    // DC is removed per window: our output's DC level steps (the YM2612's
    // idle offset changes with the panning) must not count as sound.
    let rms: Vec<f64> = from.chunks(window).map(ac_rms).collect();
    let floor = 1e-3; // about -60 dBFS
    let first = rms.iter().position(|&v| v > floor)?;
    let span = &rms[first..(first + 500).min(rms.len())];
    let loudest = span.iter().cloned().fold(0.0, f64::max);
    let start = first + span.iter().position(|&v| v >= 0.3 * loudest)?;
    Some(after_s + start as f64 * WINDOW_MS / 1000.0)
}

/// Index of the first envelope window at or above `threshold`.
pub fn onset(envelope: &[f64], threshold: f64) -> Option<usize> {
    envelope.iter().position(|&v| v >= threshold)
}

/// Pearson correlation of the common part of `a` and `b`.
pub fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mean = |x: &[f64]| x.iter().sum::<f64>() / n as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        num += (x - ma) * (y - mb);
        da += (x - ma) * (x - ma);
        db += (y - mb) * (y - mb);
    }
    if da == 0.0 || db == 0.0 {
        if da == db { 1.0 } else { 0.0 }
    } else {
        num / (da * db).sqrt()
    }
}

/// RMS around the mean: the level without the DC offset (GPGX's blip_buf
/// output stage removes DC, we keep the YM2612's idle offset).
pub fn ac_rms(x: &[f64]) -> f64 {
    let mean = x.iter().sum::<f64>() / x.len() as f64;
    (x.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Energy of a signal (sum of squares).
pub fn energy(x: &[f64]) -> f64 {
    x.iter().map(|v| v * v).sum()
}

/// Left / (left + right) power without DC: 0.5 centred, 1 left only, 0
/// right only. DC is left out: our silent channel keeps the YM2612's idle
/// offset, which GPGX's output stage removes.
pub fn balance_of(left: &[f64], right: &[f64]) -> f64 {
    let (l, r) = (ac_rms(left).powi(2), ac_rms(right).powi(2));
    if l + r == 0.0 { 0.5 } else { l / (l + r) }
}

pub fn balance(wav: &Wav) -> f64 {
    balance_of(&wav.left, &wav.right)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    fn tone(f: f64, seconds: f64, start_s: f64, amplitude: f64) -> Vec<f64> {
        (0..(seconds * RATE as f64) as usize)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                if t < start_s {
                    0.0
                } else {
                    amplitude * (2.0 * std::f64::consts::PI * f * t).sin()
                }
            })
            .collect()
    }

    #[test]
    fn pitch_of_a_known_tone() {
        for f in [110.0, 440.4, 1234.5] {
            let x = tone(f, 1.0, 0.0, 0.5);
            let got = dominant_frequency(&x, RATE, f * 0.8, f * 1.2);
            assert!((got - f).abs() / f < 0.0005, "{f} Hz measured as {got}");
        }
    }

    #[test]
    fn pulse_train_is_found_to_the_millisecond() {
        // A short blip at 0.1 s (ignored: before `after_s`), then 1-frame
        // pulses from 0.7512 s.
        let mut x = vec![0.0; RATE as usize * 2];
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f64 / RATE as f64;
            let in_pulse =
                t >= 0.7512 && (((t - 0.7512) * 59.92) as u32).is_multiple_of(2) && t < 1.1;
            if (0.1..0.11).contains(&t) || in_pulse {
                *v = 0.4 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
            }
            // DC steps before the train (as our idle YM2612 offset).
            if (0.6..0.7).contains(&t) {
                *v += 0.05;
            }
        }
        let start = pulse_train_start(&x, RATE, 0.5).unwrap();
        assert!((start - 0.7512).abs() < 0.0015, "{start}");
    }

    #[test]
    fn wide_search_finds_the_tone() {
        let x = tone(1234.5, 0.3, 0.0, 0.5);
        let got = dominant_frequency_wide(&x, RATE, 50.0, 5000.0);
        assert!((got - 1234.5).abs() < 0.5, "{got}");
    }

    #[test]
    fn onset_and_envelope_ignore_the_gain() {
        let quiet = tone(440.0, 1.0, 0.25, 0.1);
        let loud = tone(440.0, 1.0, 0.25, 0.9);
        let (eq, el) = (envelope(&quiet, RATE, 5.0), envelope(&loud, RATE, 5.0));
        assert_eq!(onset(&eq, 0.1), Some(50), "250 ms in 5 ms windows");
        assert_eq!(onset(&eq, 0.1), onset(&el, 0.1));
        assert!(correlation(&eq, &el) > 0.9999);
        let late = tone(440.0, 1.0, 0.5, 0.9);
        assert!(
            correlation(&eq, &envelope(&late, RATE, 5.0)) < 0.7,
            "shifted onset"
        );
    }

    #[test]
    fn ac_rms_ignores_dc() {
        let x = tone(440.0, 1.0, 0.0, 0.5);
        let shifted: Vec<f64> = x.iter().map(|v| v + 0.3).collect();
        assert!((ac_rms(&x) - 0.5 / 2f64.sqrt()).abs() < 1e-3);
        assert!((ac_rms(&shifted) - ac_rms(&x)).abs() < 1e-9);
    }

    #[test]
    fn balance_of_panned_signals() {
        let x = tone(440.0, 0.2, 0.0, 0.5);
        let silent = vec![0.0; x.len()];
        let wav = |left: &Vec<f64>, right: &Vec<f64>| Wav {
            rate: RATE,
            left: left.clone(),
            right: right.clone(),
        };
        assert!((balance(&wav(&x, &x)) - 0.5).abs() < 1e-12);
        assert_eq!(balance(&wav(&x, &silent)), 1.0);
        let offset = vec![0.02; x.len()];
        assert_eq!(
            balance(&wav(&x, &offset)),
            1.0,
            "DC on the silent side ignored"
        );
        assert_eq!(balance(&wav(&silent, &x)), 0.0);
    }

    #[test]
    fn reads_what_md_frontend_writes() {
        // Header layout of md-frontend's headless::write_wav (16-bit stereo).
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("analysis.wav");
        let samples: [i16; 4] = [16384, -16384, 0, 32767];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + 8u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 2] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [RATE, RATE * 4] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [4u16, 16] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let wav = Wav::read(&path);
        assert_eq!(wav.rate, RATE);
        assert_eq!(wav.left, [0.5, 0.0]);
        assert_eq!(wav.right, [-0.5, 32767.0 / 32768.0]);
    }
}

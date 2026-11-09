//! Comparison against the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh).
//! Ignored by default (needs the oracle); `scripts/check.sh` runs them when
//! the oracle is built:
//!   cargo test -p ym2612 --test oracle -- --ignored

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use ym2612::{CYCLES_PER_SAMPLE, Ym2612};

/// Spacing between register writes (busy lasts 32 cycles).
const WRITE_SPACING: u64 = 48;

#[derive(Clone, Copy)]
enum Cmd {
    Write { clock: u64, port: u8, data: u8 },
    Read { clock: u64, port: u8 },
}

#[derive(Default)]
struct Session {
    cmds: Vec<Cmd>,
    /// Next free clock for a register write.
    at: u64,
}

impl Session {
    /// Register write on part 0 or 1: address then data on the next clock.
    fn reg(&mut self, part: u8, address: u8, data: u8) -> &mut Self {
        let base = 2 * part;
        self.cmds.push(Cmd::Write {
            clock: self.at,
            port: base,
            data: address,
        });
        self.cmds.push(Cmd::Write {
            clock: self.at + 1,
            port: base + 1,
            data,
        });
        self.at += WRITE_SPACING;
        self
    }

    fn read(&mut self, clock: u64) -> &mut Self {
        self.cmds.push(Cmd::Read { clock, port: 0 });
        self
    }
}

#[derive(Debug, PartialEq)]
struct Output {
    samples: Vec<(i64, i64)>,
    reads: Vec<(u64, u8)>,
}

fn oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/build/ym_oracle")
}

fn sorted(cmds: &[Cmd]) -> Vec<Cmd> {
    let mut v = cmds.to_vec();
    v.sort_by_key(|c| match *c {
        Cmd::Write { clock, .. } | Cmd::Read { clock, .. } => clock,
    });
    v
}

fn run_oracle(session: &Session, clocks: u64) -> Output {
    let path = oracle_path();
    assert!(
        path.exists(),
        "{} missing — run ./scripts/fetch-nuked-opn2.sh",
        path.display()
    );
    let mut input = String::new();
    for c in sorted(&session.cmds) {
        match c {
            Cmd::Write { clock, port, data } => writeln!(input, "w {clock} {port} {data}"),
            Cmd::Read { clock, port } => writeln!(input, "r {clock} {port}"),
        }
        .unwrap();
    }
    writeln!(input, "run {clocks}").unwrap();
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let mut result = Output {
        samples: Vec::new(),
        reads: Vec::new(),
    };
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f[0] {
            "s" => result
                .samples
                .push((f[1].parse().unwrap(), f[2].parse().unwrap())),
            _ => result
                .reads
                .push((f[1].parse().unwrap(), f[2].parse().unwrap())),
        }
    }
    result
}

fn run_model(session: &Session, clocks: u64) -> Output {
    let cmds = sorted(&session.cmds);
    let mut chip = Ym2612::new();
    let mut next = 0;
    let mut result = Output {
        samples: Vec::new(),
        reads: Vec::new(),
    };
    let (mut l, mut r) = (0i64, 0i64);
    for clock in 0..clocks {
        while let Some(&c) = cmds.get(next) {
            match c {
                Cmd::Write {
                    clock: at,
                    port,
                    data,
                } if at == clock => chip.write(port, data),
                Cmd::Read { clock: at, port } if at == clock => {
                    result.reads.push((clock, chip.read(port)))
                }
                _ => break,
            }
            next += 1;
        }
        let (a, b) = chip.clock();
        l += a as i64;
        r += b as i64;
        if (clock + 1) % CYCLES_PER_SAMPLE as u64 == 0 {
            result.samples.push((l, r));
            (l, r) = (0, 0);
        }
    }
    result
}

/// Runs both and reports the first differing sample with context.
fn assert_matches_oracle(session: &Session, clocks: u64) {
    let want = run_oracle(session, clocks);
    let got = run_model(session, clocks);
    assert_eq!(got.reads, want.reads, "status reads");
    if let Some(i) = (0..want.samples.len()).find(|&i| got.samples.get(i) != Some(&want.samples[i]))
    {
        let lo = i.saturating_sub(2);
        let hi = (i + 3).min(want.samples.len());
        panic!(
            "first difference at sample {i}:\n  model  {:?}\n  oracle {:?}",
            &got.samples[lo..hi.min(got.samples.len())],
            &want.samples[lo..hi]
        );
    }
    assert_eq!(got.samples.len(), want.samples.len());
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn idle_output() {
    assert_matches_oracle(&Session::default(), 24 * 50);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn busy_flag_after_a_data_write() {
    let mut s = Session::default();
    s.reg(0, 0x30, 0x01);
    for clock in 0..60 {
        s.read(clock);
    }
    assert_matches_oracle(&s, 24 * 4);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn timers_a_and_b_overflow() {
    let mut s = Session::default();
    s.reg(0, 0x24, 0xFF).reg(0, 0x25, 0x01); // timer A = 1021: 3 ticks
    s.reg(0, 0x26, 0xFE); // timer B = 254: 2 ticks of 16 samples
    s.reg(0, 0x27, 0x0F); // load and enable both
    for clock in (s.at..s.at + 24 * 80).step_by(7) {
        s.read(clock);
    }
    let end = s.at + 24 * 80;
    s.reg(0, 0x27, 0x30); // reset both flags
    s.at = end;
    s.read(end + 24 * 4);
    assert_matches_oracle(&s, end + 24 * 8);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn dac_channel6() {
    let mut s = Session::default();
    s.reg(0, 0x2B, 0x80); // DAC on
    for (i, v) in [0x00u8, 0x40, 0x80, 0xC0, 0xFF, 0x7F]
        .into_iter()
        .enumerate()
    {
        s.reg(0, 0x2A, v);
        s.at += 24 * (i as u64 + 1);
    }
    s.reg(1, 0xB6, 0x80); // channel 6 left only
    s.reg(0, 0x2A, 0x10);
    assert_matches_oracle(&s, s.at + 24 * 10);
}

/// Register offset of operator `op` (1-4) of channel `ch` (0-2 within a part).
fn op_reg(base: u8, ch: u8, op: u8) -> u8 {
    // Register order is OP1, OP3, OP2, OP4.
    let index = [0u8, 8, 4, 12][(op - 1) as usize];
    base + index + ch
}

/// Channel `ch` of `part`: one operator `op` audible (TL 0, instant attack),
/// the others silent; algorithm `alg`, F-number `fnum`, block `block`.
fn single_operator(s: &mut Session, part: u8, ch: u8, op: u8, alg: u8, block: u8, fnum: u16) {
    for o in 1..=4 {
        let tl = if o == op { 0x00 } else { 0x7F };
        s.reg(part, op_reg(0x30, ch, o), 0x01); // DT 0, MUL 1
        s.reg(part, op_reg(0x40, ch, o), tl);
        s.reg(part, op_reg(0x50, ch, o), 0x1F); // AR 31
        s.reg(part, op_reg(0x60, ch, o), 0x00);
        s.reg(part, op_reg(0x70, ch, o), 0x00);
        s.reg(part, op_reg(0x80, ch, o), 0x0F); // SL 0, RR 15
    }
    s.reg(part, 0xB0 + ch, alg);
    s.reg(part, 0xB4 + ch, 0xC0);
    s.reg(part, 0xA4 + ch, (block << 3) | (fnum >> 8) as u8);
    s.reg(part, 0xA0 + ch, fnum as u8);
}

fn key_on(s: &mut Session, part: u8, ch: u8, operators: u8) {
    s.reg(0, 0x28, (operators << 4) | (part << 2) | ch);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn single_sine_operator() {
    let mut s = Session::default();
    single_operator(&mut s, 0, 0, 1, 7, 4, 0x269);
    key_on(&mut s, 0, 0, 0x1);
    assert_matches_oracle(&s, s.at + 24 * 2000);
}

/// Full voice on channel `ch` of `part`: every operator audible with the
/// given envelope bytes ($50 AR/KS, $60 DR/AM, $70 SR, $80 SL/RR) and TLs.
#[allow(clippy::too_many_arguments)]
fn voice(
    s: &mut Session,
    part: u8,
    ch: u8,
    alg_fb: u8,
    dt_mul: [u8; 4],
    tl: [u8; 4],
    env: [u8; 4],
    block_fnum: (u8, u16),
) {
    for o in 1..=4u8 {
        let i = (o - 1) as usize;
        s.reg(part, op_reg(0x30, ch, o), dt_mul[i]);
        s.reg(part, op_reg(0x40, ch, o), tl[i]);
        s.reg(part, op_reg(0x50, ch, o), env[0]);
        s.reg(part, op_reg(0x60, ch, o), env[1]);
        s.reg(part, op_reg(0x70, ch, o), env[2]);
        s.reg(part, op_reg(0x80, ch, o), env[3]);
    }
    s.reg(part, 0xB0 + ch, alg_fb);
    s.reg(part, 0xB4 + ch, 0xC0);
    let (block, fnum) = block_fnum;
    s.reg(part, 0xA4 + ch, (block << 3) | (fnum >> 8) as u8);
    s.reg(part, 0xA0 + ch, fnum as u8);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn every_algorithm_with_feedback() {
    for alg in 0..8u8 {
        let mut s = Session::default();
        voice(
            &mut s,
            0,
            1,
            alg | (5 << 3),
            [0x01, 0x32, 0x13, 0x01],
            [0x20, 0x18, 0x10, 0x00],
            [0x1F, 0x05, 0x02, 0x24],
            (4, 0x2A0),
        );
        key_on(&mut s, 0, 1, 0xF);
        assert_matches_oracle(&s, s.at + 24 * 1500);
    }
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn envelope_phases_and_key_off() {
    let mut s = Session::default();
    // Slow attack, decay to SL 4, sustain rate, then release after key off.
    voice(
        &mut s,
        1,
        2,
        7,
        [0x01; 4],
        [0x00, 0x7F, 0x7F, 0x7F],
        [0x4C, 0x0A, 0x04, 0x47],
        (3, 0x400),
    );
    key_on(&mut s, 1, 2, 0x1);
    s.at += 24 * 3000;
    key_on(&mut s, 1, 2, 0x0);
    assert_matches_oracle(&s, s.at + 24 * 3000);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn key_scaling_and_detune() {
    for (ks, dt) in [(0x00u8, 0x00u8), (0x40, 0x10), (0x80, 0x30), (0xC0, 0x70)] {
        let mut s = Session::default();
        voice(
            &mut s,
            0,
            0,
            7,
            [dt | 0x01, dt | 0x02, dt, dt | 0x0F],
            [0x10, 0x18, 0x20, 0x08],
            [ks | 0x14, 0x08, 0x06, 0x35],
            (6, 0x7FF),
        );
        key_on(&mut s, 0, 0, 0xF);
        assert_matches_oracle(&s, s.at + 24 * 1500);
    }
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn ssg_eg_modes() {
    for mode in 8..16u8 {
        let mut s = Session::default();
        voice(
            &mut s,
            0,
            0,
            7,
            [0x01; 4],
            [0x00, 0x7F, 0x7F, 0x7F],
            [0x1F, 0x1A, 0x10, 0x2F],
            (4, 0x300),
        );
        s.reg(0, op_reg(0x90, 0, 1), mode);
        key_on(&mut s, 0, 0, 0x1);
        s.at += 24 * 2500;
        key_on(&mut s, 0, 0, 0x0);
        assert_matches_oracle(&s, s.at + 24 * 800);
    }
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn lfo_am_and_pm() {
    for (lfo, ams_pms) in [(0x08u8, 0x37u8), (0x0B, 0x23), (0x0F, 0x14)] {
        let mut s = Session::default();
        s.reg(0, 0x22, lfo);
        voice(
            &mut s,
            0,
            0,
            4,
            [0x01, 0x02, 0x01, 0x03],
            [0x08, 0x00, 0x10, 0x00],
            [0x1F, 0x80, 0x00, 0x0F],
            (4, 0x280),
        );
        s.reg(0, 0xB4, 0xC0 | ams_pms);
        key_on(&mut s, 0, 0, 0xF);
        assert_matches_oracle(&s, s.at + 24 * 4000);
    }
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn channel3_special_mode() {
    let mut s = Session::default();
    voice(
        &mut s,
        0,
        2,
        7,
        [0x01; 4],
        [0x10, 0x10, 0x10, 0x10],
        [0x1F, 0x00, 0x00, 0x0F],
        (4, 0x200),
    );
    s.reg(0, 0x27, 0x40); // channel 3 special mode
    for (i, fnum) in [0x180u16, 0x240, 0x300].into_iter().enumerate() {
        s.reg(0, 0xAC + i as u8, (4 << 3) | (fnum >> 8) as u8);
        s.reg(0, 0xA8 + i as u8, fnum as u8);
    }
    key_on(&mut s, 0, 2, 0xF);
    assert_matches_oracle(&s, s.at + 24 * 2000);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn csm_mode_keys_channel3_on_timer_a() {
    let mut s = Session::default();
    voice(
        &mut s,
        0,
        2,
        7,
        [0x01; 4],
        [0x08, 0x10, 0x18, 0x20],
        [0x1F, 0x0C, 0x06, 0x1F],
        (4, 0x250),
    );
    s.reg(0, 0x24, 0xF8).reg(0, 0x25, 0x00); // timer A = 992: 32 samples
    s.reg(0, 0x27, 0x85); // CSM + load and enable timer A
    assert_matches_oracle(&s, s.at + 24 * 3000);
}

#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn six_channels_with_dac_and_pan() {
    let mut s = Session::default();
    for part in 0..2u8 {
        for ch in 0..3u8 {
            voice(
                &mut s,
                part,
                ch,
                ((ch + 3 * part) % 8) | 0x10,
                [0x01, 0x02, 0x03, 0x04],
                [0x18, 0x10, 0x20, 0x08],
                [0x1C, 0x06, 0x03, 0x26],
                (3 + ch, 0x200 + 0x40 * ch as u16),
            );
            s.reg(part, 0xB4 + ch, [0x80, 0x40, 0xC0][ch as usize]);
            key_on(&mut s, part, ch, 0xF);
        }
    }
    s.reg(0, 0x2B, 0x80);
    s.reg(0, 0x2A, 0xA0);
    assert_matches_oracle(&s, s.at + 24 * 2000);
}

/// Every detune setting against every block and key-code note (F-number
/// bits 10-7 select notes 0-3), so each entry of the detune table is used.
#[test]
#[ignore = "needs the Nuked-OPN2 oracle (scripts/fetch-nuked-opn2.sh)"]
fn detune_table_coverage() {
    for dt in [1u8, 2, 3, 5, 6, 7] {
        for block in 0..8u8 {
            for fnum in [0x100u16, 0x380, 0x400, 0x600] {
                let mut s = Session::default();
                single_operator(&mut s, 0, 0, 1, 7, block, fnum);
                s.reg(0, op_reg(0x30, 0, 1), (dt << 4) | 0x01);
                key_on(&mut s, 0, 0, 0x1);
                assert_matches_oracle(&s, s.at + 24 * 300);
            }
        }
    }
}

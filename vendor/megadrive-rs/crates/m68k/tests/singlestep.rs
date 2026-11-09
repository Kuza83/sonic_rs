//! SingleStepTests/680x0 runner (68000/v1).
//!
//! Heavy (~1M tests): ignored by default, run in release:
//!   ./scripts/fetch-singlestep.sh
//!   cargo test -p m68k --release --test singlestep -- --ignored --nocapture
//! Environment:
//!   SST_FILTER=ADD.b,NOP   only these files (stem without .json.gz)
//!   SST_SHOW=5             detailed failures printed per file (default 3)

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use m68k::{ADDRESS_MASK, Bus, Cpu, disassemble, disassemble_template};
use serde::Deserialize;

/// Tests whose expected data is wrong upstream. Every entry needs a reason.
const KNOWN_BAD: &[(&str, &[u32], &str)] = &[(
    "ASL.b",
    &[1583, 1761],
    "SingleStepTests/680x0 issue #4: incorrect data for ASL.b tests 1583 and 1761",
)];

/// SR condition code bits used by expectation patches.
const SR_C: u16 = 0x0001;
const SR_X: u16 = 0x0010;

/// SR supervisor bit, to pick the active stack pointer.
const SR_S: u16 = 0x2000;
/// Exception vector address of the zero divide trap (vector 5).
const ZERO_DIVIDE_VECTOR_ADDR: u32 = 0x14;

/// Corrects expectations where this emulator deliberately differs from the
/// v1 data, so the rest of each test is still checked. Returns true when a
/// patch was applied. Every rule is documented in docs/decisions.md.
fn patch_expected(stem: &str, test: &mut Test) -> bool {
    match stem {
        "ASR.b" | "ASR.w" | "ASR.l" => patch_asr(stem, test),
        "DIVU" | "DIVS" if reads_vector(test, ZERO_DIVIDE_VECTOR_ADDR) => patch_zero_divide(test),
        "DIVS" => patch_divs_signed_overflow(test),
        _ => false,
    }
}

/// ASR with a register count larger than the operand size on a negative value:
/// the sign bit keeps being shifted out, so C and X must be set. The v1 data was
/// produced by CLK before TomHarte/CLK PR #1913 (merged 2026-10-24, "Fix 68000
/// ASR carry and extend for oversized shifts"), which cleared them.
fn patch_asr(stem: &str, test: &mut Test) -> bool {
    let bits = match stem {
        "ASR.b" => 8,
        "ASR.w" => 16,
        "ASR.l" => 32,
        _ => return false,
    };
    let op = test.initial.prefetch[0];
    let register_form = (op >> 6) & 3 != 3 && op & 0x0020 != 0;
    if !register_form {
        return false;
    }
    let d = test.initial.d();
    let count = d[((op >> 9) & 7) as usize] & 63;
    let value = d[(op & 7) as usize];
    let negative = value >> (bits - 1) & 1 != 0;
    if count > bits && negative {
        test.expected.sr |= SR_C | SR_X;
        return true;
    }
    false
}

fn reads_vector(test: &Test, addr: u32) -> bool {
    test.transactions.iter().any(|t| {
        t.as_array()
            .is_some_and(|t| t[0].as_str() == Some("r") && t[3].as_u64() == Some(addr as u64))
    })
}

/// Word of the initial memory image (program words come from the prefetch queue).
fn initial_word(state: &State, addr: u32) -> u16 {
    match addr.wrapping_sub(state.pc) {
        0 => state.prefetch[0],
        2 => state.prefetch[1],
        _ => {
            let byte = |x: u32| {
                state
                    .ram
                    .iter()
                    .find(|(r, _)| *r == x & ADDRESS_MASK)
                    .map_or(0, |(_, v)| *v)
            };
            u16::from_be_bytes([byte(addr), byte(addr.wrapping_add(1))])
        }
    }
}

/// Zero divide: the stacked PC is the address of the next instruction (M68000
/// PRM, as for TRAP/TRAPV/CHK in the same data set); the single v1 test stacks
/// the DIVU address itself. The frame's PC lives at final SSP + 2..=5.
fn patch_zero_divide(test: &mut Test) -> bool {
    let pc = test.initial.pc;
    let next = pc.wrapping_add(disassemble(pc, |a| initial_word(&test.initial, a)).len);
    let frame_pc = test.expected.ssp.wrapping_add(2);
    let bytes = next.to_be_bytes();
    let mut patched = false;
    for (addr, value) in test.expected.ram.iter_mut() {
        let offset = addr.wrapping_sub(frame_pc);
        if offset < 4 && *value != bytes[offset as usize] {
            *value = bytes[offset as usize];
            patched = true;
        }
    }
    patched
}

/// DIVS whose quotient overflows 16 bits without an absolute overflow: the
/// division runs to completion (Jorge Cwik's timing, as in Genesis Plus GX),
/// whereas the v1 data exits as early as an absolute overflow.
fn patch_divs_signed_overflow(test: &mut Test) -> bool {
    let Some(divisor) = divs_source(&test.initial) else {
        return false;
    };
    let dividend = test.initial.d()[((test.initial.prefetch[0] >> 9) & 7) as usize] as i32;
    let divisor = divisor as i16;
    if divisor == 0 || (dividend.unsigned_abs() >> 16) >= divisor.unsigned_abs() as u32 {
        return false;
    }
    let quotient = dividend as i64 / divisor as i64;
    if quotient == quotient as i16 as i64 {
        return false;
    }
    let early_exit = (if dividend < 0 { 7 } else { 6 } + 2) * 2;
    test.length += m68k::timing::divs_cycles(dividend, divisor) - early_exit;
    true
}

/// Source operand of a DIVS, computed independently from the initial state.
/// `None` when the access would fault (odd address).
fn divs_source(s: &State) -> Option<u16> {
    let op = s.prefetch[0];
    let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
    let ext = s.prefetch[1];
    let d = s.d();
    let mut a = [0u32; 8];
    a[..7].copy_from_slice(&s.a());
    a[7] = if s.sr & SR_S != 0 { s.ssp } else { s.usp };
    let sext16 = |v: u16| v as i16 as i32 as u32;
    let index = |base: u32| {
        let r = ((ext >> 12) & 7) as usize;
        let v = if ext & 0x8000 != 0 { a[r] } else { d[r] };
        let v = if ext & 0x0800 != 0 {
            v
        } else {
            sext16(v as u16)
        };
        base.wrapping_add(v).wrapping_add(ext as i8 as i32 as u32)
    };
    let addr = match (mode, reg) {
        (0, _) => return Some(d[reg] as u16),
        (2 | 3, _) => a[reg],
        (4, _) => a[reg].wrapping_sub(2),
        (5, _) => a[reg].wrapping_add(sext16(ext)),
        (6, _) => index(a[reg]),
        (7, 0) => sext16(ext),
        (7, 1) => ((ext as u32) << 16) | initial_word(s, s.pc.wrapping_add(4)) as u32,
        (7, 2) => s.pc.wrapping_add(2).wrapping_add(sext16(ext)),
        (7, 3) => index(s.pc.wrapping_add(2)),
        (7, 4) => return Some(ext),
        _ => return None,
    };
    (addr & 1 == 0).then(|| initial_word(s, addr))
}

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    length: u32,
    transactions: Vec<serde_json::Value>,
}

#[derive(Deserialize, Clone)]
struct State {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
    d4: u32,
    d5: u32,
    d6: u32,
    d7: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    usp: u32,
    ssp: u32,
    sr: u16,
    pc: u32,
    prefetch: [u16; 2],
    ram: Vec<(u32, u8)>,
}

impl State {
    fn d(&self) -> [u32; 8] {
        [
            self.d0, self.d1, self.d2, self.d3, self.d4, self.d5, self.d6, self.d7,
        ]
    }

    fn a(&self) -> [u32; 7] {
        [
            self.a0, self.a1, self.a2, self.a3, self.a4, self.a5, self.a6,
        ]
    }
}

/// Sparse memory: allocating 16 MiB per test would dominate the run time.
/// Logs every access in the SingleStepTests transaction notation.
#[derive(Default)]
struct SparseBus {
    mem: HashMap<u32, u8>,
    log: Vec<String>,
}

impl SparseBus {
    fn peek(&self, addr: u32) -> u8 {
        self.mem.get(&(addr & ADDRESS_MASK)).copied().unwrap_or(0)
    }

    fn poke(&mut self, addr: u32, value: u8) {
        self.mem.insert(addr & ADDRESS_MASK, value);
    }
}

fn fmt_transaction(kind: &str, addr: u32, size: &str, value: u32) -> String {
    format!("{kind} {:06X} {size} {value:04X}", addr & ADDRESS_MASK)
}

impl Bus for SparseBus {
    fn read8(&mut self, addr: u32) -> u8 {
        let v = self.peek(addr);
        self.log.push(fmt_transaction("r", addr, ".b", v as u32));
        v
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let v = u16::from_be_bytes([self.peek(addr), self.peek(addr.wrapping_add(1))]);
        self.log.push(fmt_transaction("r", addr, ".w", v as u32));
        v
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.log
            .push(fmt_transaction("w", addr, ".b", value as u32));
        self.poke(addr, value);
    }

    fn write16(&mut self, addr: u32, value: u16) {
        self.log
            .push(fmt_transaction("w", addr, ".w", value as u32));
        let [hi, lo] = value.to_be_bytes();
        self.poke(addr, hi);
        self.poke(addr.wrapping_add(1), lo);
    }
}

/// Expected transactions, idle cycles dropped (the CPU does not report them).
fn expected_transactions(test: &Test) -> Vec<String> {
    test.transactions
        .iter()
        .filter_map(|t| {
            let t = t.as_array()?;
            let kind = t[0].as_str()?;
            if kind == "n" {
                return None;
            }
            let kind = if kind == "t" { "w" } else { kind };
            Some(fmt_transaction(
                kind,
                t[3].as_u64()? as u32,
                t[4].as_str()?,
                t[5].as_u64()? as u32,
            ))
        })
        .collect()
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/singlestep/680x0/68000/v1")
}

fn load_cpu(state: &State) -> Cpu {
    let mut cpu = Cpu::new();
    let d = state.d();
    let a = state.a();
    cpu.regs.d = d;
    cpu.regs.a[..7].copy_from_slice(&a);
    cpu.regs.set_sr(state.sr);
    cpu.regs.set_usp(state.usp);
    cpu.regs.set_ssp(state.ssp);
    cpu.regs.pc = state.pc;
    cpu.prefetch = state.prefetch;
    cpu
}

/// Returns the list of mismatches (empty = pass).
fn diff_state(cpu: &Cpu, bus: &SparseBus, want: &State) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |what: String, got: u32, exp: u32| {
        if got != exp {
            out.push(format!("{what}: got {got:#010X}, want {exp:#010X}"));
        }
    };
    for (i, (&g, e)) in cpu.regs.d.iter().zip(want.d()).enumerate() {
        check(format!("d{i}"), g, e);
    }
    for (i, (&g, e)) in cpu.regs.a.iter().zip(want.a()).enumerate() {
        check(format!("a{i}"), g, e);
    }
    check("usp".into(), cpu.regs.usp(), want.usp);
    check("ssp".into(), cpu.regs.ssp(), want.ssp);
    check("sr".into(), cpu.regs.sr() as u32, want.sr as u32);
    check("pc".into(), cpu.regs.pc, want.pc);
    check(
        "prefetch0".into(),
        cpu.prefetch[0] as u32,
        want.prefetch[0] as u32,
    );
    check(
        "prefetch1".into(),
        cpu.prefetch[1] as u32,
        want.prefetch[1] as u32,
    );
    for &(addr, value) in &want.ram {
        check(
            format!("ram[{addr:#08X}]"),
            bus.peek(addr) as u32,
            value as u32,
        );
    }
    out
}

/// "e3ae [LSL.l D1, D6] 5" -> ("LSL.l D1, D6", 5)
fn split_name(name: &str) -> (&str, u32) {
    let open = name.find('[').unwrap();
    let close = name.rfind(']').unwrap();
    let index = name[close + 1..].trim().parse().unwrap();
    (&name[open + 1..close], index)
}

#[derive(Default)]
struct FileReport {
    total: usize,
    state_ok: usize,
    cycles_ok: usize,
    template_ok: usize,
    known_bad: usize,
    patched: usize,
}

fn run_file(path: &Path, show: usize) -> FileReport {
    let stem = path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_end_matches(".json.gz");
    let mut tests: Vec<Test> =
        serde_json::from_reader(BufReader::new(GzDecoder::new(File::open(path).unwrap())))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let bad: &[u32] = KNOWN_BAD
        .iter()
        .find(|(f, _, _)| *f == stem)
        .map_or(&[], |(_, ids, _)| ids);

    let mut report = FileReport::default();
    let mut shown = 0;
    for test in &mut tests {
        let (_, index) = split_name(&test.name);
        if bad.contains(&index) {
            report.known_bad += 1;
            continue;
        }
        report.patched += patch_expected(stem, test) as usize;
        let test = &*test;
        let (template, _) = split_name(&test.name);
        report.total += 1;

        let mut bus = SparseBus::default();
        for &(addr, value) in &test.initial.ram {
            bus.poke(addr, value);
        }
        let mut cpu = load_cpu(&test.initial);
        let cycles = cpu.step(&mut bus);

        let diffs = diff_state(&cpu, &bus, &test.expected);
        let state_ok = diffs.is_empty();
        let cycles_ok = cycles == test.length;
        let got_template = disassemble_template(test.initial.prefetch[0]);
        let template_ok = got_template.as_deref() == Some(template);
        report.state_ok += state_ok as usize;
        report.cycles_ok += cycles_ok as usize;
        report.template_ok += template_ok as usize;

        if (!state_ok || !cycles_ok || !template_ok) && shown < show {
            shown += 1;
            let dis = disassemble(test.initial.pc, |addr| initial_word(&test.initial, addr));
            println!("  FAIL {} — {}", test.name, dis.text);
            if !template_ok {
                println!("    template: got {got_template:?}, want {template:?}");
            }
            if !cycles_ok {
                println!("    cycles: got {cycles}, want {}", test.length);
            }
            for d in diffs {
                println!("    {d}");
            }
            let want = expected_transactions(test);
            if want != bus.log {
                println!("    bus (got | want):");
                for i in 0..want.len().max(bus.log.len()) {
                    let g = bus.log.get(i).map_or("", String::as_str);
                    let w = want.get(i).map_or("", String::as_str);
                    let mark = if g == w { ' ' } else { '*' };
                    println!("     {mark} {g:<22} | {w}");
                }
            }
        }
    }
    report
}

#[test]
#[ignore = "heavy: needs downloaded data, run with --release -- --ignored"]
fn singlestep_68000() {
    let dir = data_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e} — run ./scripts/fetch-singlestep.sh first",
                dir.display()
            )
        })
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".json.gz"))
        .collect();
    files.sort();

    let filter: Option<Vec<String>> = std::env::var("SST_FILTER")
        .ok()
        .map(|f| f.split(',').map(|s| s.trim().to_string()).collect());
    if let Some(filter) = &filter {
        files.retain(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            filter.iter().any(|f| name == format!("{f}.json.gz"))
        });
        assert_eq!(files.len(), filter.len(), "SST_FILTER names a missing file");
    }
    assert!(!files.is_empty(), "no test files in {}", dir.display());
    let show = std::env::var("SST_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    let mut all_pass = true;
    println!(
        "{:<14} {:>15} {:>15} {:>15}",
        "file", "state", "cycles", "disasm"
    );
    for path in &files {
        let r = run_file(path, show);
        let stem = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".json.gz", "");
        let pass = r.state_ok == r.total && r.cycles_ok == r.total && r.template_ok == r.total;
        all_pass &= pass;
        let mut bad = String::new();
        if r.known_bad > 0 {
            bad += &format!("  ({} known-bad skipped)", r.known_bad);
        }
        if r.patched > 0 {
            bad += &format!(
                "  ({} expectations patched, see docs/decisions.md)",
                r.patched
            );
        }
        println!(
            "{:<14} {:>7}/{:<7} {:>7}/{:<7} {:>7}/{:<7} {}{bad}",
            stem,
            r.state_ok,
            r.total,
            r.cycles_ok,
            r.total,
            r.template_ok,
            r.total,
            if pass { "ok" } else { "FAIL" }
        );
    }
    assert!(all_pass, "some SingleStepTests failed");
}

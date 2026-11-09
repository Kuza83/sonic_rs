//! SingleStepTests/z80 runner (v1).
//!
//! Heavy (1604 files × 1000 tests): ignored by default, run in release:
//!   ./scripts/fetch-singlestep-z80.sh
//!   cargo test -p z80 --release --test singlestep -- --ignored --nocapture
//! Environment:
//!   SST_FILTER=00,cb 00    only these files (stem without .json.gz)
//!   SST_PREFIX=ed          only files whose stem starts with this
//!   SST_SHOW=5             detailed failures printed per file (default 3)

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde::Deserialize;
use z80::{Bus, Cpu, Registers, disassemble};

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    expected: State,
    cycles: Vec<serde_json::Value>,
    #[serde(default)]
    ports: Vec<(u16, u8, String)>,
}

#[derive(Deserialize, Clone)]
struct State {
    pc: u16,
    sp: u16,
    a: u8,
    b: u8,
    c: u8,
    d: u8,
    e: u8,
    f: u8,
    h: u8,
    l: u8,
    i: u8,
    r: u8,
    ei: u8,
    wz: u16,
    ix: u16,
    iy: u16,
    af_: u16,
    bc_: u16,
    de_: u16,
    hl_: u16,
    im: u8,
    p: u8,
    q: u8,
    iff1: u8,
    iff2: u8,
    ram: Vec<(u16, u8)>,
}

impl State {
    fn registers(&self) -> Registers {
        Registers {
            a: self.a,
            f: self.f,
            b: self.b,
            c: self.c,
            d: self.d,
            e: self.e,
            h: self.h,
            l: self.l,
            af_alt: self.af_,
            bc_alt: self.bc_,
            de_alt: self.de_,
            hl_alt: self.hl_,
            ix: self.ix,
            iy: self.iy,
            sp: self.sp,
            pc: self.pc,
            i: self.i,
            r: self.r,
            wz: self.wz,
            iff1: self.iff1 != 0,
            iff2: self.iff2 != 0,
            im: self.im,
            q: self.q,
            p: self.p != 0,
            ei: self.ei != 0,
        }
    }
}

/// Memory from the test, ports answering the scripted reads, writes logged.
#[derive(Default)]
struct TestBus {
    mem: HashMap<u16, u8>,
    port_reads: VecDeque<(u16, u8)>,
    port_writes: Vec<(u16, u8)>,
}

impl Bus for TestBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem.get(&addr).copied().unwrap_or(0)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.mem.insert(addr, value);
    }

    fn port_in(&mut self, port: u16) -> u8 {
        match self.port_reads.pop_front() {
            Some((p, v)) if p == port => v,
            Some((p, v)) => panic!("IN from {port:#06X}, test expected {p:#06X} = {v:#04X}"),
            None => panic!("unexpected IN from {port:#06X}"),
        }
    }

    fn port_out(&mut self, port: u16, value: u8) {
        self.port_writes.push((port, value));
    }
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/singlestep/z80/v1")
}

fn diff(got: &Registers, want: &Registers, bus: &TestBus, test: &Test) -> Vec<String> {
    let mut out = Vec::new();
    macro_rules! check {
        ($($field:ident),*) => {$(
            if got.$field != want.$field {
                out.push(format!("{}: got {:?}, want {:?}", stringify!($field), got.$field, want.$field));
            }
        )*};
    }
    check!(
        a, f, b, c, d, e, h, l, af_alt, bc_alt, de_alt, hl_alt, ix, iy, sp, pc, i, r, wz
    );
    check!(iff1, iff2, im, q, p, ei);
    for &(addr, value) in &test.expected.ram {
        let got = bus.mem.get(&addr).copied().unwrap_or(0);
        if got != value {
            out.push(format!(
                "ram[{addr:#06X}]: got {got:#04X}, want {value:#04X}"
            ));
        }
    }
    let want_writes: Vec<(u16, u8)> = test
        .ports
        .iter()
        .filter(|(_, _, dir)| dir == "w")
        .map(|&(p, v, _)| (p, v))
        .collect();
    if bus.port_writes != want_writes {
        out.push(format!(
            "port writes: got {:?}, want {:?}",
            bus.port_writes, want_writes
        ));
    }
    out
}

#[derive(Default)]
struct FileReport {
    total: usize,
    state_ok: usize,
    cycles_ok: usize,
}

fn run_file(path: &Path, show: usize) -> FileReport {
    let tests: Vec<Test> =
        serde_json::from_reader(BufReader::new(GzDecoder::new(File::open(path).unwrap())))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut report = FileReport::default();
    let mut shown = 0;
    for test in &tests {
        report.total += 1;
        let mut bus = TestBus::default();
        for &(addr, value) in &test.initial.ram {
            bus.mem.insert(addr, value);
        }
        bus.port_reads = test
            .ports
            .iter()
            .filter(|(_, _, dir)| dir == "r")
            .map(|&(p, v, _)| (p, v))
            .collect();
        let mut cpu = Cpu::new();
        cpu.regs = test.initial.registers();
        let tstates = cpu.step(&mut bus);

        let diffs = diff(&cpu.regs, &test.expected.registers(), &bus, test);
        let state_ok = diffs.is_empty();
        let cycles_ok = tstates as usize == test.cycles.len();
        report.state_ok += state_ok as usize;
        report.cycles_ok += cycles_ok as usize;
        if (!state_ok || !cycles_ok) && shown < show {
            shown += 1;
            let ram: HashMap<u16, u8> = test.initial.ram.iter().copied().collect();
            let dis = disassemble(test.initial.pc, |a| ram.get(&a).copied().unwrap_or(0));
            println!("  FAIL {} — {}", test.name, dis.text);
            if !cycles_ok {
                println!("    T-states: got {tstates}, want {}", test.cycles.len());
            }
            for d in diffs {
                println!("    {d}");
            }
        }
    }
    report
}

#[test]
#[ignore = "heavy: needs downloaded data, run with --release -- --ignored"]
fn singlestep_z80() {
    let dir = data_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e} — run ./scripts/fetch-singlestep-z80.sh first",
                dir.display()
            )
        })
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".json.gz"))
        .collect();
    files.sort();
    let stem = |p: &PathBuf| {
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".json.gz")
            .to_string()
    };
    if let Ok(filter) = std::env::var("SST_FILTER") {
        let wanted: Vec<String> = filter.split(',').map(|s| s.trim().to_string()).collect();
        files.retain(|p| wanted.contains(&stem(p)));
        assert_eq!(files.len(), wanted.len(), "SST_FILTER names a missing file");
    }
    if let Ok(prefix) = std::env::var("SST_PREFIX") {
        files.retain(|p| stem(p).starts_with(&prefix));
    }
    assert!(!files.is_empty(), "no test files in {}", dir.display());
    let show = std::env::var("SST_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    let (mut files_ok, mut all_pass) = (0, true);
    for path in &files {
        let r = run_file(path, show);
        let pass = r.state_ok == r.total && r.cycles_ok == r.total;
        all_pass &= pass;
        files_ok += pass as usize;
        if !pass || show > 0 {
            println!(
                "{:<12} {:>5}/{:<5} {:>5}/{:<5} {}",
                stem(path),
                r.state_ok,
                r.total,
                r.cycles_ok,
                r.total,
                if pass { "ok" } else { "FAIL" }
            );
        }
    }
    println!("{files_ok}/{} files pass", files.len());
    assert!(all_pass, "some SingleStepTests failed");
}

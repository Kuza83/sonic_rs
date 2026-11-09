//! Recompilateur statique 68000 -> Rust.
//!
//! Lit le code de la ROM, explore le flot de contrôle depuis chaque point d'entrée et génère
//! une fonction Rust par routine, calquée instruction par instruction (drapeaux exacts via `rt`).
//! Convention : la fonction générée est appelée alors que l'adresse de retour est déjà sur la pile (comme
//! après un `bsr`) ; elle renvoie l'adresse où l'exécution doit continuer (celle dépilée par son `rts`,
//! ou le point de sortie vers le CPU émulé pour tout ce qu'elle ne sait pas traduire).
#![allow(dead_code)]

use m68k::{decode, Ea, Instruction, Size};
use m68k::{AluOp, BitNumber, BitOp, ExgMode, MulDivOp, ShiftCount, ShiftKind, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Opérande résolu (mots d'extension lus).
#[derive(Clone, Debug)]
enum Opd {
    Dn(u8), An(u8), Ind(u8), Post(u8), Pre(u8),
    Disp(u8, i16), Idx(u8, u16), Abs(u32), PcAbs(u32), PcIdx(u32, u16), Imm(u32),
}

struct Insn {
    pc: u32,
    next: u32,
    ins: Instruction,
    ops: Vec<Opd>,      // opérandes dans l'ordre de lecture des mots d'extension
    imm: u32,           // mot immédiat propre à l'instruction (masque movem, déplacement, n° de bit...)
}

struct Reader<'a> { rom: &'a [u8], pc: u32 }
impl Reader<'_> {
    fn word(&mut self) -> u16 {
        let a = self.pc as usize;
        let w = if a + 1 < self.rom.len() { u16::from_be_bytes([self.rom[a], self.rom[a + 1]]) } else { 0 };
        self.pc += 2;
        w
    }
    fn long(&mut self) -> u32 { let h = self.word() as u32; (h << 16) | self.word() as u32 }
}

fn read_ea(rd: &mut Reader, ea: Ea, sz: Size) -> Opd {
    match ea {
        Ea::DataReg(r) => Opd::Dn(r),
        Ea::AddrReg(r) => Opd::An(r),
        Ea::Indirect(r) => Opd::Ind(r),
        Ea::PostInc(r) => Opd::Post(r),
        Ea::PreDec(r) => Opd::Pre(r),
        Ea::Disp16(r) => Opd::Disp(r, rd.word() as i16),
        Ea::Index8(r) => Opd::Idx(r, rd.word()),
        Ea::AbsShort => Opd::Abs(rd.word() as i16 as i32 as u32),
        Ea::AbsLong => Opd::Abs(rd.long()),
        Ea::PcDisp16 => { let base = rd.pc; let d = rd.word() as i16 as i32; Opd::PcAbs(base.wrapping_add(d as u32)) }
        Ea::PcIndex8 => { let base = rd.pc; Opd::PcIdx(base, rd.word()) }
        Ea::Immediate => Opd::Imm(match sz { Size::Long => rd.long(), Size::Word => rd.word() as u32, Size::Byte => rd.word() as u32 & 0xFF }),
    }
}

fn decode_at(rom: &[u8], pc: u32) -> Insn {
    let a = pc as usize;
    let opcode = if a + 1 < rom.len() { u16::from_be_bytes([rom[a], rom[a + 1]]) } else { 0 };
    let ins = decode(opcode);
    let mut rd = Reader { rom, pc: pc + 2 };
    let mut ops = Vec::new();
    let mut imm = 0u32;
    use Instruction::*;
    match ins {
        Move { size, src, dst } => { ops.push(read_ea(&mut rd, src, size)); ops.push(read_ea(&mut rd, dst, size)); }
        Movea { size, src, .. } => ops.push(read_ea(&mut rd, src, size)),
        Lea { src, .. } | Pea { src } => ops.push(read_ea(&mut rd, src, Size::Long)),
        Clr { size, dst } | Tst { size, dst } => ops.push(read_ea(&mut rd, dst, size)),
        AluToReg { size, src, .. } => ops.push(read_ea(&mut rd, src, size)),
        AluToEa { size, dst, .. } => ops.push(read_ea(&mut rd, dst, size)),
        AluImm { size, dst, .. } => { ops.push(read_ea(&mut rd, Ea::Immediate, size)); ops.push(read_ea(&mut rd, dst, size)); }
        AluQuick { size, dst, .. } => ops.push(read_ea(&mut rd, dst, size)),
        AluAddr { size, src, .. } => ops.push(read_ea(&mut rd, src, size)),
        Unary { size, dst, .. } => ops.push(read_ea(&mut rd, dst, size)),
        CcrImm { .. } | SrImm { .. } => imm = rd.word() as u32,
        ShiftMem { dst, .. } => ops.push(read_ea(&mut rd, dst, Size::Word)),
        Bit { bit, dst, .. } => {
            if matches!(bit, BitNumber::Immediate) { imm = rd.word() as u32; }
            ops.push(read_ea(&mut rd, dst, Size::Byte));
        }
        Scc { dst, .. } | Tas { dst } | Nbcd { dst } | MoveFromSr { dst } => ops.push(read_ea(&mut rd, dst, Size::Byte)),
        Bcc { disp, .. } | Bsr { disp } => { if disp == 0 { imm = rd.word() as i16 as i32 as u32; } else { imm = disp as i32 as u32; } }
        Dbcc { .. } => imm = rd.word() as i16 as i32 as u32,
        Jmp { dst } | Jsr { dst } => ops.push(read_ea(&mut rd, dst, Size::Long)),
        Link { .. } => imm = rd.word() as i16 as i32 as u32,
        Movem { size, ea, .. } => { imm = rd.word() as u32; ops.push(read_ea(&mut rd, ea, size)); }
        MoveToCcr { src } | MoveToSr { src } => ops.push(read_ea(&mut rd, src, Size::Word)),
        MulDiv { src, .. } | Chk { src, .. } => ops.push(read_ea(&mut rd, src, Size::Word)),
        Stop => imm = rd.word() as u32,
        Movep { .. } => imm = rd.word() as u32,
        _ => {}
    }
    Insn { pc, next: rd.pc, ins, ops, imm }
}

fn hex(v: u32) -> String { format!("0x{:X}u32", v) }
fn szc(sz: Size) -> &'static str { match sz { Size::Byte => "B", Size::Word => "W", Size::Long => "L" } }
fn szn(sz: Size) -> u8 { match sz { Size::Byte => 1, Size::Word => 2, Size::Long => 4 } }

/// Emplacement d'un opérande après calcul d'adresse.
enum Loc { D(u8), A(u8), M(String), Imm(u32) }

struct Ctx { out: Vec<String>, tmp: u32 }
impl Ctx {
    fn new() -> Self { Ctx { out: Vec::new(), tmp: 0 } }
    fn emit(&mut self, s: impl Into<String>) { self.out.push(s.into()); }
    fn t(&mut self) -> String { self.tmp += 1; format!("t{}", self.tmp) }

    /// Adresse d'un mode « mémoire » (sans effet de bord).
    fn addr_of(&self, o: &Opd) -> String {
        match o {
            Opd::Ind(r) => format!("m.a[{r}]"),
            Opd::Disp(r, d) => format!("m.a[{r}].wrapping_add({})", hex(*d as i32 as u32)),
            Opd::Idx(r, e) => format!("m.a[{r}].wrapping_add({}).wrapping_add({})", hex(*e as u8 as i8 as i32 as u32), self.index(*e)),
            Opd::Abs(a) | Opd::PcAbs(a) => hex(*a),
            Opd::PcIdx(base, e) => format!("{}.wrapping_add({}).wrapping_add({})", hex(*base), hex(*e as u8 as i8 as i32 as u32), self.index(*e)),
            _ => unreachable!("mode sans adresse: {o:?}"),
        }
    }
    fn index(&self, e: u16) -> String {
        let reg = (e >> 12) & 7;
        let base = if e & 0x8000 != 0 { format!("m.a[{reg}]") } else { format!("m.d[{reg}]") };
        if e & 0x0800 != 0 { base } else { format!("sx({base}, W)") }
    }

    /// Résout un opérande : émet le calcul d'adresse (avec pré-décrément / post-incrément) et renvoie son emplacement.
    fn resolve(&mut self, o: &Opd, sz: Size) -> Loc {
        let n = szn(sz) as u32;
        match o {
            Opd::Dn(r) => Loc::D(*r),
            Opd::An(r) => Loc::A(*r),
            Opd::Imm(v) => Loc::Imm(*v),
            Opd::Post(r) => {
                let step = if *r == 7 && n == 1 { 2 } else { n };
                let t = self.t();
                self.emit(format!("let {t} = m.a[{r}]; m.a[{r}] = {t}.wrapping_add({step});"));
                Loc::M(t)
            }
            Opd::Pre(r) => {
                let step = if *r == 7 && n == 1 { 2 } else { n };
                let t = self.t();
                self.emit(format!("m.a[{r}] = m.a[{r}].wrapping_sub({step}); let {t} = m.a[{r}];"));
                Loc::M(t)
            }
            _ => {
                let t = self.t();
                let e = self.addr_of(o);
                self.emit(format!("let {t} = {e};"));
                Loc::M(t)
            }
        }
    }
    fn load(&self, l: &Loc, sz: Size) -> String {
        match l {
            Loc::D(r) => format!("m.dreg({r}, {})", szc(sz)),
            Loc::A(r) => format!("(m.a[{r}] & mask({}))", szc(sz)),
            Loc::M(t) => format!("m.rd({t}, {})", szc(sz)),
            Loc::Imm(v) => hex(*v),
        }
    }
    fn store(&mut self, l: &Loc, sz: Size, v: &str) {
        match l {
            Loc::D(r) => self.emit(format!("m.set_dreg({r}, {}, {v});", szc(sz))),
            Loc::M(t) => self.emit(format!("m.wr({t}, {}, {v});", szc(sz))),
            _ => unreachable!("écriture dans un opérande non modifiable"),
        }
    }
    /// Lit un opérande dans une variable temporaire et renvoie son nom.
    fn read(&mut self, o: &Opd, sz: Size) -> String {
        let l = self.resolve(o, sz);
        let e = self.load(&l, sz);
        let t = self.t();
        self.emit(format!("let {t} = {e};"));
        t
    }
}

enum Term { Next, Jump(u32), Cond(String, u32, u32), Ret(String), Stop }

struct Func { entry: u32, insns: BTreeMap<u32, Insn>, leaders: BTreeSet<u32> }

pub struct Recompiler<'a> { rom: &'a [u8], entries: BTreeSet<u32>, universal: Vec<(u32, u32)> }

impl<'a> Recompiler<'a> {
    pub fn new(rom: &'a [u8], entries: &[u32]) -> Self { Recompiler { rom, entries: entries.iter().copied().collect(), universal: Vec::new() } }
    /// Régions de code pour lesquelles on génère la routine « universelle » (une entrée par adresse paire).
    pub fn with_universal(mut self, ranges: Vec<(u32, u32)>) -> Self { self.universal = ranges; self }

    fn word_at(&self, a: u32) -> u16 { let a = a as usize; if a + 1 < self.rom.len() { u16::from_be_bytes([self.rom[a], self.rom[a + 1]]) } else { 0 } }

    /// Cibles possibles d'un `jmp (table,pc,Dn)` : table de déplacements mots relatifs à `base`, ou table de `bra`.
    fn jump_targets(&self, base: u32) -> Vec<u32> {
        let w0 = self.word_at(base);
        let mut out = Vec::new();
        if w0 & 0xFF00 == 0x6000 {
            // Table d'instructions BRA : créneaux de 2 octets (bra.s) ou 4 (bra.w).
            let slot = if w0 & 0xFF == 0 { 4 } else { 2 };
            for k in 0..64u32 {
                let a = base + k * slot;
                let w = self.word_at(a);
                if w & 0xFF00 != 0x6000 || (slot == 4) != (w & 0xFF == 0) { break; }
                out.push(a);
            }
            return out;
        }
        let mut min_t = u32::MAX;
        for n in 0..64u32 {
            let at = base + 2 * n;
            if at >= min_t { break; }
            let off = self.word_at(at) as i16 as i32;
            let t = base.wrapping_add(off as u32);
            if off <= 0 || t % 2 != 0 || t as usize >= self.rom.len() || t < at + 2 { break; }
            min_t = min_t.min(t);
            out.push(t);
        }
        out
    }

    fn is_tail_call(&self, entry: u32, target: u32) -> bool { target != entry && self.entries.contains(&target) }

    fn explore(&self, entry: u32) -> Func {
        let mut insns = BTreeMap::new();
        let mut leaders = BTreeSet::new();
        leaders.insert(entry);
        let mut todo = vec![entry];
        while let Some(mut pc) = todo.pop() {
            loop {
                if insns.contains_key(&pc) { break; }
                if pc != entry && self.entries.contains(&pc) { break; }   // la suite est une autre routine : appel terminal
                let i = decode_at(self.rom, pc);
                let next = i.next;
                use Instruction::*;
                let (succ, falls): (Vec<u32>, bool) = match i.ins {
                    Bcc { cond, .. } => {
                        let t = next.wrapping_sub(0).wrapping_add(0); let _ = t;
                        let target = self.branch_target(&i);
                        let inside = !self.is_tail_call(entry, target);
                        if cond == 0 { (if inside { vec![target] } else { vec![] }, false) }
                        else { (if inside { vec![target] } else { vec![] }, true) }
                    }
                    Dbcc { .. } => (vec![self.branch_target(&i)], true),
                    Jmp { .. } => match i.ops[0] {
                        Opd::Abs(a) | Opd::PcAbs(a) => (if self.is_tail_call(entry, a) { vec![] } else { vec![a] }, false),
                        Opd::PcIdx(b, e) => (self.jump_targets(b.wrapping_add(e as u8 as i8 as i32 as u32)), false),
                        _ => (vec![], false),
                    },
                    Rts | Rtr | Rte | Stop | Reset | Illegal | LineA | LineF | Trap { .. } | Trapv => (vec![], false),
                    _ => (vec![], true),
                };
                insns.insert(pc, i);
                for s in &succ { leaders.insert(*s); todo.push(*s); }
                if matches!(insns[&pc].ins, Bcc { cond, .. } if cond != 0) || matches!(insns[&pc].ins, Dbcc { .. }) {
                    leaders.insert(next);
                }
                if !falls { break; }
                pc = next;
            }
        }
        Func { entry, insns, leaders }
    }

    /// Fermeture sur le graphe d'appels : les points d'entrée donnés + toutes les routines appelées directement.
    pub fn closure(rom: &[u8], seeds: &[u32], limit: usize) -> BTreeSet<u32> {
        let mut set: BTreeSet<u32> = seeds.iter().copied().collect();
        let mut todo: Vec<u32> = seeds.to_vec();
        let r = Recompiler { rom, entries: BTreeSet::new(), universal: Vec::new() };
        while let Some(e) = todo.pop() {
            let f = r.explore(e);
            for i in f.insns.values() {
                let mut ts: Vec<u32> = Vec::new();
                match i.ins {
                    Instruction::Bsr { .. } => ts.push(r.branch_target(i)),
                    Instruction::Jsr { .. } => match i.ops[0] {
                        Opd::Abs(a) | Opd::PcAbs(a) => ts.push(a),
                        // jsr (table,pc,Dn) : chaque créneau de la table est un point d'entrée
                        Opd::PcIdx(b, e) => ts.extend(r.jump_targets(b.wrapping_add(e as u8 as i8 as i32 as u32))),
                        _ => {}
                    },
                    _ => {}
                }
                for t in ts {
                    if (t as usize) < rom.len() && t >= 0x200 && !set.contains(&t) && set.len() < limit { set.insert(t); todo.push(t); }
                }
            }
        }
        set
    }

    /// Balayage linéaire d'une région de code : renvoie les adresses qui suivent une instruction terminale
    /// (rts, rte, rtr, jmp, bra inconditionnel, illégale) = débuts probables de routines.
    pub fn sweep(rom: &[u8], start: u32, end: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut pc = start & !1;
        let mut prev_term = true;
        while pc < end && (pc as usize) + 2 <= rom.len() {
            let i = decode_at(rom, pc);
            if prev_term && !matches!(i.ins, Instruction::Illegal | Instruction::Nop) { out.push(pc); }
            // Adresses « prises » (pointeurs de routines calculés) : lea / pea / move.l #adresse.
            let taken = match i.ins {
                Instruction::Lea { .. } | Instruction::Pea { .. } => match i.ops[0] { Opd::PcAbs(t) | Opd::Abs(t) => Some(t), _ => None },
                Instruction::Move { size: Size::Long, .. } | Instruction::Movea { size: Size::Long, .. } => match i.ops[0] { Opd::Imm(t) => Some(t), _ => None },
                _ => None,
            };
            if let Some(t) = taken {
                if t >= 0x200 && t % 2 == 0 && (t as usize) + 2 <= rom.len() && !matches!(decode_at(rom, t).ins, Instruction::Illegal) { out.push(t); }
            }
            prev_term = matches!(i.ins,
                Instruction::Rts | Instruction::Rte | Instruction::Rtr | Instruction::Jmp { .. } | Instruction::Illegal
                | Instruction::Bcc { cond: 0, .. });
            pc = i.next;
        }
        // Tables de pointeurs de routines : longs alignés de la région qui désignent une adresse de code valide.
        let mut a = start & !3;
        while a < end && (a as usize) + 4 <= rom.len() {
            let t = u32::from_be_bytes([rom[a as usize], rom[a as usize + 1], rom[a as usize + 2], rom[a as usize + 3]]);
            if t >= start && t < end && t % 2 == 0 && !matches!(decode_at(rom, t).ins, Instruction::Illegal | Instruction::Nop) { out.push(t); }
            a += 4;
        }
        out
    }

    fn branch_target(&self, i: &Insn) -> u32 {
        // Le déplacement est relatif à l'adresse du mot d'extension (pc + 2).
        i.pc.wrapping_add(2).wrapping_add(i.imm)
    }

    /// Octets de la ROM occupés par des instructions réellement atteintes par le flot de contrôle depuis les points d'entrée
    /// (exclut les tables de données intercalées dans le code). Sert à produire le fichier d'assets sans le code.
    pub fn code_bytes(&self) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        for &e in &self.entries {
            let f = self.explore(e);
            for i in f.insns.values() { for a in (i.pc..i.next).step_by(1) { out.insert(a); } }
        }
        out
    }

    /// Génère le fichier Rust complet pour les points d'entrée donnés.
    pub fn generate(&self) -> String {
        let mut s = String::new();
        s.push_str("// Fichier généré par `sonic1_rs --recompile` — ne pas modifier à la main.\n#![allow(unused_variables, unused_mut, unused_assignments, unreachable_code, unused_parens, clippy::all)]\nuse sonic1_rt::machine::Machine;\nuse sonic1_rt::rt::*;\n\n");
        let mut names = Vec::new();
        let ord: std::collections::HashMap<u32, usize> = self.entries.iter().enumerate().map(|(i, e)| (*e, i)).collect();
        let _ = writeln!(s, "/// Couverture : nombre d'exécutions de chaque routine générée (même ordre que `TABLE`).\npub static COV: [std::sync::atomic::AtomicU32; {}] = [const {{ std::sync::atomic::AtomicU32::new(0) }}; {}];\n", self.entries.len(), self.entries.len());
        for &e in &self.entries {
            let f = self.explore(e);
            s.push_str(&self.emit_func(&f, ord[&e]));
            names.push(e);
        }
        s.push_str(&self.emit_universal());
        s.push_str("pub type GenFn = fn(&mut Machine) -> u32;\n\n");
        s.push_str("pub static TABLE: &[(u32, &str, GenFn)] = &[\n");
        for e in &names { let _ = writeln!(s, "    (0x{e:X}, \"GEN_{e:06X}\", g_{e:06x}),"); }
        s.push_str("];\n\npub fn lookup(a: u32) -> Option<GenFn> {\n    match a & 0xFFFFFF {\n");
        for e in &names { let _ = writeln!(s, "        0x{e:X} => Some(g_{e:06x}),"); }
        s.push_str("        _ => None,\n    }\n}\n");
        s
    }

    fn emit_func(&self, f: &Func, ordinal: usize) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "/// Routine 0x{:06X} ({} instructions).", f.entry, f.insns.len());
        let _ = writeln!(s, "pub fn g_{:06x}(m: &mut Machine) -> u32 {{", f.entry);
        let _ = writeln!(s, "    COV[{ordinal}].fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n    let mut pc: u32 = 0x{:X};\n    loop {{\n        match pc {{", f.entry);
        for &l in &f.leaders {
            if !f.insns.contains_key(&l) { continue; }
            let mut arm = String::new();
            let mut pc = l;
            let mut impure = false;
            let mut wait_loop = false;
            loop {
                let i = &f.insns[&pc];
                let mut cx = Ctx::new();
                let term = self.emit_insn(f, i, &mut cx);
                impure |= cx.out.iter().any(|x| x.contains("m.wr(") || x.contains("m.wr32") || x.contains("wrapping_sub(4)") || x.contains("g_") || x.contains("lookup") || x.contains("return") || x.contains("m.bitop(1") || x.contains("m.bitop(2") || x.contains("m.bitop(3"));
                for line in &cx.out { let _ = writeln!(arm, "                {line}"); }
                let next_leader = f.leaders.contains(&i.next);
                match term {
                    Term::Next => {
                        if next_leader || !f.insns.contains_key(&i.next) {
                            if f.insns.contains_key(&i.next) { let _ = writeln!(arm, "                pc = 0x{:X};", i.next); }
                            else if self.entries.contains(&i.next) { let _ = writeln!(arm, "                return g_{:06x}(m);", i.next); }
                            else { let _ = writeln!(arm, "                return 0x{:X};", i.next); }
                            break;
                        }
                        pc = i.next;
                    }
                    Term::Jump(t) => { let _ = writeln!(arm, "                pc = 0x{t:X};"); break; }
                    Term::Cond(c, t, n) => {
                        // Boucle d'attente pure (le bloc boucle sur lui-même sans écrire ni appeler) : le temps doit passer
                        // pour qu'elle se termine (interruption, DMA, Z80...) -> on rend la main au CPU émulé avant de l'entamer.
                        if !impure && (t == l || n == l) { wait_loop = true; }
                        let _ = writeln!(arm, "                if {c} {{ pc = 0x{t:X}; }} else {{ pc = 0x{n:X}; }}");
                        break;
                    }
                    Term::Ret(e) => { let _ = writeln!(arm, "                return {e};"); break; }
                    Term::Stop => break,
                }
            }
            if wait_loop {
                // Avec CPU émulé : on lui rend la main avant la boucle. Sans CPU : on exécute la boucle en sautant le temps
                // jusqu'à la fin de la ligne vidéo à chaque tour (le matériel avance, les interruptions sont livrées).
                let _ = writeln!(s, "            0x{l:X} => {{\n                if m.con.is_none() {{ return 0x{l:X}; }}\n                m.service(0x{l:X});");
                let arm = arm.replace(&format!("pc = 0x{l:X};"), &format!("m.wait_idle(0x{l:X}); pc = 0x{l:X};"));
                s.push_str(&arm);
                let _ = writeln!(s, "            }}");
            } else {
                let _ = writeln!(s, "            0x{l:X} => {{\n                m.budget -= 1; if m.budget <= 0 {{ return 0x{l:X}; }}\n                m.service(0x{l:X});");
                s.push_str(&arm);
                let _ = writeln!(s, "            }}");
            }
        }
        let _ = writeln!(s, "            _ => {{ if let Some(g) = crate::lookup(pc) {{ return g(m); }} if m.con.is_none() {{ return pc; }} return crate::univ(m, pc); }}\n        }}\n    }}\n}}\n");
        s
    }

    /// Routine universelle : une entrée par adresse paire des régions de code ; sert quand un saut calculé tombe
    /// sur une adresse qui n'est le point d'entrée d'aucune routine. Chaque entrée exécute une seule instruction.
    /// Découpée en « pages » de 256 octets (une fonction par page) pour que la compilation reste raisonnable.
    fn emit_universal(&self) -> String {
        // Les `return X;` des fragments générés renvoient maintenant un u64 (valeur de retour, sans le drapeau CONT).
        fn wrap_returns(line: &str) -> String {
            let mut out = String::new();
            let mut rest = line;
            while let Some(i) = rest.find("return ") {
                out.push_str(&rest[..i]);
                let after = &rest[i + 7..];
                match after.find(';') {
                    Some(j) => { out.push_str(&format!("return (({}) as u64);", &after[..j])); rest = &after[j + 1..]; }
                    None => { out.push_str(&rest[i..]); rest = ""; }
                }
            }
            out.push_str(rest);
            out
        }
        let f = Func { entry: u32::MAX, insns: BTreeMap::new(), leaders: BTreeSet::new() };
        let mut pages: BTreeMap<u32, String> = BTreeMap::new();
        for &(a, b) in &self.universal {
            let mut pc = a & !1;
            while pc < b && (pc as usize) + 2 <= self.rom.len() {
                let i = decode_at(self.rom, pc);
                let mut cx = Ctx::new();
                let term = self.emit_insn(&f, &i, &mut cx);
                let arm = pages.entry(pc & !0xFF).or_default();
                let _ = writeln!(arm, "            0x{pc:X} => {{\n                m.service(0x{pc:X});");
                for line in &cx.out {
                    for l in line.lines() {
                        if l.trim_start().starts_with("//") { continue; }
                        let _ = writeln!(arm, "                {}", wrap_returns(l));
                    }
                }
                match term {
                    Term::Next => { let _ = writeln!(arm, "                pc = 0x{:X};", i.next); }
                    Term::Jump(t) => { let _ = writeln!(arm, "                pc = 0x{t:X};"); }
                    Term::Cond(c, t, n) => { let _ = writeln!(arm, "                if {c} {{ pc = 0x{t:X}; }} else {{ pc = 0x{n:X}; }}"); }
                    Term::Ret(e) => { let _ = writeln!(arm, "                return (({e}) as u64);"); }
                    Term::Stop => {}
                }
                let _ = writeln!(arm, "            }}");
                pc += 2;
            }
        }
        let mut s = String::new();
        s.push_str("const CONT: u64 = 1 << 32;\n\n");
        for (page, arms) in &pages {
            let _ = writeln!(s, "fn up_{page:06x}(m: &mut Machine, mut pc: u32) -> u64 {{\n    loop {{\n        if (pc >> 8) != 0x{:X} {{ return CONT | pc as u64; }}\n        match pc {{", page >> 8);
            s.push_str(arms);
            s.push_str("            _ => return pc as u64,\n        }\n    }\n}\n\n");
        }
        s.push_str("/// Routine universelle (voir recomp.rs) : exécute le code à partir de `start`, jusqu'au `rts` qui sort de la routine.\npub fn univ(m: &mut Machine, start: u32) -> u32 {\n    let mut pc: u32 = start & 0xFFFFFF;\n    loop {\n        let r = match pc >> 8 {\n");
        for page in pages.keys() { let _ = writeln!(s, "            0x{:X} => up_{page:06x}(m, pc),", page >> 8); }
        s.push_str("            _ => return pc,\n        };\n        if r & CONT != 0 { pc = r as u32; } else { return r as u32; }\n    }\n}\n\n");
        s
    }

    /// Appel direct d'une routine traduite (ou sortie vers le CPU si elle ne l'est pas).
    fn emit_call(&self, cx: &mut Ctx, target: u32, ret: u32) {
        cx.emit(format!("m.a[7] = m.a[7].wrapping_sub(4); m.wr32(m.a[7], {});", hex(ret)));
        if self.entries.contains(&target) {
            cx.emit(format!("let r = g_{target:06x}(m); if r != {} {{ return r; }}", hex(ret)));
        } else {
            cx.emit(format!("if m.con.is_none() {{ return {}; }}", hex(target)));
            cx.emit(format!("let r = crate::univ(m, {}); if r != {} {{ return r; }}", hex(target), hex(ret)));
        }
    }

    fn escape(&self, cx: &mut Ctx, pc: u32) -> Term {
        cx.emit(format!("return {};", hex(pc)));
        Term::Stop
    }

    fn emit_insn(&self, f: &Func, i: &Insn, cx: &mut Ctx) -> Term {
        use Instruction::*;
        let words = (i.next - i.pc) / 2;
        cx.emit(format!("// {:06X}: {:?}", i.pc, i.ins));
        cx.emit(format!("m.cyc({});", 4 * words));
        match i.ins {
            Nop => {}
            Move { size, .. } => {
                let v = cx.read(&i.ops[0], size);
                cx.emit(format!("m.logic_flags({v}, {});", szc(size)));
                let l = cx.resolve(&i.ops[1], size);
                cx.store(&l, size, &v);
            }
            Movea { size, dst, .. } => {
                let v = cx.read(&i.ops[0], size);
                cx.emit(format!("m.a[{dst}] = sx({v}, {});", szc(size)));
            }
            Moveq { value, dst } => {
                cx.emit(format!("m.d[{dst}] = {};", hex(value as i32 as u32)));
                cx.emit(format!("m.logic_flags(m.d[{dst}], L);"));
            }
            Lea { dst, .. } => {
                let a = cx.addr_of(&i.ops[0]);
                cx.emit(format!("m.a[{dst}] = {a};"));
            }
            Pea { .. } => {
                let a = cx.addr_of(&i.ops[0]);
                cx.emit(format!("let ea = {a}; m.a[7] = m.a[7].wrapping_sub(4); m.wr32(m.a[7], ea);"));
            }
            Clr { size, .. } => {
                let l = cx.resolve(&i.ops[0], size);
                if let Loc::M(t) = &l { cx.emit(format!("let _ = m.rd({t}, {});", szc(size))); }
                cx.emit("m.sr = (m.sr & !0x0F) | Z;");
                cx.store(&l, size, "0");
            }
            Tst { size, .. } => {
                let v = cx.read(&i.ops[0], size);
                cx.emit(format!("m.logic_flags({v}, {});", szc(size)));
            }
            Exg { mode, rx, ry } => match mode {
                ExgMode::DataData => cx.emit(format!("m.d.swap({rx}, {ry});")),
                ExgMode::AddrAddr => cx.emit(format!("m.a.swap({rx}, {ry});")),
                ExgMode::DataAddr => cx.emit(format!("std::mem::swap(&mut m.d[{rx}], &mut m.a[{ry}]);")),
            },
            Swap { reg } => {
                cx.emit(format!("m.d[{reg}] = m.d[{reg}].rotate_left(16); m.logic_flags(m.d[{reg}], L);"));
            }
            Ext { size, reg } => {
                let from = if size == Size::Word { "B" } else { "W" };
                cx.emit(format!("let v = sx(m.d[{reg}], {from}); m.set_dreg({reg}, {}, v); m.logic_flags(v, {});", szc(size), szc(size)));
            }
            AluToReg { op, size, dst, .. } => {
                let s = cx.read(&i.ops[0], size);
                let d = format!("m.dreg({dst}, {})", szc(size));
                self.alu_stmt(cx, op, size, &d, &s, &Loc::D(dst));
            }
            AluToEa { op, size, src, .. } => {
                let l = cx.resolve(&i.ops[0], size);
                let d = cx.t();
                let e = cx.load(&l, size);
                cx.emit(format!("let {d} = {e};"));
                let s = format!("m.dreg({src}, {})", szc(size));
                self.alu_stmt(cx, op, size, &d, &s, &l);
            }
            AluImm { op, size, .. } => {
                let imm = if let Opd::Imm(v) = i.ops[0] { v } else { 0 };
                let l = cx.resolve(&i.ops[1], size);
                let d = cx.t();
                let e = cx.load(&l, size);
                cx.emit(format!("let {d} = {e};"));
                self.alu_stmt(cx, op, size, &d, &hex(imm), &l);
            }
            AluQuick { op, size, data, .. } => {
                let l = cx.resolve(&i.ops[0], size);
                if let Loc::A(r) = l {
                    let o = if op == AluOp::Add { "wrapping_add" } else { "wrapping_sub" };
                    cx.emit(format!("m.a[{r}] = m.a[{r}].{o}({data});"));
                } else {
                    let d = cx.t();
                    let e = cx.load(&l, size);
                    cx.emit(format!("let {d} = {e};"));
                    self.alu_stmt(cx, op, size, &d, &format!("{data}u32"), &l);
                }
            }
            AluAddr { op, size, dst, .. } => {
                let s = cx.read(&i.ops[0], size);
                let sv = format!("sx({s}, {})", szc(size));
                match op {
                    AluOp::Add => cx.emit(format!("m.a[{dst}] = m.a[{dst}].wrapping_add({sv});")),
                    AluOp::Sub => cx.emit(format!("m.a[{dst}] = m.a[{dst}].wrapping_sub({sv});")),
                    _ => cx.emit(format!("m.cmp(L, m.a[{dst}], {sv});")),
                }
            }
            AluX { op, size, memory, rx, ry } => {
                let f = if op == AluOp::Add { "addx" } else { "subx" };
                if memory {
                    let step = if size == Size::Byte { 1 } else { szn(size) as u32 };
                    let (sy, sxx) = (if ry == 7 && step == 1 { 2 } else { step }, if rx == 7 && step == 1 { 2 } else { step });
                    cx.emit(format!("m.a[{ry}] = m.a[{ry}].wrapping_sub({sy}); let s = m.rd(m.a[{ry}], {});", szc(size)));
                    cx.emit(format!("m.a[{rx}] = m.a[{rx}].wrapping_sub({sxx}); let d = m.rd(m.a[{rx}], {});", szc(size)));
                    cx.emit(format!("let r = m.{f}({}, d, s); m.wr(m.a[{rx}], {}, r);", szc(size), szc(size)));
                } else {
                    cx.emit(format!("let r = m.{f}({}, m.dreg({rx}, {}), m.dreg({ry}, {})); m.set_dreg({rx}, {}, r);", szc(size), szc(size), szc(size), szc(size)));
                }
            }
            Cmpm { size, ax, ay } => {
                let n = szn(size) as u32;
                let sy = if ay == 7 && n == 1 { 2 } else { n };
                let sxx = if ax == 7 && n == 1 { 2 } else { n };
                cx.emit(format!("let s = m.rd(m.a[{ay}], {}); m.a[{ay}] = m.a[{ay}].wrapping_add({sy});", szc(size)));
                cx.emit(format!("let d = m.rd(m.a[{ax}], {}); m.a[{ax}] = m.a[{ax}].wrapping_add({sxx});", szc(size)));
                cx.emit(format!("m.cmp({}, d, s);", szc(size)));
            }
            Unary { op, size, .. } => {
                let l = cx.resolve(&i.ops[0], size);
                let e = cx.load(&l, size);
                let f = match op { UnaryOp::Neg => "neg", UnaryOp::Negx => "negx", UnaryOp::Not => "not" };
                cx.emit(format!("let r = m.{f}({}, {e});", szc(size)));
                cx.store(&l, size, "r");
            }
            CcrImm { op } => {
                let o = match op { AluOp::And => "&", AluOp::Or => "|", _ => "^" };
                cx.emit(format!("m.sr = (m.sr & !0x1F) | (((m.sr {o} {}u16) & 0x1F));", i.imm & 0x1F));
            }
            Shift { kind, left, size, count, reg } => {
                let k = match kind { ShiftKind::Arithmetic => 0, ShiftKind::Logical => 1, ShiftKind::RotateExtend => 2, ShiftKind::Rotate => 3 };
                let c = match count { ShiftCount::Immediate(n) => format!("{n}u32"), ShiftCount::Register(r) => format!("(m.d[{r}] & 63)") };
                cx.emit(format!("let r = m.shift({k}, {left}, {}, m.d[{reg}], {c}); m.set_dreg({reg}, {}, r);", szc(size), szc(size)));
            }
            ShiftMem { kind, left, .. } => {
                let k = match kind { ShiftKind::Arithmetic => 0, ShiftKind::Logical => 1, ShiftKind::RotateExtend => 2, ShiftKind::Rotate => 3 };
                let l = cx.resolve(&i.ops[0], Size::Word);
                let e = cx.load(&l, Size::Word);
                cx.emit(format!("let r = m.shift({k}, {left}, W, {e}, 1);"));
                cx.store(&l, Size::Word, "r");
            }
            Bit { op, bit, .. } => {
                let number = match bit { BitNumber::Immediate => hex(i.imm), BitNumber::Register(r) => format!("m.d[{r}]") };
                let opn = match op { BitOp::Btst => 0, BitOp::Bchg => 1, BitOp::Bclr => 2, BitOp::Bset => 3 };
                let l = cx.resolve(&i.ops[0], Size::Byte);
                match &l {
                    Loc::D(r) => {
                        cx.emit(format!("let r = m.bitop({opn}, m.d[{r}], {number} & 31);"));
                        if opn != 0 { cx.emit(format!("m.d[{r}] = r;")); }
                    }
                    Loc::Imm(v) => cx.emit(format!("let _ = m.bitop(0, {}, {number} & 7);", hex(*v))),
                    Loc::M(t) => {
                        cx.emit(format!("let r = m.bitop({opn}, m.rd({t}, B), {number} & 7);"));
                        if opn != 0 { cx.emit(format!("m.wr({t}, B, r);")); }
                    }
                    Loc::A(_) => unreachable!(),
                }
            }
            Scc { cond, .. } => {
                let l = cx.resolve(&i.ops[0], Size::Byte);
                if let Loc::M(t) = &l { cx.emit(format!("let _ = m.rd({t}, B);")); }
                cx.emit(format!("let v = if m.cond({cond}) {{ 0xFF }} else {{ 0 }};"));
                cx.store(&l, Size::Byte, "v");
            }
            Tas { .. } => {
                let l = cx.resolve(&i.ops[0], Size::Byte);
                let e = cx.load(&l, Size::Byte);
                cx.emit(format!("let v = {e}; m.logic_flags(v, B);"));
                cx.store(&l, Size::Byte, "(v | 0x80)");
            }
            Bcc { cond, .. } => {
                let t = self.branch_target(i);
                if cond == 0 {
                    if self.is_tail_call(f.entry, t) {
                        cx.emit(format!("return g_{t:06x}(m);"));
                        return Term::Stop;
                    }
                    return Term::Jump(t);
                }
                if self.is_tail_call(f.entry, t) {
                    cx.emit(format!("if m.cond({cond}) {{ return g_{t:06x}(m); }}"));
                    return Term::Next;
                }
                return Term::Cond(format!("m.cond({cond})"), t, i.next);
            }
            Dbcc { cond, reg } => {
                let t = self.branch_target(i);
                cx.emit(format!("let mut taken = false;\nif !m.cond({cond}) {{ let w = (m.d[{reg}] as u16).wrapping_sub(1); m.set_dreg({reg}, W, w as u32); taken = w != 0xFFFF; }}"));
                return Term::Cond("taken".into(), t, i.next);
            }
            Bsr { .. } => {
                let t = self.branch_target(i);
                self.emit_call(cx, t, i.next);
            }
            Jsr { .. } => match &i.ops[0] {
                Opd::Abs(a) | Opd::PcAbs(a) => self.emit_call(cx, *a, i.next),
                o => {
                    let a = cx.addr_of(o);
                    cx.emit(format!("let target = {a}; m.a[7] = m.a[7].wrapping_sub(4); m.wr32(m.a[7], {});", hex(i.next)));
                    cx.emit(format!("if let Some(g) = crate::lookup(target) {{ let r = g(m); if r != {} {{ return r; }} }} else if m.con.is_none() {{ return target; }} else {{ let r = crate::univ(m, target); if r != {} {{ return r; }} }}", hex(i.next), hex(i.next)));
                }
            },
            Jmp { .. } => match &i.ops[0] {
                Opd::Abs(a) | Opd::PcAbs(a) => {
                    if self.is_tail_call(f.entry, *a) { cx.emit(format!("return g_{a:06x}(m);")); return Term::Stop; }
                    return Term::Jump(*a);
                }
                o => {
                    let a = cx.addr_of(o);
                    cx.emit(format!("pc = {a};"));
                    return Term::Stop;
                }
            },
            Rts => {
                cx.emit("let r = m.rd32(m.a[7]); m.a[7] = m.a[7].wrapping_add(4);");
                return Term::Ret("r".into());
            }
            Link { reg } => {
                cx.emit(format!("m.a[7] = m.a[7].wrapping_sub(4); m.wr32(m.a[7], m.a[{reg}]); m.a[{reg}] = m.a[7]; m.a[7] = m.a[7].wrapping_add({});", hex(i.imm)));
            }
            Unlk { reg } => {
                cx.emit(format!("m.a[7] = m.a[{reg}]; m.a[{reg}] = m.rd32(m.a[7]); m.a[7] = m.a[7].wrapping_add(4);"));
            }
            Movem { size, to_memory, .. } => {
                let mask = i.imm as u16;
                let n = szn(size) as u32;
                let reg = |idx: u32| if idx < 8 { format!("m.d[{idx}]") } else { format!("m.a[{}]", idx - 8) };
                if to_memory {
                    if let Opd::Pre(an) = i.ops[0] {
                        cx.emit(format!("let mut ad = m.a[{an}];"));
                        for bit in 0..16u32 {
                            if mask & (1 << bit) == 0 { continue; }
                            let v = reg(15 - bit);
                            cx.emit(format!("ad = ad.wrapping_sub({n}); m.wr(ad, {}, {v});", szc(size)));
                        }
                        cx.emit(format!("m.a[{an}] = ad;"));
                    } else {
                        let a = cx.addr_of(&i.ops[0]);
                        cx.emit(format!("let mut ad = {a};"));
                        for idx in 0..16u32 {
                            if mask & (1 << idx) == 0 { continue; }
                            cx.emit(format!("m.wr(ad, {}, {}); ad = ad.wrapping_add({n});", szc(size), reg(idx)));
                        }
                    }
                } else {
                    if let Opd::Post(an) = i.ops[0] { cx.emit(format!("let mut ad = m.a[{an}];")); }
                    else { let a = cx.addr_of(&i.ops[0]); cx.emit(format!("let mut ad = {a};")); }
                    for idx in 0..16u32 {
                        if mask & (1 << idx) == 0 { continue; }
                        cx.emit(format!("{} = sx(m.rd(ad, {}), {}); ad = ad.wrapping_add({n});", reg(idx), szc(size), szc(size)));
                    }
                    cx.emit("let _ = m.rd(ad, W);");
                    if let Opd::Post(an) = i.ops[0] { cx.emit(format!("m.a[{an}] = ad;")); }
                }
            }
            MoveFromSr { .. } => {
                let l = cx.resolve(&i.ops[0], Size::Word);
                if let Loc::M(t) = &l { cx.emit(format!("let _ = m.rd({t}, W);")); }
                cx.store(&l, Size::Word, "(m.sr as u32)");
            }
            MoveUsp { to_usp, reg } => {
                if to_usp { cx.emit(format!("m.usp = m.a[{reg}];")); } else { cx.emit(format!("m.a[{reg}] = m.usp;")); }
            }
            Stop => {
                // STOP #imm : charge SR puis attend une interruption (sans CPU : le temps avance jusqu'à ce qu'elle soit livrée).
                cx.emit(format!("if m.con.is_none() {{ return {}; }}", hex(i.pc)));
                cx.emit(format!("m.sr = {}u16;", i.imm as u16));
                cx.emit(format!("m.stop_wait({});", hex(i.next)));
            }
            SrImm { op } => {
                let o = match op { AluOp::And => "&", AluOp::Or => "|", _ => "^" };
                cx.emit(format!("m.sr = m.sr {o} {}u16;", i.imm as u16));
            }
            MoveToSr { .. } => {
                let v = cx.read(&i.ops[0], Size::Word);
                cx.emit(format!("m.sr = {v} as u16;"));
            }
            Rte => {
                cx.emit("m.sr = m.rd16(m.a[7]); m.a[7] = m.a[7].wrapping_add(2); let r = m.rd32(m.a[7]); m.a[7] = m.a[7].wrapping_add(4);");
                return Term::Ret("r".into());
            }
            Rtr => {
                cx.emit("let ccr = m.rd16(m.a[7]); m.a[7] = m.a[7].wrapping_add(2); m.sr = (m.sr & !0x1F) | (ccr & 0x1F); let r = m.rd32(m.a[7]); m.a[7] = m.a[7].wrapping_add(4);");
                return Term::Ret("r".into());
            }
            MoveToCcr { .. } => {
                let v = cx.read(&i.ops[0], Size::Word);
                cx.emit(format!("m.sr = (m.sr & !0x1F) | ({v} as u16 & 0x1F);"));
            }
            MulDiv { op, dst, .. } => {
                match op {
                    MulDivOp::Mulu | MulDivOp::Muls => {
                        let s = cx.read(&i.ops[0], Size::Word);
                        let f = if op == MulDivOp::Mulu { "mulu" } else { "muls" };
                        cx.emit(format!("m.d[{dst}] = m.{f}(m.d[{dst}], {s});"));
                        cx.emit("m.cyc(34);");
                    }
                    _ => {
                        let l = cx.resolve(&i.ops[0], Size::Word);
                        let e = cx.load(&l, Size::Word);
                        cx.emit(format!("let s = {e}; if s as u16 == 0 {{ return {}; }}", hex(i.pc)));
                        let f = if op == MulDivOp::Divu { "divu" } else { "divs" };
                        cx.emit(format!("if let Some(r) = m.{f}(m.d[{dst}], s) {{ m.d[{dst}] = r; }}"));
                        cx.emit("m.cyc(130);");
                    }
                }
            }
            // Tout le reste (instructions privilégiées, exceptions, BCD, MOVEP, CHK, STOP, RTE...) : sortie vers le CPU émulé.
            _ => return self.escape(cx, i.pc),
        }
        Term::Next
    }

    /// Opération ALU à deux opérandes sur `d` (valeur courante) et `s` ; écrit le résultat dans `l` sauf CMP.
    fn alu_stmt(&self, cx: &mut Ctx, op: AluOp, size: Size, d: &str, s: &str, l: &Loc) {
        let sz = szc(size);
        match op {
            AluOp::Cmp => cx.emit(format!("m.cmp({sz}, {d}, {s});")),
            _ => {
                let f = match op { AluOp::Add => "add", AluOp::Sub => "sub", AluOp::And => "and", AluOp::Or => "or", _ => "eor" };
                cx.emit(format!("let r = m.{f}({sz}, {d}, {s});"));
                cx.store(l, size, "r");
            }
        }
    }
}

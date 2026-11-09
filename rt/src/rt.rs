//! Runtime du code généré par le recompilateur (`recomp.rs`) : accès registres/mémoire dimensionnés
//! et sémantique exacte des instructions 68000 (drapeaux CCR), calquée sur le CPU de l'émulateur.
#![allow(dead_code)]
use crate::machine::Machine;

pub const B: u8 = 1;
pub const W: u8 = 2;
pub const L: u8 = 4;

pub const X: u16 = 0x10;
pub const N: u16 = 0x08;
pub const Z: u16 = 0x04;
pub const V: u16 = 0x02;
pub const C: u16 = 0x01;

#[inline] pub fn mask(sz: u8) -> u32 { match sz { 1 => 0xFF, 2 => 0xFFFF, _ => 0xFFFF_FFFF } }
#[inline] pub fn msb(sz: u8) -> u32 { match sz { 1 => 0x80, 2 => 0x8000, _ => 0x8000_0000 } }
/// Extension de signe d'une valeur de taille `sz` vers 32 bits.
#[inline] pub fn sx(v: u32, sz: u8) -> u32 {
    match sz { 1 => v as u8 as i8 as i32 as u32, 2 => v as u16 as i16 as i32 as u32, _ => v }
}

impl Machine {
    // ---- registres ---------------------------------------------------------------------
    #[inline] pub fn dreg(&self, r: usize, sz: u8) -> u32 { self.d[r] & mask(sz) }
    #[inline] pub fn set_dreg(&mut self, r: usize, sz: u8, v: u32) {
        let mk = mask(sz);
        self.d[r] = (self.d[r] & !mk) | (v & mk);
    }
    // ---- mémoire -----------------------------------------------------------------------
    #[inline] pub fn rd(&self, a: u32, sz: u8) -> u32 {
        match sz { 1 => self.rd8(a) as u32, 2 => self.rd16(a) as u32, _ => self.rd32(a) }
    }
    #[inline] pub fn wr(&mut self, a: u32, sz: u8, v: u32) {
        match sz { 1 => self.wr8(a, v as u8), 2 => self.wr16(a, v as u16), _ => self.wr32(a, v) }
    }
    // ---- drapeaux ----------------------------------------------------------------------
    #[inline] fn upd(&mut self, affected: u16, ccr: u16) { self.sr = (self.sr & !affected) | (ccr & affected); }
    #[inline] fn nz(v: u32, sz: u8) -> u16 {
        let v = v & mask(sz);
        (if v & msb(sz) != 0 { N } else { 0 }) | (if v == 0 { Z } else { 0 })
    }
    /// N,Z d'après `v` ; V = C = 0 ; X inchangé (MOVE, TST, AND, OR, EOR, NOT, EXT, SWAP, MOVEQ...).
    pub fn logic_flags(&mut self, v: u32, sz: u8) { self.upd(N | Z | V | C, Self::nz(v, sz)); }
    #[inline] pub fn x_bit(&self) -> u32 { (self.sr & X != 0) as u32 }

    /// dst + src + x avec N/Z/V/C(X). Renvoie (valeur, ccr).
    fn add_core(dst: u32, src: u32, x: u32, sz: u8) -> (u32, u16) {
        let mk = mask(sz) as u64;
        let wide = (dst as u64 & mk) + (src as u64 & mk) + x as u64;
        let value = wide as u32 & mask(sz);
        let mut ccr = Self::nz(value, sz);
        if wide > mk { ccr |= C | X; }
        if (src ^ value) & (dst ^ value) & msb(sz) != 0 { ccr |= V; }
        (value, ccr)
    }
    /// dst - src - x avec N/Z/V/C(X).
    fn sub_core(dst: u32, src: u32, x: u32, sz: u8) -> (u32, u16) {
        let mk = mask(sz) as u64;
        let (d, s) = (dst as u64 & mk, (src as u64 & mk) + x as u64);
        let value = d.wrapping_sub(s) as u32 & mask(sz);
        let mut ccr = Self::nz(value, sz);
        if s > d { ccr |= C | X; }
        if (dst ^ src) & (dst ^ value) & msb(sz) != 0 { ccr |= V; }
        (value, ccr)
    }
    pub fn add(&mut self, sz: u8, dst: u32, src: u32) -> u32 { let (v, c) = Self::add_core(dst, src, 0, sz); self.upd(N | Z | V | C | X, c); v }
    pub fn sub(&mut self, sz: u8, dst: u32, src: u32) -> u32 { let (v, c) = Self::sub_core(dst, src, 0, sz); self.upd(N | Z | V | C | X, c); v }
    pub fn cmp(&mut self, sz: u8, dst: u32, src: u32) { let (_, c) = Self::sub_core(dst, src, 0, sz); self.upd(N | Z | V | C, c); }
    pub fn and(&mut self, sz: u8, dst: u32, src: u32) -> u32 { let v = dst & src & mask(sz); self.logic_flags(v, sz); v }
    pub fn or(&mut self, sz: u8, dst: u32, src: u32) -> u32 { let v = (dst | src) & mask(sz); self.logic_flags(v, sz); v }
    pub fn eor(&mut self, sz: u8, dst: u32, src: u32) -> u32 { let v = (dst ^ src) & mask(sz); self.logic_flags(v, sz); v }
    fn extended(&mut self, add: bool, sz: u8, dst: u32, src: u32) -> u32 {
        let x = self.x_bit();
        let (v, c) = if add { Self::add_core(dst, src, x, sz) } else { Self::sub_core(dst, src, x, sz) };
        let keep_z = self.sr & Z;
        let ccr = (c & !Z) | if v == 0 { keep_z } else { 0 };
        self.upd(X | N | Z | V | C, ccr);
        v
    }
    pub fn addx(&mut self, sz: u8, dst: u32, src: u32) -> u32 { self.extended(true, sz, dst, src) }
    pub fn subx(&mut self, sz: u8, dst: u32, src: u32) -> u32 { self.extended(false, sz, dst, src) }
    pub fn neg(&mut self, sz: u8, v: u32) -> u32 { let (r, c) = Self::sub_core(0, v, 0, sz); self.upd(X | N | Z | V | C, c); r }
    pub fn negx(&mut self, sz: u8, v: u32) -> u32 { self.extended(false, sz, 0, v) }
    pub fn not(&mut self, sz: u8, v: u32) -> u32 { let r = !v & mask(sz); self.logic_flags(r, sz); r }

    /// Condition 68000 (table des codes `cc`).
    pub fn cond(&self, cc: u8) -> bool {
        let s = self.sr;
        let (n, z, v, c) = (s & N != 0, s & Z != 0, s & V != 0, s & C != 0);
        match cc & 0xF {
            0x0 => true, 0x1 => false, 0x2 => !c && !z, 0x3 => c || z, 0x4 => !c, 0x5 => c,
            0x6 => !z, 0x7 => z, 0x8 => !v, 0x9 => v, 0xA => !n, 0xB => n,
            0xC => n == v, 0xD => n != v, 0xE => !z && n == v, _ => z || n != v,
        }
    }

    /// Décalages / rotations (`kind` : 0 = AS, 1 = LS, 2 = ROX, 3 = RO).
    pub fn shift(&mut self, kind: u8, left: bool, sz: u8, value: u32, count: u32) -> u32 {
        let (mk, ms) = (mask(sz), msb(sz));
        let old = self.sr;
        let mut v = value & mk;
        let mut x = old & X != 0;
        let mut carry = false;
        let mut overflow = false;
        for _ in 0..count {
            let out = if left { v & ms != 0 } else { v & 1 != 0 };
            let msb_before = v & ms;
            v = match (kind, left) {
                (0 | 1, true) => v << 1,
                (0, false) => (v >> 1) | msb_before,
                (1, false) => v >> 1,
                (3, true) => (v << 1) | out as u32,
                (3, false) => (v >> 1) | if out { ms } else { 0 },
                (_, true) => (v << 1) | x as u32,
                (_, false) => (v >> 1) | if x { ms } else { 0 },
            } & mk;
            if kind == 0 && left && v & ms != msb_before { overflow = true; }
            carry = out;
            if kind != 3 { x = out; }
        }
        if count == 0 && kind == 2 { carry = x; }
        let mut ccr = old & X;
        if count > 0 && kind != 3 { ccr = if x { X } else { 0 }; }
        if v & ms != 0 { ccr |= N; }
        if v == 0 { ccr |= Z; }
        if overflow { ccr |= V; }
        if carry { ccr |= C; }
        self.sr = (old & !0x1F) | ccr;
        v
    }

    /// Opérations sur les bits : renvoie la nouvelle valeur ; Z = bit était nul. `op` : 0 BTST, 1 BCHG, 2 BCLR, 3 BSET.
    pub fn bitop(&mut self, op: u8, value: u32, bit: u32) -> u32 {
        let z = if value & (1 << bit) == 0 { Z } else { 0 };
        self.sr = (self.sr & !Z) | z;
        match op { 0 => value, 1 => value ^ (1 << bit), 2 => value & !(1 << bit), _ => value | (1 << bit) }
    }

    pub fn mulu(&mut self, d: u32, s: u32) -> u32 { let r = (d as u16 as u32) * (s as u16 as u32); self.logic_flags(r, L); r }
    pub fn muls(&mut self, d: u32, s: u32) -> u32 { let r = ((d as u16 as i16 as i32) * (s as u16 as i16 as i32)) as u32; self.logic_flags(r, L); r }
    /// DIVU : Some(résultat) ou None si dépassement (V=1, C=0, destination inchangée). Diviseur nul : à traiter avant.
    pub fn divu(&mut self, d: u32, s: u32) -> Option<u32> {
        let s = s as u16 as u32;
        let q = d / s;
        if q > 0xFFFF { self.sr = (self.sr & !C) | V; return None; }
        let r = d % s;
        self.logic_flags(q, W);
        Some((r << 16) | (q & 0xFFFF))
    }
    pub fn divs(&mut self, d: u32, s: u32) -> Option<u32> {
        let (dd, ss) = (d as i32, s as u16 as i16);
        let q = dd as i64 / ss as i64;
        if q != q as i16 as i64 { self.sr = (self.sr & !C) | V; return None; }
        let r = dd as i64 % ss as i64;
        self.logic_flags(q as u32, W);
        Some(((r as u32) << 16) | (q as u32 & 0xFFFF))
    }

    /// Ajoute des cycles 68000 (estimation).
    #[inline] pub fn cyc(&self, n: u32) { self.cycles.set(self.cycles.get() + n); }
}

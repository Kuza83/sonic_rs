//! Modèle minimal de la Mega Drive : ROM, RAM 68000, ports VDP/E-S/Z80 et registres D0-D7/A0-A7.
//!
//! Le pseudo-code Ghidra utilise les registres 68000 comme arguments implicites
//! (`in_D0`, `unaff_A6`, `extraout_A0`...). On les traduit donc en `m.d[n]` / `m.a[n]`.

use m68k::Bus;

pub const ROM_SIZE: usize = 0x80000;
pub const RAM_BASE: u32 = 0xFF0000;

pub struct Machine {
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,      // 64 Kio à 0xFF0000
    pub z80ram: Vec<u8>,   // 8 Kio à 0xA00000
    pub io: [u8; 0x20],    // 0xA10000..0xA1001F
    pub vdp_data_log: Vec<u16>,
    pub vdp_ctrl_log: Vec<u32>,
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub sr_carry: bool,
    /// Si présent, tous les accès mémoire passent par le vrai bus de l'émulateur.
    pub bus: Option<*mut md_core::MdBus>,
    /// Estimation des cycles 68000 consommés (4 par accès bus de 16 bits).
    pub cycles: std::cell::Cell<u32>,
    /// SR du CPU à l'entrée du hook (octet système + masque d'interruptions utiles à `move sr,Dn`).
    pub sr: u16,
    /// Budget de blocs de base pour le code généré (évite les boucles d'attente infinies : sortie vers le CPU émulé).
    pub budget: i32,
    /// Mode « sans CPU » : console matérielle à laquelle on fournit les cycles consommés.
    pub con: Option<*mut md_core::Console>,
    /// Appelé à chaque image terminée (présentation, entrées, cadence) ; renvoie false pour quitter.
    pub frame_cb: Option<fn(&mut md_core::Console) -> bool>,
    /// Pointeur de pile utilisateur (MOVE USP).
    pub usp: u32,
}

impl Machine {
    pub fn new(rom: Vec<u8>) -> Self {
        Self {
            rom,
            ram: vec![0; 0x10000],
            z80ram: vec![0; 0x2000],
            io: [0; 0x20],
            vdp_data_log: Vec::new(),
            vdp_ctrl_log: Vec::new(),
            d: [0; 8],
            a: [0; 8],
            sr_carry: false,
            bus: None,
            cycles: std::cell::Cell::new(0),
            sr: 0x2700,
            budget: 20000,
            con: None,
            frame_cb: None,
            usp: 0,
        }
    }

    /// Machine « attachée » : mémoire, VDP et E/S sont ceux de l'émulateur.
    /// Le pointeur doit rester valide tant que la Machine est utilisée.
    pub fn attached(bus: &mut md_core::MdBus) -> Self {
        let mut m = Self::new(Vec::new());
        m.bus = Some(bus as *mut _);
        m
    }

    #[inline]
    fn tick(&self, c: u32) {
        self.cycles.set(self.cycles.get() + c);
    }

    #[inline]
    fn bus(&self) -> Option<&mut md_core::MdBus> {
        // SAFETY: `attached` garantit un pointeur valide et exclusif pendant l'appel du hook.
        self.bus.map(|p| unsafe { &mut *p })
    }


    #[inline]
    fn norm(addr: u32) -> u32 {
        // Le 68000 n'a que 24 bits d'adresse.
        addr & 0x00FF_FFFF
    }

    pub fn rd8(&self, addr: u32) -> u8 {
        if let Some(b) = self.bus() { self.tick(2); if let Some(t) = crate::TRAP.get() { t.check(addr, 1); } return b.read8(addr); }
        let a = Self::norm(addr);
        match a {
            0x000000..=0x3FFFFF => *self.rom.get(a as usize).unwrap_or(&0),
            0xA00000..=0xA01FFF => self.z80ram[(a - 0xA00000) as usize],
            0xA10000..=0xA1001F => self.io[(a - 0xA10000) as usize],
            0xFF0000..=0xFFFFFF => self.ram[(a - RAM_BASE) as usize],
            _ => 0,
        }
    }

    pub fn rd16(&self, addr: u32) -> u16 {
        if let Some(b) = self.bus() { self.tick(4); if let Some(t) = crate::TRAP.get() { t.check(addr, 2); } return b.read16(addr); }
        ((self.rd8(addr) as u16) << 8) | self.rd8(addr.wrapping_add(1)) as u16
    }

    pub fn rd32(&self, addr: u32) -> u32 {
        if let Some(b) = self.bus() { self.tick(8); if let Some(t) = crate::TRAP.get() { t.check(addr, 4); } return b.read32(addr); }
        ((self.rd16(addr) as u32) << 16) | self.rd16(addr.wrapping_add(2)) as u32
    }

    pub fn wr8(&mut self, addr: u32, v: u8) {
        if let Some(b) = self.bus() { self.tick(4); return b.write8(addr, v); }
        let a = Self::norm(addr);
        match a {
            0xA00000..=0xA01FFF => self.z80ram[(a - 0xA00000) as usize] = v,
            0xA10000..=0xA1001F => self.io[(a - 0xA10000) as usize] = v,
            0xFF0000..=0xFFFFFF => self.ram[(a - RAM_BASE) as usize] = v,
            _ => {}
        }
    }

    pub fn wr16(&mut self, addr: u32, v: u16) {
        if let Some(b) = self.bus() { self.tick(4); return b.write16(addr, v); }
        let a = Self::norm(addr);
        match a {
            0xC00000..=0xC00003 => self.vdp_data_log.push(v),
            0xC00004..=0xC00007 => self.vdp_ctrl_log.push(v as u32),
            _ => {
                self.wr8(addr, (v >> 8) as u8);
                self.wr8(addr.wrapping_add(1), v as u8);
            }
        }
    }

    pub fn wr32(&mut self, addr: u32, v: u32) {
        if let Some(b) = self.bus() { self.tick(8); return b.write32(addr, v); }
        let a = Self::norm(addr);
        match a {
            0xC00000..=0xC00003 => {
                self.vdp_data_log.push((v >> 16) as u16);
                self.vdp_data_log.push(v as u16);
            }
            0xC00004..=0xC00007 => self.vdp_ctrl_log.push(v),
            _ => {
                self.wr16(addr, (v >> 16) as u16);
                self.wr16(addr.wrapping_add(2), v as u16);
            }
        }
    }
}

// ---- Aides pour manipuler les sous-registres comme le fait le 68000 ----
#[inline] pub fn set_w(r: &mut u32, v: u16) { *r = (*r & 0xFFFF_0000) | v as u32; }
#[inline] pub fn set_b(r: &mut u32, v: u8) { *r = (*r & 0xFFFF_FF00) | v as u32; }
#[inline] pub fn sx16(v: u16) -> u32 { v as i16 as i32 as u32 }
#[inline] pub fn sx8(v: u8) -> u32 { v as i8 as i32 as u32 }

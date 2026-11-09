//! Runtime partagé par le programme et le code généré.
pub mod machine;
pub mod nocpu;
pub mod rt;

use machine::Machine;

pub type GenFn = fn(&mut Machine) -> u32;

/// Table de routines générées, enregistrée par le programme au démarrage (évite une dépendance circulaire).
pub struct GenApi {
    pub lookup: fn(u32) -> Option<GenFn>,
    pub univ: fn(&mut Machine, u32) -> u32,
}
pub static GEN: std::sync::OnceLock<GenApi> = std::sync::OnceLock::new();

/// Espion de lectures : signale toute lecture (en tant que donnée) d'un octet de ROM marqué « code effacé ».
pub struct Trap {
    pub map: Vec<bool>,
    pub seen: std::sync::Mutex<std::collections::HashSet<u32>>,
    pub file: std::sync::Mutex<std::fs::File>,
}
pub static TRAP: std::sync::OnceLock<Trap> = std::sync::OnceLock::new();

impl Trap {
    pub fn check(&self, addr: u32, size: u32) {
        let a = addr & 0xFFFFFF;
        for k in 0..size {
            let b = a + k;
            if (b as usize) < self.map.len() && self.map[b as usize] && self.seen.lock().unwrap().insert(b) {
                use std::io::Write;
                let _ = writeln!(self.file.lock().unwrap(), "{b:x}");
            }
        }
    }
}

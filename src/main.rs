//! Sonic the Hedgehog (Mega Drive) en Rust : le code 68000 de la ROM est recompilé en Rust (crate `sonic1_gen`),
//! exécuté sans CPU émulé par le runtime `sonic1_rt` ; `md-core` ne fournit que le matériel (VDP, YM2612, PSG, Z80).
//!
//! Usage : `sonic1_rs` (fenêtre) ; `--headless N [sortie.ppm]` ; outils : `--make-assets`, `--recompile`, `--sweep`, `--flow`, `--disasm`.
//! Les données viennent de `SONIC_ASSETS` / `./sonic1.assets` si présent, sinon de la ROM (`SONIC_ROM`).
mod devtools;
mod hd;
mod pack;
mod host;
mod recomp;
pub use sonic1_gen as generated;
pub use sonic1_rt::{machine, nocpu, rt};

use anyhow::Result;

fn main() -> Result<()> {
    let _ = sonic1_rt::GEN.set(sonic1_rt::GenApi { lookup: generated::lookup, univ: generated::univ });
    let mut args = std::env::args().skip(1).peekable();
    if let Some(r) = devtools::run(&mut args) { return r; }
    match args.peek().map(String::as_str) {
        Some("--headless") | Some("--nocpu-headless") => {
            args.next();
            let n: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(300);
            let out = args.next().filter(|x| x.ends_with(".ppm") || x.ends_with(".png"));
            host::run_nocpu(Some((n, out)))
        }
        _ => host::run_nocpu(None), // `--nocpu` accepté pour compatibilité
    }
}

#!/bin/sh
# Régénère src/generated.rs : routines à la main connues + graines (vecteurs, objets, balayage statique, découvertes)
# + tout ce qu'elles appellent, plus la routine universelle (une entrée par adresse paire du code, en pages de 256 octets).
# Pour que la compilation du générateur reste rapide, on repart d'un generated.rs minimal.
cd "$(dirname "$0")"
KN=$(cat seeds_known.txt)
cat > gen/src/lib.rs <<'STUB'
use sonic1_rt::machine::Machine;
pub type GenFn = fn(&mut Machine) -> u32;
pub static COV: [std::sync::atomic::AtomicU32; 8] = [const { std::sync::atomic::AtomicU32::new(0) }; 8];
pub static TABLE: &[(u32, &str, GenFn)] = &[];
pub fn lookup(_a: u32) -> Option<GenFn> { None }
pub fn univ(_m: &mut Machine, pc: u32) -> u32 { pc }
STUB
cargo build --release 2>&1 | grep -E '^error' -A6
./target/release/sonic1_rs --recompile "closure:$(cat seeds.txt),$(cat seeds_extra.txt),$(cat seeds_sweep.txt),$KN" 20000 "universal:200-1d800,71a00-72e00" > /tmp/gen_new.rs && cp /tmp/gen_new.rs gen/src/lib.rs
cargo build --release 2>&1 | grep -E '^error' -A9 | head -40

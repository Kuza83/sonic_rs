use sonic1_rt::machine::Machine;
pub type GenFn = fn(&mut Machine) -> u32;
pub static COV: [std::sync::atomic::AtomicU32; 8] = [const { std::sync::atomic::AtomicU32::new(0) }; 8];
pub static TABLE: &[(u32, &str, GenFn)] = &[];
pub fn lookup(_a: u32) -> Option<GenFn> { None }
pub fn univ(_m: &mut Machine, pc: u32) -> u32 { pc }

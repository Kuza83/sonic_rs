//! What one game brings to the environment: where its state lives in work
//! RAM, which buttons the agent may press, the reward and when an episode
//! ends.

use md_core::Buttons;

/// How an episode ended (besides running out of steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The player lost a life.
    Died,
    /// The level was finished.
    Completed,
}

pub trait Game {
    /// The game's variables, as read from RAM at one time.
    type State: Copy;
    /// What the reward remembers within an episode (the best X reached...),
    /// back to its default at every reset.
    type Memory: Default + Copy;
    /// Actions the agent chooses from: the buttons held for one step.
    const ACTIONS: &'static [Buttons];

    /// Reads the state from the 68000 work RAM (64 KiB, address `0xFF0000`
    /// at index 0).
    fn read(&self, ram: &[u8]) -> Self::State;
    /// Reward for the step that led from `prev` to `now`.
    fn reward(&self, memory: &mut Self::Memory, prev: &Self::State, now: &Self::State) -> f32;
    /// How far the player has got (larger is further), for cutting episodes
    /// that stop progressing (`EnvConfig::stall_steps`).
    fn progress(&self, state: &Self::State) -> i64;
    /// Whether the episode that began at `start` is over at `now`.
    fn end(&self, start: &Self::State, now: &Self::State) -> Option<End>;
    /// Numbers in the feature vector (`features`).
    fn feature_count(&self) -> usize;
    /// The state as `feature_count()` numbers of about unit size, for agents
    /// that read numbers rather than the picture.
    fn features(&self, state: &Self::State, out: &mut [f32]);
}

/// Index in the work RAM of the 68000 address `addr` (the RAM is mirrored
/// every 64 KiB at the top of the address space).
fn index(addr: u32) -> usize {
    (addr & 0xFFFF) as usize
}

/// Big-endian byte of work RAM at the 68000 address `addr`.
pub fn ram_u8(ram: &[u8], addr: u32) -> u8 {
    ram[index(addr)]
}

/// Big-endian word of work RAM at the 68000 address `addr`.
pub fn ram_u16(ram: &[u8], addr: u32) -> u16 {
    let i = index(addr);
    u16::from_be_bytes([ram[i], ram[i + 1]])
}

/// Big-endian long of work RAM at the 68000 address `addr`.
pub fn ram_u32(ram: &[u8], addr: u32) -> u32 {
    let i = index(addr);
    u32::from_be_bytes([ram[i], ram[i + 1], ram[i + 2], ram[i + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_reads_are_big_endian_and_mirrored() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x0010..0x0014].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(ram_u8(&ram, 0xFF0010), 0x12);
        assert_eq!(ram_u16(&ram, 0xFF0010), 0x1234);
        assert_eq!(ram_u32(&ram, 0xFF0010), 0x1234_5678);
        assert_eq!(ram_u16(&ram, 0xE00010), 0x1234, "mirror at $E00000");
        assert_eq!(ram_u16(&ram, 0xFF0012), 0x5678);
    }
}

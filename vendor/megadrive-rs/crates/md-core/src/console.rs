//! The whole machine: 68000 + bus, run one scanline at a time.
//!
//! Timing (Sega Retro / Plutiedev, CLAUDE.md): master clock 53.69 MHz, the
//! 68000 runs at master / 7, a line lasts 3420 master cycles (~488.6 68000
//! cycles); 262 lines per frame in NTSC, 313 in PAL. Budgets are kept in
//! master cycles so the fractional part never drifts.
//!
//! Interrupts (J5): the VDP's requested level is presented to the 68000
//! before every instruction; VINT is raised at its cycle within line 224.
//!
//! Z80 (J8): clocked at master / 15, run after each 68000 instruction until
//! it catches up. It only runs while out of reset and not stopped by a 68000
//! bus request; while reset is held it stays in its reset state. Its /INT
//! line is asserted with the vertical interrupt for one line (GPGX).

use m68k::Cpu;

use savestate::{Reader, State, StateError, Visitor, Writer};

use crate::bus::MdBus;
use crate::cartridge::Cartridge;
use crate::input::{Buttons, PadKind};
use crate::io::MdConfig;

pub const MASTER_CYCLES_PER_LINE: u32 = 3420;
pub const M68K_CLOCK_DIVIDER: u32 = 7;
pub const Z80_CLOCK_DIVIDER: u32 = 15;
/// Save state header: magic, format version, ROM hash, body checksum.
const STATE_MAGIC: &[u8; 4] = b"MDRS";
const STATE_VERSION: u32 = 5;

/// FNV-1a 64-bit hash.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
    bytes
        .iter()
        .fold(FNV_OFFSET, |h, &b| (h ^ b as u64).wrapping_mul(FNV_PRIME))
}

#[derive(Debug, Clone)]
pub struct Console {
    config: MdConfig,
    /// Active cheats forcing work RAM (applied at the start of every frame).
    ram_cheats: Vec<crate::cheats::Cheat>,
    cpu: Cpu,
    audio: crate::audio::Audio,
    z80: z80::Cpu,
    bus: MdBus,
    /// Z80 master cycles run past the end of the previous line.
    z80_overrun: u32,
    /// Master cycles run past the end of the previous line (an instruction
    /// cannot be split), charged to the next one.
    overrun: u32,
    frame: u64,
    /// Function-entry interception: when the 68000 is about to execute an
    /// instruction at one of `hook_addrs`, `hook` may run it natively instead.
    hook_addrs: Vec<u32>,
    hook: Option<Hook>,
    /// Profil (diagnostic) : nombre d'instructions exécutées par le CPU émulé, par adresse.
    profile: Option<std::collections::HashMap<u32, u64>>,
    /// Temps matériel « sans CPU » (voir `nc_advance`).
    nc: NoCpu,
}

#[derive(Debug, Clone, Default)]
struct NoCpu { started: bool, line: u16, elapsed: u32, z80_elapsed: u32, vint_done: bool, new_frame: bool }

/// Native replacement for a 68000 routine. Called with the CPU stopped at the
/// routine's first instruction; returns `Some(68000 cycles)` if it did the
/// whole job itself (including the return), `None` to let the ROM code run.
pub type Hook = fn(&mut Cpu, &mut MdBus) -> Option<u32>;

impl Console {
    pub fn new(cart: Cartridge, config: MdConfig) -> Self {
        let mut bus = MdBus::new(cart, config);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        let mut z80 = z80::Cpu::new();
        z80.reset();
        Self {
            config,
            ram_cheats: Vec::new(),
            cpu,
            audio: crate::audio::Audio::new(config.region.is_pal()),
            z80,
            bus,
            z80_overrun: 0,
            overrun: 0,
            frame: 0,
            hook_addrs: Vec::new(),
            hook: None,
            profile: None,
            nc: NoCpu::default(),
        }
    }

    // ---- Temps matériel sans CPU : le programme (code Rust) fournit les cycles 68000 consommés. ----

    fn nc_begin_line(&mut self, line: u16) {
        self.bus.vdp_mut().start_line(line);
        self.bus.io_mut().refresh_line();
        self.z80.set_int(false);
        self.nc.vint_done = false;
    }

    /// Fait avancer le matériel de `cycles` cycles 68000 (Z80, VDP, YM2612, PSG, audio). Renvoie true quand une image
    /// vient de s'achever (framebuffer et `audio_samples()` sont alors valides jusqu'au prochain appel).
    pub fn nc_advance(&mut self, cycles: u32) -> bool {
        if !self.nc.started {
            self.nc.started = true;
            self.apply_ram_cheats();
            self.audio.clear();
            self.nc_begin_line(0);
        }
        if self.nc.new_frame {
            self.nc.new_frame = false;
            self.apply_ram_cheats();
            self.audio.clear();
            self.nc_begin_line(0);
        }
        self.nc.elapsed += cycles * M68K_CLOCK_DIVIDER;
        let mut frame_done = false;
        loop {
            let vdp = self.bus.vdp();
            if self.nc.line == vdp.active_lines() && !self.nc.vint_done && self.nc.elapsed >= vdp.vint_cycle() {
                let at = vdp.vint_cycle();
                let mut ze = self.nc.z80_elapsed;
                self.run_z80_until(&mut ze, at);
                self.nc.z80_elapsed = ze;
                self.bus.vdp_mut().trigger_vint();
                self.z80.set_int(true);
                self.nc.vint_done = true;
            }
            if self.nc.elapsed < MASTER_CYCLES_PER_LINE { break; }
            let mut ze = self.nc.z80_elapsed;
            self.run_z80_until(&mut ze, MASTER_CYCLES_PER_LINE);
            self.bus.vdp_mut().render_line(self.nc.line as usize);
            self.bus.fm_mut().end_line(MASTER_CYCLES_PER_LINE);
            self.audio.run(&mut self.bus, MASTER_CYCLES_PER_LINE);
            self.nc.elapsed -= MASTER_CYCLES_PER_LINE;
            self.nc.z80_elapsed = ze.saturating_sub(MASTER_CYCLES_PER_LINE);
            self.nc.line += 1;
            if self.nc.line >= self.bus.vdp().lines_per_frame() {
                self.nc.line = 0;
                self.frame += 1;
                self.nc.new_frame = true;
                frame_done = true;
                break;
            }
            let l = self.nc.line;
            self.nc_begin_line(l);
        }
        let e = self.nc.elapsed;
        self.bus.vdp_mut().set_line_cycle(e);
        self.bus.set_master_time(e);
        let mut ze = self.nc.z80_elapsed;
        self.run_z80_until(&mut ze, e);
        self.nc.z80_elapsed = ze;
        frame_done
    }

    /// Cycles 68000 restants jusqu'à la fin de la ligne courante (pour sauter une attente).
    pub fn nc_cycles_to_line_end(&self) -> u32 {
        MASTER_CYCLES_PER_LINE.saturating_sub(self.nc.elapsed) / M68K_CLOCK_DIVIDER + 1
    }

    /// Niveau d'interruption demandé par le VDP (0 = aucune).
    pub fn nc_irq_level(&self) -> u8 { self.bus.vdp().irq_level() }

    /// Acquittement d'une interruption par le « CPU » (efface le drapeau servi).
    pub fn nc_ack_irq(&mut self) { self.bus.vdp_mut().acknowledge_interrupt(); }

    /// Active ou coupe le profil des adresses exécutées par le CPU émulé.
    pub fn set_profile(&mut self, on: bool) { self.profile = if on { Some(Default::default()) } else { None }; }

    /// Profil collecté (adresse -> nombre d'exécutions).
    pub fn profile(&self) -> Option<&std::collections::HashMap<u32, u64>> { self.profile.as_ref() }

    /// Installs `hook` for the routines entered at `addrs` (empty = none).
    pub fn set_hooks(&mut self, addrs: &[u32], hook: Option<Hook>) {
        self.hook_addrs = addrs.to_vec();
        self.hook = hook;
    }

    /// Runs one full frame: the CPUs for each line, then the line is rendered.
    pub fn run_frame(&mut self) {
        self.apply_ram_cheats();
        self.audio.clear();
        for line in 0..self.bus.vdp().lines_per_frame() {
            self.bus.vdp_mut().start_line(line);
            self.bus.io_mut().refresh_line();
            // The Z80 interrupt lasts one line from the vertical interrupt.
            self.z80.set_int(false);
            // The vertical interrupt starts VBLANK, after the picture's last
            // line: 224, or 240 in V30 (latched by the VDP at line 0).
            let vint_at =
                (line == self.bus.vdp().active_lines()).then(|| self.bus.vdp().vint_cycle());
            let mut elapsed = self.overrun;
            let mut z80_elapsed = self.z80_overrun;
            if let Some(at) = vint_at {
                elapsed = self.run_until(elapsed, &mut z80_elapsed, at);
                self.bus.vdp_mut().trigger_vint();
                self.z80.set_int(true);
            }
            elapsed = self.run_until(elapsed, &mut z80_elapsed, MASTER_CYCLES_PER_LINE);
            self.run_z80_until(&mut z80_elapsed, MASTER_CYCLES_PER_LINE);
            self.overrun = elapsed - MASTER_CYCLES_PER_LINE;
            self.z80_overrun = z80_elapsed.saturating_sub(MASTER_CYCLES_PER_LINE);
            self.bus.vdp_mut().render_line(line as usize);
            // The YM2612 is completed to the end of the line; the PSG advances
            // a line at a time (its writes take effect at line granularity).
            self.bus.fm_mut().end_line(MASTER_CYCLES_PER_LINE);
            self.audio.run(&mut self.bus, MASTER_CYCLES_PER_LINE);
        }
        self.frame += 1;
    }

    /// Steps the 68000 until `limit` master cycles of the line have elapsed,
    /// letting the Z80 catch up after each instruction; returns the 68000's
    /// actual elapsed count (an instruction may overshoot).
    fn run_until(&mut self, mut elapsed: u32, z80_elapsed: &mut u32, limit: u32) -> u32 {
        while elapsed < limit {
            let level = self.bus.vdp().irq_level();
            self.cpu.set_irq_level(level);
            self.bus.vdp_mut().set_line_cycle(elapsed);
            self.bus.set_master_time(elapsed);
            let hooked = match self.hook {
                Some(h) if self.hook_addrs.contains(&self.cpu.regs.pc) => {
                    h(&mut self.cpu, &mut self.bus)
                }
                _ => None,
            };
            elapsed += match hooked {
                Some(cycles) => cycles,
                None => {
                    if let Some(p) = self.profile.as_mut() { *p.entry(self.cpu.regs.pc).or_insert(0) += 1; }
                    self.cpu.step(&mut self.bus)
                }
            } * M68K_CLOCK_DIVIDER;
            elapsed += self.bus.take_dma_stall();
            self.run_z80_until(z80_elapsed, elapsed.min(limit));
        }
        elapsed
    }

    /// Runs the Z80 up to `limit` master cycles, if it is allowed to run.
    fn run_z80_until(&mut self, z80_elapsed: &mut u32, limit: u32) {
        let io = self.bus.io();
        if !io.z80_running() {
            self.z80.reset();
            *z80_elapsed = (*z80_elapsed).max(limit);
            return;
        }
        if io.z80_busreq() {
            *z80_elapsed = (*z80_elapsed).max(limit);
            return;
        }
        let mut view = crate::z80_bus::Z80View { md: &mut self.bus };
        while *z80_elapsed < limit {
            view.md.set_master_time(*z80_elapsed);
            *z80_elapsed += self.z80.step(&mut view) * Z80_CLOCK_DIVIDER;
        }
    }

    /// Audio of the last frame: interleaved stereo 16-bit samples at
    /// `audio::OUTPUT_RATE` Hz.
    pub fn audio_samples(&self) -> &[i16] {
        self.audio.samples()
    }

    pub fn z80(&self) -> &z80::Cpu {
        &self.z80
    }

    /// YM2612 sound synthesis on (default) or off. Off, the game runs the
    /// same (the chip's status register behaves the same) but the FM part
    /// is silent, several times faster: for training an agent. A run-time
    /// option, not saved in states, kept across resets.
    pub fn set_fm_synthesis(&mut self, on: bool) {
        self.bus.fm_mut().set_synthesis(on);
    }

    pub fn fm_synthesis(&self) -> bool {
        self.bus.fm().synthesis()
    }

    /// Plugs a pad into `port` (0 = port 1, 1 = port 2, 2 = expansion).
    pub fn connect_pad(&mut self, port: usize, kind: Option<PadKind>) {
        self.bus.io_mut().connect(port, kind);
    }

    /// Buttons currently held on the pad in `port`.
    pub fn set_buttons(&mut self, port: usize, buttons: Buttons) {
        self.bus.io_mut().set_buttons(port, buttons);
    }

    /// Activates `cheats` (replacing the previous ones). ROM codes patch reads
    /// of the image; RAM codes are forced every frame. A code aimed elsewhere
    /// is refused and nothing changes.
    pub fn set_cheats(
        &mut self,
        cheats: &[crate::cheats::Cheat],
    ) -> Result<(), crate::cheats::CheatError> {
        let rom_len = self.bus.cartridge().rom().len() as u32;
        if let Some(&bad) = cheats.iter().find(|c| !c.is_ram() && c.address >= rom_len) {
            return Err(crate::cheats::CheatError::Target(bad));
        }
        let (ram, rom): (Vec<crate::cheats::Cheat>, Vec<crate::cheats::Cheat>) =
            cheats.iter().partition(|c| c.is_ram());
        let patches: Vec<(u32, u16)> = rom.iter().map(|c| (c.address, c.value)).collect();
        self.bus.cartridge_mut().set_rom_patches(&patches);
        self.ram_cheats = ram;
        Ok(())
    }

    /// RAM codes (GPGX RAMCheatUpdate): word write when the value's high
    /// byte is set, byte write otherwise.
    fn apply_ram_cheats(&mut self) {
        for cheat in &self.ram_cheats {
            if cheat.value & 0xFF00 != 0 {
                self.bus
                    .write_work_ram(cheat.address & !1, (cheat.value >> 8) as u8);
                self.bus
                    .write_work_ram(cheat.address | 1, cheat.value as u8);
            } else {
                self.bus.write_work_ram(cheat.address, cheat.value as u8);
            }
        }
    }

    /// Reset button: the 68000 restarts from its reset vectors while video,
    /// sound (PSG) and memory keep their contents (see `MdBus::soft_reset`).
    /// GPGX also moves the 68000 to a random point of the frame; the current
    /// position is kept here so runs stay deterministic.
    pub fn reset_soft(&mut self) {
        self.bus.soft_reset();
        self.z80.reset();
        self.cpu.reset(&mut self.bus);
    }

    /// Power off and on: everything restarts except the battery-backed SRAM
    /// and the plugged pads.
    pub fn power_cycle(&mut self) {
        let mut cart = self.bus.cartridge().clone();
        cart.reset();
        let pads: Vec<_> = (0..crate::io::PORT_COUNT)
            .map(|port| self.bus.io().pad_kind(port))
            .collect();
        let synthesis = self.fm_synthesis();
        *self = Console::new(cart, self.config);
        self.set_fm_synthesis(synthesis);
        for (port, kind) in pads.into_iter().enumerate() {
            self.connect_pad(port, kind);
        }
    }

    /// Complete machine state: `MDRS`, version, ROM hash, checksum of the
    /// body, then the tagged sections of every component.
    pub fn save_state(&mut self) -> Vec<u8> {
        let mut body = Writer::new();
        self.visit(&mut body)
            .expect("writing a state has no failure case");
        let body = body.into_bytes();
        let mut out = Writer::new();
        out.bytes(STATE_MAGIC);
        out.bytes(&STATE_VERSION.to_be_bytes());
        out.bytes(&self.bus.cartridge().rom_hash().to_be_bytes());
        out.bytes(&fnv1a(&body).to_be_bytes());
        out.bytes(&body);
        out.into_bytes()
    }

    /// Restores a state made by `save_state` for the same ROM. On any error
    /// the console is left exactly as it was.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let mut r = Reader::new(bytes);
        if r.take(4).map_err(|_| StateError::BadMagic)? != STATE_MAGIC {
            return Err(StateError::BadMagic);
        }
        let mut version = 0u32;
        r.field(&mut version)?;
        if version != STATE_VERSION {
            return Err(StateError::Version(version));
        }
        let (mut rom_hash, mut checksum) = (0u64, 0u64);
        r.field(&mut rom_hash)?;
        r.field(&mut checksum)?;
        if rom_hash != self.bus.cartridge().rom_hash() {
            return Err(StateError::RomMismatch);
        }
        let body = &bytes[bytes.len().min(4 + 4 + 8 + 8)..];
        if fnv1a(body) != checksum {
            return Err(StateError::InvalidValue("checksum"));
        }
        let mut candidate = self.clone();
        let mut r = Reader::new(body);
        candidate.visit(&mut r)?;
        r.finish()?;
        *self = candidate;
        Ok(())
    }

    /// Battery-backed cartridge RAM (64 KiB image), if the cartridge has one.
    pub fn sram(&self) -> Option<&[u8]> {
        self.bus.cartridge().sram().map(|s| s.data())
    }

    /// Restores a saved SRAM image (ignored without SRAM).
    pub fn load_sram(&mut self, bytes: &[u8]) {
        if let Some(sram) = self.bus.cartridge_mut().sram_mut() {
            sram.load(bytes);
        }
    }

    /// Whether the game wrote to SRAM since the last call (to save it lazily).
    pub fn take_sram_dirty(&mut self) -> bool {
        self.bus
            .cartridge_mut()
            .sram_mut()
            .is_some_and(|s| s.take_dirty())
    }

    pub fn frame_count(&self) -> u64 {
        self.frame
    }

    /// CPU et bus en accès mutable (outils de test : injecter un état, appeler un hook).
    pub fn cpu_bus_mut(&mut self) -> (&mut Cpu, &mut MdBus) {
        (&mut self.cpu, &mut self.bus)
    }

    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn bus(&self) -> &MdBus {
        &self.bus
    }

    /// Direct bus access (tests, debugging): bypasses the CPUs and timing.
    pub fn bus_mut(&mut self) -> &mut MdBus {
        &mut self.bus
    }

    /// Big-endian long from 68000 work RAM at `addr` ($FF0000-based).
    pub fn ram_long(&self, addr: u32) -> u32 {
        let i = (addr & 0xFFFF) as usize;
        u32::from_be_bytes(self.bus.work_ram()[i..i + 4].try_into().unwrap())
    }

    /// RGBA8888 pixels (0xRRGGBBAA), `vdp::FRAME_WIDTH` per row.
    pub fn framebuffer(&self) -> &[u32] {
        self.bus.vdp().framebuffer()
    }

    /// Width of the active picture (256 in H32, 320 in H40).
    pub fn screen_width(&self) -> usize {
        self.bus.vdp().registers().screen_width()
    }

    /// Height of the active picture (224, or 240 in V30).
    pub fn screen_height(&self) -> usize {
        self.bus.vdp().active_lines() as usize
    }

    /// Visible picture as RGBA bytes, `screen_width() * screen_height() * 4`.
    pub fn frame_rgba(&self) -> Vec<u8> {
        let width = self.screen_width();
        self.framebuffer()
            .as_chunks::<{ vdp::FRAME_WIDTH }>()
            .0
            .iter()
            .take(self.screen_height())
            .flat_map(|row| row[..width].iter().flat_map(|p| p.to_be_bytes()))
            .collect()
    }

    /// FNV-1a 64-bit hash of the visible picture, for regression tests.
    pub fn frame_hash(&self) -> u64 {
        const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
        self.frame_rgba()
            .iter()
            .fold(FNV_OFFSET, |h, &b| (h ^ b as u64).wrapping_mul(FNV_PRIME))
    }
}

impl State for Console {
    fn visit<V: Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"CONS", |v| {
            v.field(&mut self.z80_overrun)?;
            v.field(&mut self.overrun)?;
            v.field(&mut self.frame)
        })?;
        v.state(&mut self.cpu)?;
        v.state(&mut self.z80)?;
        v.state(&mut self.bus)?;
        v.state(&mut self.audio)
    }
}

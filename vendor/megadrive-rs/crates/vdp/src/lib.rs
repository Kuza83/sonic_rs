//! Video Display Processor (YM7101), mode 5.
//!
//! Port protocol: Plutiedev "VDP commands" / "VDP registers", cross-checked
//! with Genesis Plus GX core/vdp_ctrl.c (`vdp_68k_ctrl_w`, `vdp_bus_w`).
//! J4: registers, VRAM/CRAM/VSRAM, plane A rendering. DMA, scrolling, plane
//! B, window and sprites come in J7.

mod dma;
mod fifo;
mod interrupts;
mod memory;
mod registers;
mod render;

pub use dma::DmaRequest;
pub use interrupts::{
    HINT_LEVEL, LINES_NTSC, LINES_PAL, VINT_CYCLE_H32, VINT_CYCLE_H40, VINT_LEVEL,
};
use memory::{CRAM_ENTRIES, CRAM_MASK, Target, VRAM_SIZE, VSRAM_ENTRIES};
pub use registers::{REGISTER_COUNT, Registers, reg};
pub use render::{FRAME_HEIGHT, FRAME_WIDTH, MAX_FRAME_HEIGHT, cram_to_rgba};

/// Status register bits (Genesis Plus GX, core/vdp_ctrl.c, status register comment).
pub mod status {
    pub const PAL: u16 = 1 << 0;
    pub const DMA_BUSY: u16 = 1 << 1;
    pub const HBLANK: u16 = 1 << 2;
    pub const VBLANK: u16 = 1 << 3;
    pub const ODD_FRAME: u16 = 1 << 4;
    pub const SPRITE_COLLISION: u16 = 1 << 5;
    pub const SPRITE_OVERFLOW: u16 = 1 << 6;
    pub const VINT_PENDING: u16 = 1 << 7;
    pub const FIFO_FULL: u16 = 1 << 8;
    pub const FIFO_EMPTY: u16 = 1 << 9;
}

/// Control word `10xR RRRR DDDD DDDD` writes register R with D.
const REGISTER_WRITE_MASK: u16 = 0xC000;
const REGISTER_WRITE_TAG: u16 = 0x8000;
/// First command word: CD1-0 in bits 15-14, A13-0 in bits 13-0.
const FIRST_WORD_ADDR_MASK: u16 = 0x3FFF;
/// Second command word: CD5-2 in bits 7-4, A15-14 in bits 1-0.
const SECOND_WORD_CODE_MASK: u16 = 0x00F0;
const SECOND_WORD_ADDR_MASK: u16 = 0x0003;
/// Lines of active display in V28 mode (224) and V30 mode (240); VBLANK
/// covers the rest of the frame.
pub const ACTIVE_LINES: u16 = 224;
pub const MAX_ACTIVE_LINES: u16 = 240;

/// Bytes of the internal sprite table copy (GPGX `sat[0x400]`).
const SAT_CACHE_SIZE: usize = 0x400;
/// SAT region and index bits of a VRAM address, H40 then H32 (GPGX
/// `sat_base_mask` / `sat_addr_mask`, register 12).
const SAT_MASKS_H40: (u16, u16) = (0xFC00, 0x03FF);
const SAT_MASKS_H32: (u16, u16) = (0xFE00, 0x01FF);

#[derive(Debug, Clone)]
pub struct Vdp {
    pal: bool,
    regs: Registers,
    vram: Box<[u8; VRAM_SIZE]>,
    cram: [u16; CRAM_ENTRIES],
    vsram: [u16; VSRAM_ENTRIES],
    /// First half of a command received, waiting for the second word.
    pending: bool,
    /// Access code CD5-CD0.
    code: u8,
    /// Access address A15-A0.
    addr: u16,
    /// Current scanline, set by the machine scheduler.
    line: u16,
    /// Active lines of the current frame (224 or 240), latched at its first
    /// line from register 1 (GPGX: "Changes should be applied on next
    /// frame").
    active_lines: u16,
    /// Master cycles elapsed in the current line, set by the scheduler.
    line_cycle: u32,
    /// Horizontal interrupt line counter (reloaded from register 10).
    hint_counter: u8,
    hint_pending: bool,
    vint_pending: bool,
    framebuffer: Vec<u32>,
    /// Origine de chaque pixel du framebuffer (voir `render::provenance`), rempli seulement si `prov_enabled`.
    prov: Vec<u32>,
    prov_enabled: bool,
    /// VRAM et CRAM au moment de la dernière ligne affichée (mode `prov_enabled` seulement).
    prov_snapshot: (Vec<u8>, Vec<u16>),
    lines: render::LineBuffers,
    sprite_state: render::SpriteState,
    /// DMA fill armed: performed on the next data port write.
    fill_pending: bool,
    /// 68000 → VDP transfer waiting to be serviced by the machine.
    dma_request: Option<DmaRequest>,
    /// Fill or copy in progress.
    dma_progress: Option<dma::DmaProgress>,
    /// Data port write FIFO.
    fifo: fifo::Fifo,
    /// Internal copy of the sprite attribute table: Y, size and link are
    /// read from it (GPGX `sat[]`; see `sat_write`).
    sat_cache: Box<[u8; SAT_CACHE_SIZE]>,
    palette: render::PaletteCache,
    /// 68000 wait on a full FIFO not yet taken by the machine.
    fifo_stall: u32,
}

impl Default for Vdp {
    fn default() -> Self {
        Self::new(false)
    }
}

impl Vdp {
    pub fn new(pal: bool) -> Self {
        Self {
            pal,
            regs: Registers::default(),
            vram: Box::new([0; VRAM_SIZE]),
            cram: [0; CRAM_ENTRIES],
            vsram: [0; VSRAM_ENTRIES],
            pending: false,
            code: 0,
            addr: 0,
            line: 0,
            active_lines: ACTIVE_LINES,
            line_cycle: 0,
            hint_counter: 0,
            hint_pending: false,
            vint_pending: false,
            framebuffer: vec![0; FRAME_WIDTH * MAX_FRAME_HEIGHT],
            prov: vec![0; FRAME_WIDTH * MAX_FRAME_HEIGHT],
            prov_enabled: false,
            prov_snapshot: (Vec::new(), Vec::new()),
            lines: render::LineBuffers::default(),
            sprite_state: render::SpriteState::default(),
            fill_pending: false,
            dma_request: None,
            dma_progress: None,
            fifo: fifo::Fifo::default(),
            sat_cache: Box::new([0; SAT_CACHE_SIZE]),
            palette: render::PaletteCache::default(),
            fifo_stall: 0,
        }
    }

    // --- 68000 ports ------------------------------------------------------

    pub fn write_control(&mut self, value: u16) {
        if self.pending {
            self.pending = false;
            self.addr =
                (self.addr & FIRST_WORD_ADDR_MASK) | ((value & SECOND_WORD_ADDR_MASK) << 14);
            self.code = (self.code & 0x03) | ((value & SECOND_WORD_CODE_MASK) >> 2) as u8;
            self.start_dma();
            return;
        }
        // The first word always updates A13-A0 and CD1-CD0, even when it
        // turns out to be a register write (GPGX vdp_68k_ctrl_w).
        self.addr = (self.addr & !FIRST_WORD_ADDR_MASK) | (value & FIRST_WORD_ADDR_MASK);
        self.code = (self.code & 0x3C) | (value >> 14) as u8;
        if value & REGISTER_WRITE_MASK == REGISTER_WRITE_TAG {
            let index = ((value >> 8) & 0x1F) as usize;
            self.regs.set(index, value as u8);
            // Register 0 bit 2 (palette select) changes every colour.
            self.palette.dirty |= index == reg::MODE_1;
        } else if self.regs.mode5() {
            self.pending = true;
        }
    }

    pub fn write_data(&mut self, value: u16) {
        self.fifo_write();
        self.write_data_word(value);
        if std::mem::take(&mut self.fill_pending) {
            self.dma_fill(value);
        }
    }

    /// Data port write without the DMA fill trigger (also used by DMA).
    pub(crate) fn write_data_word(&mut self, value: u16) {
        self.pending = false;
        match Target::from_code(self.code) {
            Target::VramWrite => {
                // Word access at A15-A1; an odd address swaps the bytes.
                let value = if self.addr & 1 != 0 {
                    value.swap_bytes()
                } else {
                    value
                };
                let i = (self.addr & !1) as usize;
                self.vram[i..i + 2].copy_from_slice(&value.to_be_bytes());
                let [high, low] = value.to_be_bytes();
                self.sat_write(i as u16, high);
                self.sat_write(i as u16 | 1, low);
            }
            Target::CramWrite => {
                self.cram[Self::cram_index(self.addr)] = value & CRAM_MASK;
                self.palette.dirty = true;
            }
            Target::VsramWrite => self.vsram[Self::vsram_index(self.addr)] = value,
            _ => {}
        }
        self.advance();
    }

    pub fn read_data(&mut self) -> u16 {
        self.pending = false;
        let value = match Target::from_code(self.code) {
            Target::VramRead => {
                let i = (self.addr & !1) as usize;
                u16::from_be_bytes([self.vram[i], self.vram[i + 1]])
            }
            Target::CramRead => self.cram[Self::cram_index(self.addr)],
            Target::VsramRead => self.vsram[Self::vsram_index(self.addr)],
            _ => 0,
        };
        self.advance();
        value
    }

    /// Reading the status also clears the pending command flag.
    /// Bits 10-15 are open bus on hardware; 0 here.
    pub fn read_status(&mut self) -> u16 {
        self.pending = false;
        let mut s = 0;
        match self.fifo_status() {
            (true, _) => s |= status::FIFO_EMPTY,
            (false, true) => s |= status::FIFO_FULL,
            (false, false) => {}
        }
        if self.pal {
            s |= status::PAL;
        }
        // VBLANK reads as 1 while the display is disabled (GPGX vdp_68k_ctrl_r).
        if self.in_vblank() || !self.regs.display_enabled() {
            s |= status::VBLANK;
        }
        if self.vint_pending {
            s |= status::VINT_PENDING;
        }
        if self.dma_busy() {
            s |= status::DMA_BUSY;
        }
        // Sprite overflow and collision are cleared by reading the status
        // (GPGX vdp_68k_ctrl_r: `status &= 0xFF9F`).
        if std::mem::take(&mut self.sprite_state.overflow) {
            s |= status::SPRITE_OVERFLOW;
        }
        if std::mem::take(&mut self.sprite_state.collision) {
            s |= status::SPRITE_COLLISION;
        }
        s
    }

    pub fn read_hv_counter(&mut self) -> u16 {
        self.hv_counter()
    }

    /// A VRAM byte written at `addr` also lands in the internal sprite table
    /// copy when it falls inside the current table (GPGX vdp_bus_w,
    /// vdp_dma_fill, vdp_dma_copy). Changing register 5 does not reload it.
    pub(crate) fn sat_write(&mut self, addr: u16, byte: u8) {
        let (base_mask, index_mask) = self.sat_masks();
        if addr & base_mask == self.regs.sprite_table_base() {
            self.sat_cache[(addr & index_mask) as usize] = byte;
        }
    }

    pub(crate) fn sat_masks(&self) -> (u16, u16) {
        if self.regs.h40() {
            SAT_MASKS_H40
        } else {
            SAT_MASKS_H32
        }
    }

    fn advance(&mut self) {
        self.addr = self.addr.wrapping_add(self.regs.auto_increment());
    }

    fn cram_index(addr: u16) -> usize {
        ((addr >> 1) as usize) & (CRAM_ENTRIES - 1)
    }

    fn vsram_index(addr: u16) -> usize {
        ((addr >> 1) as usize) & (VSRAM_ENTRIES - 1)
    }

    // --- scheduler and inspection ------------------------------------------

    /// Position of the beam inside the current line, in master cycles.
    pub fn set_line_cycle(&mut self, cycle: u32) {
        self.line_cycle = cycle;
        self.run_dma(cycle);
    }

    pub fn is_pal(&self) -> bool {
        self.pal
    }

    pub fn registers(&self) -> &Registers {
        &self.regs
    }

    pub fn vram(&self) -> &[u8] {
        &self.vram[..]
    }

    pub fn cram(&self) -> &[u16] {
        &self.cram
    }

    pub fn vsram(&self) -> &[u16] {
        &self.vsram
    }

    /// RGBA8888 pixels (0xRRGGBBAA), FRAME_WIDTH per row; only the first
    /// `registers().screen_width()` columns are meaningful.
    /// The framebuffer: rows of `FRAME_WIDTH` pixels, `MAX_FRAME_HEIGHT`
    /// rows, of which the first `active_lines()` belong to the picture.
    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer
    }

    /// Active ou désactive l'enregistrement de l'origine des pixels (utile aux rendus en haute résolution).
    pub fn set_provenance(&mut self, on: bool) {
        self.prov_enabled = on;
    }

    /// VRAM et CRAM tels qu'ils étaient à la dernière ligne affichée de l'image (vides si l'enregistrement est inactif).
    pub fn provenance_snapshot(&self) -> (&[u8], &[u16]) {
        (&self.prov_snapshot.0, &self.prov_snapshot.1)
    }

    /// Origine de chaque pixel de `framebuffer()` (même disposition) ; voir `render::provenance` pour le format.
    pub fn provenance(&self) -> &[u32] {
        &self.prov
    }

    /// Active lines (picture height) of the current frame: 224, or 240 in V30.
    pub fn active_lines(&self) -> u16 {
        self.active_lines
    }
}

/// Everything but the per-line scratch buffers, rebuilt on every line. The
/// framebuffer is kept so the picture shown right after a load is the one
/// that was on screen when saving.
impl savestate::State for Vdp {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"VDP ", |v| {
            v.field(&mut self.pal)?;
            v.state(&mut self.regs)?;
            v.field(&mut self.vram)?;
            v.field(&mut self.cram)?;
            v.field(&mut self.vsram)?;
            v.field(&mut self.pending)?;
            v.field(&mut self.code)?;
            v.field(&mut self.addr)?;
            v.field(&mut self.line)?;
            v.field(&mut self.active_lines)?;
            v.field(&mut self.line_cycle)?;
            v.field(&mut self.hint_counter)?;
            v.field(&mut self.hint_pending)?;
            v.field(&mut self.vint_pending)?;
            v.field(&mut self.framebuffer[..])?;
            v.field(&mut self.sprite_state.mask_carry)?;
            v.field(&mut self.sprite_state.overflow)?;
            v.field(&mut self.sprite_state.collision)?;
            v.field(&mut self.fill_pending)?;
            v.field(&mut self.dma_request)?;
            v.field(&mut self.dma_progress)?;
            v.field(&mut self.fifo)?;
            v.field(&mut self.sat_cache)?;
            self.palette.dirty = true; // derived from CRAM, not saved
            v.field(&mut self.fifo_stall)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VDP in mode 5 with an auto-increment of 2, as games set it up.
    fn vdp() -> Vdp {
        let mut v = Vdp::new(false);
        v.write_control(0x8104); // mode 5, display off
        v.write_control(0x8F02); // auto-increment 2
        v
    }

    fn command(v: &mut Vdp, long: u32) {
        v.write_control((long >> 16) as u16);
        v.write_control(long as u16);
    }

    /// Plutiedev formula: ((addr & $3FFF) << 16) | ((addr & $C000) >> 14) | command.
    fn vram_write(addr: u32) -> u32 {
        ((addr & 0x3FFF) << 16) | ((addr & 0xC000) >> 14) | 0x4000_0000
    }

    #[test]
    fn register_write() {
        let mut v = vdp();
        v.write_control(0x8230);
        assert_eq!(v.registers().get(reg::PLANE_A_BASE), 0x30);
        assert_eq!(v.registers().plane_a_base(), 0xC000);
    }

    #[test]
    fn vram_write_and_read_back_with_autoincrement() {
        let mut v = vdp();
        command(&mut v, vram_write(0xC002));
        v.write_data(0x1234);
        v.write_data(0x5678);
        assert_eq!(&v.vram()[0xC002..0xC006], &[0x12, 0x34, 0x56, 0x78]);
        command(&mut v, 0x0000_0003); // VRAM read at $C000
        assert_eq!(v.read_data(), 0x0000);
        assert_eq!(v.read_data(), 0x1234);
        assert_eq!(v.read_data(), 0x5678);
    }

    #[test]
    fn odd_vram_address_swaps_bytes() {
        let mut v = vdp();
        command(&mut v, vram_write(0x0101));
        v.write_data(0xAABB);
        assert_eq!(&v.vram()[0x100..0x102], &[0xBB, 0xAA]);
    }

    #[test]
    fn cram_keeps_nine_bits_and_vsram_words() {
        let mut v = vdp();
        command(&mut v, 0xC002_0000); // CRAM write, index 1
        v.write_data(0xFFFF);
        assert_eq!(v.cram()[1], 0x0EEE);
        command(&mut v, 0x4000_0010); // VSRAM write, index 0
        v.write_data(0x0123);
        assert_eq!(v.vsram()[0], 0x0123);
        command(&mut v, 0x0002_0020); // CRAM read, index 1
        assert_eq!(v.read_data(), 0x0EEE);
    }

    #[test]
    fn status_read_cancels_pending_command() {
        let mut v = vdp();
        v.write_control(0x4000); // first half of a VRAM write at $0000
        v.read_status();
        // Not taken as the second word: this is a register write.
        v.write_control(0x8F04);
        assert_eq!(v.registers().auto_increment(), 4);
    }

    #[test]
    fn no_pending_state_outside_mode5() {
        let mut v = Vdp::new(false);
        v.write_control(0x4000);
        v.write_control(0x8F02); // register write, not a second word
        assert_eq!(v.registers().auto_increment(), 2);
    }

    #[test]
    fn vblank_flag_follows_line_and_display() {
        let mut v = vdp();
        assert_ne!(v.read_status() & status::VBLANK, 0, "display disabled");
        v.write_control(0x8144); // display on
        v.start_line(10);
        assert_eq!(v.read_status() & status::VBLANK, 0);
        v.start_line(ACTIVE_LINES);
        assert_ne!(v.read_status() & status::VBLANK, 0);
    }
}

//! DMA (Plutiedev "DMA transfer"; Genesis Plus GX vdp_ctrl.c `vdp_dma_*`).
//!
//! Started by a command whose CD5 bit is set while DMA is enabled (register
//! 1 bit 4). Register 23 bits 7-6 select the kind:
//! - 0x: 68000 bus → VRAM/CRAM/VSRAM. The VDP cannot read the 68000 bus by
//!   itself: it raises a `DmaRequest` that the machine services word by word
//!   through `dma_write`, then `finish_dma_request`. The source wraps inside
//!   its 128 KiB window.
//! - 10: fill. The next data port write is performed normally, then its high
//!   byte is written `length` times at (address ^ 1) in VRAM (the word
//!   itself for CRAM/VSRAM), the address advancing by the auto-increment.
//! - 11: VRAM copy, byte by byte from (source ^ 1) to (address ^ 1).
//!
//! Fill and copy take time while the 68000 goes on (GPGX vdp_dma_update):
//! they advance with the beam at the access-slot rate of `SLOTS_*` bytes
//! per line (copy: half, one read plus one write per byte), after a 2-slot
//! delay for the fill's initial data write. The DMA busy status bit is set
//! from the fill command (or the copy start) until the last byte. Other port
//! accesses meanwhile are processed normally and share the address register
//! with the running DMA, as in GPGX.

use crate::memory::{CRAM_MASK, Target};
use crate::{Vdp, reg};

/// Register 23: mode bits 7-6 and source bits 23-17 (bits 6-0).
const DMA_MODE_SHIFT: u8 = 6;
const DMA_MODE_FILL: u8 = 0b10;
const DMA_MODE_COPY: u8 = 0b11;
const DMA_SOURCE_HIGH_MASK: u8 = 0x7F;
/// CD5: the command starts a DMA.
const CODE_DMA: u8 = 0x20;
/// 68000 transfers stay inside a 128 KiB source window.
const DMA_WINDOW_MASK: u32 = 0x1_FFFF;
/// Master cycles per line, and DMA access slots per line (GPGX dma_timing):
/// active display H32/H40, then blanking (or display off).
const MASTER_CYCLES_PER_LINE: u32 = 3420;
const SLOTS_ACTIVE: [u32; 2] = [16, 18];
const SLOTS_BLANK: [u32; 2] = [166, 204];
/// 68000 → CRAM/VSRAM while blanking: one more slot lost per refresh slot,
/// 5 in H32 and 6 in H40 (GPGX vdp_dma_update, dma_type 0).
const SLOTS_BLANK_CRAM_VSRAM: [u32; 2] = [161, 198];

/// Fill or copy in progress (GPGX `dma_length` with `dma_type` 2 / 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DmaProgress {
    /// Copy source byte address; `None` for a fill.
    copy_source: Option<u16>,
    /// Fill data word.
    value: u16,
    /// Bytes left.
    remaining: u32,
    /// Line cycle up to which the DMA has run (may exceed the line: start
    /// delay carried into the next line).
    cycle: u32,
    /// Remainder of `rate * cycles / MASTER_CYCLES_PER_LINE`.
    fraction: u32,
}

impl savestate::Field for DmaProgress {
    fn write(&self, w: &mut savestate::Writer) {
        self.copy_source.write(w);
        self.value.write(w);
        self.remaining.write(w);
        self.cycle.write(w);
        self.fraction.write(w);
    }
    fn read(&mut self, r: &mut savestate::Reader) -> savestate::Result {
        self.copy_source.read(r)?;
        self.value.read(r)?;
        self.remaining.read(r)?;
        self.cycle.read(r)?;
        self.fraction.read(r)
    }
}

/// A 68000 → VDP transfer to service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DmaRequest {
    /// Byte address on the 68000 bus.
    pub source: u32,
    /// Number of words.
    pub length: u32,
}

impl savestate::Field for DmaRequest {
    fn write(&self, w: &mut savestate::Writer) {
        self.source.write(w);
        self.length.write(w);
    }
    fn read(&mut self, r: &mut savestate::Reader) -> savestate::Result {
        self.source.read(r)?;
        self.length.read(r)
    }
}

impl DmaRequest {
    /// Source address of word `i` (wrapping inside the 128 KiB window).
    pub fn word_address(&self, i: u32) -> u32 {
        (self.source & !DMA_WINDOW_MASK) | (self.source.wrapping_add(2 * i) & DMA_WINDOW_MASK)
    }
}

impl Vdp {
    /// Called once a command is complete: starts a DMA if CD5 is set.
    pub(crate) fn start_dma(&mut self) {
        if self.code & CODE_DMA == 0 || !self.regs.dma_enabled() {
            return;
        }
        let r23 = self.regs.get(reg::DMA_SOURCE_HI);
        match r23 >> DMA_MODE_SHIFT {
            DMA_MODE_FILL => self.fill_pending = true,
            DMA_MODE_COPY => self.start_progress(Some(self.regs.dma_source_low()), 0, 0),
            _ => {
                let source = (((r23 & DMA_SOURCE_HIGH_MASK) as u32) << 17)
                    | ((self.regs.dma_source_low() as u32) << 1);
                self.dma_request = Some(DmaRequest {
                    source,
                    length: self.regs.dma_length(),
                });
            }
        }
    }

    /// Pending 68000 → VDP transfer, if any (taken once).
    pub fn take_dma_request(&mut self) -> Option<DmaRequest> {
        self.dma_request.take()
    }

    /// One word of a 68000 → VDP transfer.
    pub fn dma_write(&mut self, word: u16) {
        self.write_data_word(word);
    }

    pub fn finish_dma_request(&mut self) {
        self.regs.finish_dma();
    }

    /// Access slots in the current line: active display or blanking (VBLANK
    /// or display disabled), H32 or H40.
    fn line_slots(&self) -> u32 {
        let h40 = self.regs.h40() as usize;
        if self.in_vblank() || !self.regs.display_enabled() {
            SLOTS_BLANK[h40]
        } else {
            SLOTS_ACTIVE[h40]
        }
    }

    /// Master cycles one transferred word costs the halted 68000: VRAM words
    /// take two access slots, CRAM/VSRAM words one (with fewer usable slots
    /// while blanking).
    pub fn dma_master_cycles_per_word(&self) -> u32 {
        let slots = self.line_slots();
        if Target::from_code(self.code) == Target::VramWrite {
            return MASTER_CYCLES_PER_LINE * 2 / slots;
        }
        let h40 = self.regs.h40() as usize;
        let slots = if slots == SLOTS_BLANK[h40] {
            SLOTS_BLANK_CRAM_VSRAM[h40]
        } else {
            slots
        };
        MASTER_CYCLES_PER_LINE / slots
    }

    /// Fill or copy in progress (or a fill armed, waiting for its data).
    pub fn dma_busy(&self) -> bool {
        self.fill_pending || self.dma_progress.is_some()
    }

    /// The data write that triggers an armed fill has just been performed.
    pub(crate) fn dma_fill(&mut self, value: u16) {
        // GPGX vdp_dma_update: the fill starts 2 slots later, the time of the
        // initial data port write.
        let delay = 2 * (MASTER_CYCLES_PER_LINE / self.line_slots());
        self.start_progress(None, value, delay);
    }

    fn start_progress(&mut self, copy_source: Option<u16>, value: u16, delay: u32) {
        self.dma_progress = Some(DmaProgress {
            copy_source,
            value,
            remaining: self.regs.dma_length(),
            cycle: self.line_cycle + delay,
            fraction: 0,
        });
        self.run_dma(self.line_cycle);
    }

    /// Runs the fill or copy in progress up to master cycle `now` of the
    /// current line.
    pub(crate) fn run_dma(&mut self, now: u32) {
        let Some(mut dma) = self.dma_progress else {
            return;
        };
        if now <= dma.cycle {
            return;
        }
        // Copy: one read and one write per byte (GPGX `rate >> (dma_type & 1)`).
        let rate = self.line_slots() >> dma.copy_source.is_some() as u32;
        let total = dma.fraction + (now - dma.cycle) * rate;
        let bytes = (total / MASTER_CYCLES_PER_LINE).min(dma.remaining);
        dma.fraction = total % MASTER_CYCLES_PER_LINE;
        dma.cycle = now;
        for _ in 0..bytes {
            match &mut dma.copy_source {
                Some(source) => {
                    let byte = self.vram[(*source ^ 1) as usize];
                    self.vram[(self.addr ^ 1) as usize] = byte;
                    self.sat_write(self.addr ^ 1, byte);
                    *source = source.wrapping_add(1);
                }
                None => match Target::from_code(self.code) {
                    Target::VramWrite => {
                        let byte = (dma.value >> 8) as u8;
                        self.vram[(self.addr ^ 1) as usize] = byte;
                        self.sat_write(self.addr ^ 1, byte);
                    }
                    Target::CramWrite => {
                        self.cram[Self::cram_index(self.addr)] = dma.value & CRAM_MASK;
                        self.palette.dirty = true;
                    }
                    Target::VsramWrite => self.vsram[Self::vsram_index(self.addr)] = dma.value,
                    _ => {}
                },
            }
            self.advance();
        }
        dma.remaining -= bytes;
        if dma.remaining == 0 {
            self.dma_progress = None;
            self.regs.finish_dma();
        } else {
            self.dma_progress = Some(dma);
        }
    }

    /// End of the current line: the DMA runs to its end, and a start delay
    /// past it carries into the next line.
    pub(crate) fn end_line_dma(&mut self) {
        self.run_dma(MASTER_CYCLES_PER_LINE);
        if let Some(dma) = &mut self.dma_progress {
            dma.cycle = dma.cycle.saturating_sub(MASTER_CYCLES_PER_LINE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(v: &mut Vdp, long: u32) {
        v.write_control((long >> 16) as u16);
        v.write_control(long as u16);
    }

    /// Lets lines go by until the fill or copy in progress is over; returns
    /// the number of line ends it took.
    fn settle(v: &mut Vdp) -> u32 {
        let mut lines = 0;
        while v.dma_busy() {
            v.start_line(v.line.wrapping_add(1) % v.lines_per_frame());
            lines += 1;
            assert!(lines < 10_000, "DMA never ends");
        }
        lines
    }

    /// Fill of `length` bytes at VRAM $0000 with auto-increment 1, display
    /// on or off, in H40 or H32, starting at the beginning of `line`.
    fn start_fill(display: bool, h40: bool, line: u16, length: u32) -> Vdp {
        let mut v = vdp(1);
        v.write_control(if display { 0x8154 } else { 0x8114 });
        v.write_control(if h40 { 0x8C81 } else { 0x8C00 });
        v.start_line(line);
        v.write_control(0x9300 | (length & 0xFF) as u16);
        v.write_control(0x9400 | (length >> 8 & 0xFF) as u16);
        v.write_control(0x9780);
        command(&mut v, 0x4000_0080);
        assert!(v.dma_busy(), "busy from the fill command");
        assert!(v.read_status() & crate::status::DMA_BUSY != 0);
        v.write_data(0x5500);
        v
    }

    /// Bytes written by a fill of $55 (VRAM was zero).
    fn filled(v: &Vdp) -> usize {
        v.vram().iter().filter(|&&b| b == 0x55).count()
    }

    #[test]
    fn fill_rate_follows_the_access_slots() {
        // GPGX dma_timing: bytes per line, H32 / H40, active / blanking.
        for (display, h40, rate) in [
            (false, true, 204),
            (false, false, 166),
            (true, true, 18),
            (true, false, 16),
        ] {
            let mut v = start_fill(display, h40, 0, 0x1000);
            // The initial write sets byte 0; the first fill byte (address 1,
            // written at 1 ^ 1) lands on it again; the 2-slot start delay
            // costs 2 bytes of the first line.
            v.start_line(1);
            assert_eq!(filled(&v), rate - 2, "first line, rate {rate}");
            v.start_line(2);
            assert_eq!(filled(&v), 2 * rate - 2, "second line, rate {rate}");
            assert!(v.dma_busy());
        }
    }

    #[test]
    fn full_vram_fill_takes_321_lines_in_blanking() {
        let mut v = start_fill(false, true, 0, 0); // length 0 = 65536
        let lines = settle(&mut v);
        // 65536 bytes + 2 delay slots at 204 per line.
        assert_eq!(lines, (65536 + 2_u32).div_ceil(204));
        assert_eq!(filled(&v), 0x1_0000, "every byte, the first one included");
        assert_eq!(v.read_status() & crate::status::DMA_BUSY, 0);
        assert_eq!(v.registers().get(reg::DMA_LENGTH_LO), 0);
    }

    #[test]
    fn progress_does_not_depend_on_how_time_is_sliced() {
        let mut coarse = start_fill(false, true, 0, 0x800);
        let mut fine = start_fill(false, true, 0, 0x800);
        for line in 1..4 {
            for c in (0..MASTER_CYCLES_PER_LINE).step_by(7) {
                fine.set_line_cycle(c);
            }
            fine.start_line(line);
            coarse.start_line(line);
            assert_eq!(fine.vram()[..], coarse.vram()[..], "line {line}");
            assert_eq!(fine.dma_progress, coarse.dma_progress);
        }
    }

    #[test]
    fn save_state_in_the_middle_of_a_fill() {
        use savestate::{Reader, State, Writer};
        let mut v = start_fill(false, true, 0, 0x2000);
        for line in 1..10 {
            v.start_line(line);
        }
        assert!(v.dma_busy(), "still filling");
        let mut w = Writer::new();
        v.visit(&mut w).unwrap();
        let bytes = w.into_bytes();
        let mut restored = Vdp::new(false);
        let mut r = Reader::new(&bytes);
        restored.visit(&mut r).unwrap();
        r.finish().unwrap();
        settle(&mut v);
        settle(&mut restored);
        assert_eq!(restored.vram()[..], v.vram()[..]);
        assert_eq!(restored.dma_progress, v.dma_progress);
        assert!(!restored.dma_busy());
    }

    #[test]
    fn copy_runs_at_half_rate() {
        let mut v = vdp(1);
        v.write_control(0x8C81); // H40, display off: 204 slots, 102 copies
        v.start_line(0);
        for w in [0x9300, 0x9410, 0x9500, 0x9600, 0x97C0] {
            v.write_control(w); // copy 4096 bytes from $0000
        }
        command(&mut v, 0x2000_00C2); // VRAM $8000, copy
        assert!(v.dma_busy());
        v.start_line(1);
        assert_eq!(v.dma_progress.unwrap().remaining, 4096 - 102);
        assert_eq!(settle(&mut v), 4096_u32.div_ceil(102) - 1);
    }

    /// Mode 5 with DMA enabled, auto-increment as given.
    fn vdp(increment: u8) -> Vdp {
        let mut v = Vdp::new(false);
        v.write_control(0x8114);
        v.write_control(0x8F00 | increment as u16);
        v
    }

    #[test]
    fn dma_disabled_does_nothing() {
        let mut v = Vdp::new(false);
        v.write_control(0x8104); // DMA off
        v.write_control(0x9301);
        command(&mut v, 0x4000_0080);
        assert!(v.take_dma_request().is_none());
    }

    #[test]
    fn vram_fill_writes_the_high_byte() {
        let mut v = vdp(1);
        for w in [0x9308, 0x9400, 0x9780] {
            v.write_control(w); // length 8, fill
        }
        command(&mut v, 0x4000_0080); // VRAM $0000, DMA
        v.write_data(0xAB00);
        settle(&mut v);
        // GPGX vdp_dma_fill: the word is first written normally ($0000-$0001,
        // address then $0001), then $AB goes to (address ^ 1) for addresses
        // $0001-$0008, i.e. bytes 0, 3, 2, 5, 4, 7, 6, 9. Byte 1 keeps the
        // low byte of the initial write; byte 8 is untouched.
        assert_eq!(
            &v.vram()[0..10],
            &[0xAB, 0x00, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0x00, 0xAB]
        );
        assert_eq!(v.registers().get(reg::DMA_LENGTH_LO), 0, "length cleared");
        assert_eq!(v.registers().get(reg::DMA_SOURCE_LO), 8, "source advanced");
    }

    #[test]
    fn vram_copy() {
        let mut v = vdp(2);
        // Register writes rewrite the address: set them before the command.
        command(&mut v, 0x4000_0000);
        for w in [0x1122, 0x3344] {
            v.write_data(w);
        }
        v.write_control(0x8F01);
        for w in [0x9304, 0x9400, 0x9500, 0x9600, 0x97C0] {
            v.write_control(w); // copy 4 bytes from $0000
        }
        command(&mut v, 0x0100_00C0); // VRAM $0100, copy (CD5 | CD4)
        settle(&mut v);
        assert_eq!(&v.vram()[0x100..0x104], &[0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn m68k_transfer_request() {
        let mut v = vdp(2);
        // 3 words from $FF1000: source registers hold address >> 1 = $7F8800.
        for w in [0x9303, 0x9400, 0x9500, 0x9688, 0x977F] {
            v.write_control(w);
        }
        command(&mut v, 0x4000_0080); // VRAM $0000
        let req = v.take_dma_request().unwrap();
        assert_eq!(
            req,
            DmaRequest {
                source: 0xFF1000,
                length: 3
            }
        );
        for (i, word) in [0x1234, 0x5678, 0x9ABC].into_iter().enumerate() {
            assert_eq!(req.word_address(i as u32), 0xFF1000 + 2 * i as u32);
            v.dma_write(word);
        }
        v.finish_dma_request();
        assert_eq!(&v.vram()[0..6], &[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
        assert!(v.take_dma_request().is_none());
    }

    #[test]
    fn source_wraps_inside_128k_window() {
        let req = DmaRequest {
            source: 0x01_FFFE,
            length: 2,
        };
        assert_eq!(req.word_address(1), 0x00_0000);
    }

    #[test]
    fn dma_bandwidth() {
        let mut v = vdp(2);
        command(&mut v, 0x4000_0000);
        // Display off: blanking rate, H32, VRAM word = 2 slots.
        assert_eq!(v.dma_master_cycles_per_word(), 3420 * 2 / 166);
        // CRAM word while blanking: 5 refresh slots lost in H32.
        command(&mut v, 0xC000_0000);
        assert_eq!(v.dma_master_cycles_per_word(), 3420 / 161);
        v.write_control(0x8C81); // H40: 6 lost
        assert_eq!(v.dma_master_cycles_per_word(), 3420 / 198);
        v.write_control(0x8154); // display on, line 0: active display, no loss
        assert_eq!(v.dma_master_cycles_per_word(), 3420 / 18);
    }
}

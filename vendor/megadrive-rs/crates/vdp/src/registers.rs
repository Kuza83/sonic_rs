//! VDP registers $00-$17 (Plutiedev "VDP registers").

pub const REGISTER_COUNT: usize = 24;

/// Register indices.
pub mod reg {
    pub const MODE_1: usize = 0x00;
    pub const MODE_2: usize = 0x01;
    pub const PLANE_A_BASE: usize = 0x02;
    pub const WINDOW_BASE: usize = 0x03;
    pub const PLANE_B_BASE: usize = 0x04;
    pub const SPRITE_BASE: usize = 0x05;
    pub const BACKGROUND: usize = 0x07;
    pub const HINT_COUNTER: usize = 0x0A;
    pub const MODE_3: usize = 0x0B;
    pub const MODE_4: usize = 0x0C;
    pub const HSCROLL_BASE: usize = 0x0D;
    pub const AUTO_INCREMENT: usize = 0x0F;
    pub const PLANE_SIZE: usize = 0x10;
    pub const WINDOW_H: usize = 0x11;
    pub const WINDOW_V: usize = 0x12;
    pub const DMA_LENGTH_LO: usize = 0x13;
    pub const DMA_LENGTH_HI: usize = 0x14;
    pub const DMA_SOURCE_LO: usize = 0x15;
    pub const DMA_SOURCE_MID: usize = 0x16;
    pub const DMA_SOURCE_HI: usize = 0x17;
}

/// Register 0 bit 2: palette select. Clear, only the low bit of each colour
/// component is used (GPGX vdp_render.c color_update_m5: "Color value is
/// limited to 00X00X00X").
const MODE_1_FULL_PALETTE: u8 = 1 << 2;
/// Register 0 bit 4: horizontal interrupt enable (IE1).
const MODE_1_HINT_ENABLE: u8 = 1 << 4;
/// Register 1 bits.
const MODE_2_DISPLAY: u8 = 1 << 6;
/// Register 1 bit 5: vertical interrupt enable (IE0).
const MODE_2_VINT_ENABLE: u8 = 1 << 5;
/// Register 1 bit 4: DMA enable (M1).
const MODE_2_DMA_ENABLE: u8 = 1 << 4;
/// Mode 5 (Mega Drive mode); in mode 4 the control port has no pending state.
const MODE_2_M5: u8 = 1 << 2;
/// Register 1 bit 3: 240 active lines (V30) instead of 224 (V28).
const MODE_2_V30: u8 = 1 << 3;
/// Register 12: RS0 (bit 7) and RS1 (bit 0) both set select H40.
const MODE_4_H40: u8 = 0x81;
/// Register 12 bit 3: shadow/highlight mode (STE).
const MODE_4_SHADOW_HIGHLIGHT: u8 = 1 << 3;
/// Plane table bases are given in units of $2000 bytes.
const PLANE_BASE_UNIT: u16 = 0x2000;
/// Register 11: VSCR (bit 2) selects 2-cell vertical scrolling.
const MODE_3_VSCROLL_2CELL: u8 = 1 << 2;
/// Register 11: HSCR/LSCR (bits 1-0) select the horizontal scroll mode.
const MODE_3_HSCROLL_MODE: u8 = 0x03;
/// Register 13: hscroll table base in units of $400 bytes (bits 5-0).
const HSCROLL_BASE_UNIT: u16 = 0x400;
const HSCROLL_BASE_MASK: u8 = 0x3F;
/// Line index mask into the hscroll table for each mode (GPGX
/// hscroll_mask_table): full screen, first 8 lines repeated, per cell, per line.
const HSCROLL_LINE_MASKS: [usize; 4] = [0x00, 0x07, 0xF8, 0xFF];
/// Window name table base: register 3 shifted left by 10, WD11 ignored in H40
/// (GPGX vdp_reg_w: `(d << 10) & 0xF000` / `0xF800`).
const WINDOW_BASE_MASK_H40: u16 = 0xF000;
const WINDOW_BASE_MASK_H32: u16 = 0xF800;
/// Registers 17/18: bit 7 = RIGT / DOWN, bits 4-0 = split position.
const WINDOW_SIDE_BIT: u8 = 0x80;
const WINDOW_POSITION_MASK: u8 = 0x1F;
/// Sprite attribute table base: register 5 shifted left by 9, SA9 ignored in
/// H40 (Plutiedev: "In H40 mode, SA9 must be 0").
const SPRITE_BASE_MASK_H40: u16 = 0xFC00;
const SPRITE_BASE_MASK_H32: u16 = 0xFE00;
/// Horizontal split in 16-pixel steps, vertical split in 8-line steps.
const WINDOW_H_UNIT: usize = 16;
const WINDOW_V_UNIT: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct Registers {
    values: [u8; REGISTER_COUNT],
}

impl Registers {
    pub fn get(&self, index: usize) -> u8 {
        self.values[index]
    }

    /// Writes register `index`; indices above $17 do not exist and are ignored.
    pub fn set(&mut self, index: usize, value: u8) {
        if let Some(r) = self.values.get_mut(index) {
            *r = value;
        }
    }

    pub fn display_enabled(&self) -> bool {
        self.values[reg::MODE_2] & MODE_2_DISPLAY != 0
    }

    pub fn full_palette(&self) -> bool {
        self.values[reg::MODE_1] & MODE_1_FULL_PALETTE != 0
    }

    pub fn hint_enabled(&self) -> bool {
        self.values[reg::MODE_1] & MODE_1_HINT_ENABLE != 0
    }

    pub fn vint_enabled(&self) -> bool {
        self.values[reg::MODE_2] & MODE_2_VINT_ENABLE != 0
    }

    pub fn dma_enabled(&self) -> bool {
        self.values[reg::MODE_2] & MODE_2_DMA_ENABLE != 0
    }

    /// DMA length in words (0 means 65536), registers 20:19.
    pub fn dma_length(&self) -> u32 {
        let len = ((self.values[reg::DMA_LENGTH_HI] as u32) << 8)
            | self.values[reg::DMA_LENGTH_LO] as u32;
        if len == 0 { 0x1_0000 } else { len }
    }

    /// DMA source registers 22:21 (word address for 68000 transfers, byte
    /// address for VRAM copy).
    pub fn dma_source_low(&self) -> u16 {
        ((self.values[reg::DMA_SOURCE_MID] as u16) << 8) | self.values[reg::DMA_SOURCE_LO] as u16
    }

    /// End of a DMA: the source registers advance by the length registers
    /// and the length registers clear (GPGX vdp_dma_update).
    pub fn finish_dma(&mut self) {
        let len = ((self.values[reg::DMA_LENGTH_HI] as u16) << 8)
            | self.values[reg::DMA_LENGTH_LO] as u16;
        let end = self.dma_source_low().wrapping_add(len);
        self.values[reg::DMA_SOURCE_LO] = end as u8;
        self.values[reg::DMA_SOURCE_MID] = (end >> 8) as u8;
        self.values[reg::DMA_LENGTH_LO] = 0;
        self.values[reg::DMA_LENGTH_HI] = 0;
    }

    pub fn hint_counter_reload(&self) -> u8 {
        self.values[reg::HINT_COUNTER]
    }

    pub fn mode5(&self) -> bool {
        self.values[reg::MODE_2] & MODE_2_M5 != 0
    }

    /// 240 active lines (V30): register 1 bit 3, in mode 5 only (GPGX
    /// vdp_reg_w: "Mode 5 only").
    pub fn v30(&self) -> bool {
        self.mode5() && self.values[reg::MODE_2] & MODE_2_V30 != 0
    }

    /// H40 (320 pixels) when RS0 and RS1 are both set, H32 (256 pixels) otherwise.
    pub fn h40(&self) -> bool {
        self.values[reg::MODE_4] & MODE_4_H40 == MODE_4_H40
    }

    pub fn shadow_highlight(&self) -> bool {
        self.values[reg::MODE_4] & MODE_4_SHADOW_HIGHLIGHT != 0
    }

    pub fn screen_width(&self) -> usize {
        if self.h40() { 320 } else { 256 }
    }

    pub fn auto_increment(&self) -> u16 {
        self.values[reg::AUTO_INCREMENT] as u16
    }

    /// Plane A name table base: register 2 bits 5-3 (SA15-13).
    pub fn plane_a_base(&self) -> u16 {
        ((self.values[reg::PLANE_A_BASE] >> 3) & 7) as u16 * PLANE_BASE_UNIT
    }

    /// Plane B name table base: register 4 bits 2-0 (SB15-13).
    pub fn plane_b_base(&self) -> u16 {
        (self.values[reg::PLANE_B_BASE] & 7) as u16 * PLANE_BASE_UNIT
    }

    /// Plane size in cells (width, height) from register 16 (HSZ bits 1-0,
    /// VSZ bits 5-4): 00 = 32, 01 = 64, 11 = 128. The invalid 10 encoding is
    /// treated as 32 here (Plutiedev: "will not work as expected").
    pub fn plane_size(&self) -> (usize, usize) {
        let cells = |bits: u8| match bits & 3 {
            1 => 64,
            3 => 128,
            _ => 32,
        };
        let v = self.values[reg::PLANE_SIZE];
        (cells(v), cells(v >> 4))
    }

    /// Line of the hscroll table used for screen line `line`.
    pub fn hscroll_line(&self, line: usize) -> usize {
        line & HSCROLL_LINE_MASKS[(self.values[reg::MODE_3] & MODE_3_HSCROLL_MODE) as usize]
    }

    pub fn hscroll_base(&self) -> u16 {
        (self.values[reg::HSCROLL_BASE] & HSCROLL_BASE_MASK) as u16 * HSCROLL_BASE_UNIT
    }

    pub fn vscroll_2cell(&self) -> bool {
        self.values[reg::MODE_3] & MODE_3_VSCROLL_2CELL != 0
    }

    pub fn window_base(&self) -> u16 {
        let mask = if self.h40() {
            WINDOW_BASE_MASK_H40
        } else {
            WINDOW_BASE_MASK_H32
        };
        ((self.values[reg::WINDOW_BASE] as u16) << 10) & mask
    }

    pub fn sprite_table_base(&self) -> u16 {
        let mask = if self.h40() {
            SPRITE_BASE_MASK_H40
        } else {
            SPRITE_BASE_MASK_H32
        };
        ((self.values[reg::SPRITE_BASE] as u16) << 9) & mask
    }

    /// Cells per row of the window name table: 64 in H40, 32 in H32.
    pub fn window_row_cells(&self) -> usize {
        if self.h40() { 64 } else { 32 }
    }

    /// Whether the window replaces plane A at screen pixel (`x`, `line`)
    /// (GPGX render_bg_m5 / window_clip): a line is all window when DOWN
    /// equals (line ≥ split), otherwise the horizontal split decides.
    pub fn window_at(&self, x: usize, line: usize) -> bool {
        let v = self.values[reg::WINDOW_V];
        let down = v & WINDOW_SIDE_BIT != 0;
        if down == (line >= (v & WINDOW_POSITION_MASK) as usize * WINDOW_V_UNIT) {
            return true;
        }
        let h = self.values[reg::WINDOW_H];
        let split = (h & WINDOW_POSITION_MASK) as usize * WINDOW_H_UNIT;
        if h & WINDOW_SIDE_BIT != 0 {
            x >= split
        } else {
            x < split
        }
    }

    /// Background colour: CRAM index from palette (bits 5-4) and entry (bits 3-0).
    pub fn background_index(&self) -> usize {
        (self.values[reg::BACKGROUND] & 0x3F) as usize
    }
}

impl savestate::State for Registers {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.field(&mut self.values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_fields() {
        let mut r = Registers::default();
        r.set(reg::PLANE_A_BASE, 0x30); // $C000
        r.set(reg::PLANE_B_BASE, 0x07); // $E000
        r.set(reg::MODE_4, 0x81);
        r.set(reg::PLANE_SIZE, 0x01);
        r.set(reg::BACKGROUND, 0x2A);
        assert_eq!(r.plane_a_base(), 0xC000);
        assert_eq!(r.plane_b_base(), 0xE000);
        assert!(r.h40());
        assert_eq!(r.screen_width(), 320);
        assert_eq!(r.plane_size(), (64, 32));
        assert_eq!(r.background_index(), 0x2A);
        r.set(0x1F, 0xFF); // nonexistent register: ignored
    }
}

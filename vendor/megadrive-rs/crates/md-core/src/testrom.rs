//! Generated test ROMs: tiny hand-assembled 68000 programs with a known
//! picture, so video tests do not depend on any external ROM (CLAUDE.md
//! rule 7). Each program is cross-checked against the disassembler in tests.

/// Initial SSP and entry point of generated ROMs.
const INITIAL_SSP: u32 = 0x00FF_FE00;
const ENTRY: u32 = 0x200;
const ROM_SIZE: usize = 0x1000;

/// Minimal sequential assembler: emits words and resolves short branches.
#[derive(Default)]
struct Asm {
    words: Vec<u16>,
}

impl Asm {
    fn here(&self) -> u32 {
        ENTRY + 2 * self.words.len() as u32
    }

    fn emit(&mut self, words: &[u16]) {
        self.words.extend_from_slice(words);
    }

    fn long(&mut self, opcode: u16, value: u32) {
        self.emit(&[opcode, (value >> 16) as u16, value as u16]);
    }

    /// Short branch (Bcc.s) to `target`: displacement from the opcode + 2.
    fn branch_short(&mut self, opcode: u16, target: u32) {
        let disp = target.wrapping_sub(self.here() + 2) as i32;
        assert!((-128..=127).contains(&disp) && disp != 0);
        self.emit(&[opcode | (disp as u8 as u16)]);
    }

    /// Fills `count` words of VDP memory with `word`: `move.l #command,(a1)`
    /// then a `dbf` loop of `move.w #word,(a0)` (a0/a1 = VDP data/control).
    fn vdp_fill(&mut self, command: u32, count: u16, word: u16) {
        self.long(MOVE_L_IMM_A1, command);
        self.emit(&[MOVE_W_IMM_D0, count - 1]);
        let lp = self.here();
        self.emit(&[MOVE_W_IMM_A0, word]);
        self.dbf(0, lp);
    }

    /// DBcc Dn to `target`: displacement from the extension word.
    fn dbf(&mut self, reg: u16, target: u32) {
        let disp = target.wrapping_sub(self.here() + 2) as u16;
        self.emit(&[0x51C8 | reg, disp]);
    }
}

// Opcodes used below (encodings: M68000 PRM; checked by the disassembler test).
const MOVE_W_SR: u16 = 0x46FC; // move.w #imm,sr
const LEA_A1_ABS_L: u16 = 0x43F9; // lea (xxx).l,a1
const LEA_A0_ABS_L: u16 = 0x41F9; // lea (xxx).l,a0
const MOVE_W_IMM_A1: u16 = 0x32BC; // move.w #imm,(a1)
const MOVE_L_IMM_A1: u16 = 0x22BC; // move.l #imm,(a1)
const MOVE_W_IMM_A0: u16 = 0x30BC; // move.w #imm,(a0)
const MOVE_W_IMM_D0: u16 = 0x303C; // move.w #imm,d0
const BNE_S: u16 = 0x6600;
const BRA_SELF: u16 = 0x60FE;
const BRA: u16 = 0x6000;

const VDP_CTRL: u32 = 0x00C0_0004;
const VDP_DATA: u32 = 0x00C0_0000;
/// Plane A at $C000, 64×32 cells.
const PLANE_A: u32 = 0xC000;
const PLANE_W: u16 = 64;
const PLANE_H: u16 = 32;

/// VDP command long for a write to `addr` in the space selected by `command`
/// (Plutiedev formula).
fn vdp_command(addr: u32, command: u32) -> u32 {
    ((addr & 0x3FFF) << 16) | ((addr & 0xC000) >> 14) | command
}
const VRAM_WRITE: u32 = 0x4000_0000;
const CRAM_WRITE: u32 = 0xC000_0000;

/// Checkerboard of 8×8 cells: tile 1 (CRAM colour 1, red) where
/// (cell_x + cell_y) is even, tile 2 (colour 2, green) elsewhere; backdrop
/// blue. The program loops forever once the picture is set up.
pub fn checkerboard() -> Vec<u8> {
    checkerboard_with_mode(MODE_2_DISPLAY_V28)
}

/// Register 1 writes that turn the display on: mode 5 with 224 lines (V28)
/// or 240 lines (V30, bit 3).
const MODE_2_DISPLAY_V28: u16 = 0x8144;
const MODE_2_DISPLAY_V30: u16 = 0x814C;

/// `checkerboard()` displayed in V30 (240 lines): plane A's 32 rows cover
/// all of them, so lines 224-239 show the pattern too.
pub fn v30_checkerboard() -> Vec<u8> {
    checkerboard_with_mode(MODE_2_DISPLAY_V30)
}

fn checkerboard_with_mode(display_on: u16) -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A1_ABS_L, VDP_CTRL);
    a.long(LEA_A0_ABS_L, VDP_DATA);
    // Mode 5 display off, auto-increment 2, plane A $C000, H40, 64×32, full
    // palette (register 0 bit 2; backdrop colour 0 is the reset value).
    for reg in [0x8104, 0x8F02, 0x8230, 0x8C81, 0x9001, 0x8004] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    // Palette 0: blue, red, green.
    a.long(MOVE_L_IMM_A1, vdp_command(0, CRAM_WRITE));
    for color in [0x0E00, 0x000E, 0x00E0] {
        a.emit(&[MOVE_W_IMM_A0, color]);
    }
    // Tiles 1 and 2 (32 bytes = 16 words each), filled with colour 1 / 2.
    a.long(MOVE_L_IMM_A1, vdp_command(32, VRAM_WRITE));
    for fill in [0x1111, 0x2222] {
        a.emit(&[0x700F]); // moveq #15,d0
        let lp = a.here();
        a.emit(&[MOVE_W_IMM_A0, fill]);
        a.dbf(0, lp);
    }
    // Plane A name table: entry = 1 + ((x ^ y) & 1).
    a.long(MOVE_L_IMM_A1, vdp_command(PLANE_A, VRAM_WRITE));
    a.emit(&[0x7400]); // moveq #0,d2  (y)
    let y_loop = a.here();
    a.emit(&[0x7600]); // moveq #0,d3  (x)
    let x_loop = a.here();
    a.emit(&[
        0x3803, // move.w d3,d4
        0xB544, // eor.w d2,d4
        0x0244, 0x0001, // andi.w #1,d4
        0x5244, // addq.w #1,d4
        0x3084, // move.w d4,(a0)
        0x5243, // addq.w #1,d3
        0x0C43, PLANE_W, // cmpi.w #64,d3
    ]);
    a.branch_short(BNE_S, x_loop);
    a.emit(&[0x5242, 0x0C42, PLANE_H]); // addq.w #1,d2 ; cmpi.w #32,d2
    a.branch_short(BNE_S, y_loop);
    a.emit(&[MOVE_W_IMM_A1, display_on]); // display on
    a.emit(&[BRA_SELF]);
    build_rom(&a.words)
}

/// Work RAM counters incremented by the interrupt handlers of `interrupts()`.
pub const VINT_COUNTER: u32 = 0x00FF_0000;
pub const HINT_COUNTER: u32 = 0x00FF_0004;
/// H-int every 8 lines (register 10 = 7).
pub const HINT_INTERVAL_LINES: u32 = 8;
/// Autovector table entries: level 4 (H-int) and level 6 (V-int).
const AUTOVECTOR_LEVEL4: usize = 0x70;
const AUTOVECTOR_LEVEL6: usize = 0x78;
const ADDQ_L_1_ABS_L: u16 = 0x52B9; // addq.l #1,(xxx).l
const RTE: u16 = 0x4E73;

/// Enables V-int and H-int (every 8 lines) with handlers counting in RAM,
/// then idles with interrupts unmasked.
pub fn interrupts() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A1_ABS_L, VDP_CTRL);
    // IE1 (H-int), display + IE0 (V-int) + mode 5, H-int every 8 lines.
    for reg in [0x8014, 0x8164, 0x8A07, 0x8F02] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    a.emit(&[MOVE_W_SR, 0x2000]); // unmask interrupts
    a.emit(&[BRA_SELF]);
    let vint_handler = a.here();
    a.long(ADDQ_L_1_ABS_L, VINT_COUNTER);
    a.emit(&[RTE]);
    let hint_handler = a.here();
    a.long(ADDQ_L_1_ABS_L, HINT_COUNTER);
    a.emit(&[RTE]);
    let mut rom = build_rom(&a.words);
    rom[AUTOVECTOR_LEVEL4..AUTOVECTOR_LEVEL4 + 4].copy_from_slice(&hint_handler.to_be_bytes());
    rom[AUTOVECTOR_LEVEL6..AUTOVECTOR_LEVEL6 + 4].copy_from_slice(&vint_handler.to_be_bytes());
    rom
}

/// Where `pad_reader()` stores the 8 bytes read from port 1.
pub const PAD_READS: u32 = 0x00FF_0010;
const PORT1_DATA: u32 = 0x00A1_0003;
const PORT1_CTRL: u32 = 0x00A1_0009;
const LEA_A0_ABS_L_PAD: u16 = 0x41F9; // lea (xxx).l,a0
const LEA_A1_ABS_L_PAD: u16 = 0x43F9; // lea (xxx).l,a1
const MOVE_B_IMM_ABS_L: u16 = 0x13FC; // move.b #imm,(xxx).l
const MOVE_B_IMM_A0: u16 = 0x10BC; // move.b #imm,(a0)
const MOVE_B_A0_A1_POSTINC: u16 = 0x12D0; // move.b (a0),(a1)+
const NOP: u16 = 0x4E71;
/// Delay loop count: well over the 6-button timeout (26 lines × ~488 cycles,
/// 10 cycles per `dbf` iteration).
const PAD_SETTLE_LOOPS: u16 = 4000;

/// Reads port 1 through four TH low/high pulses (TH as an output), storing
/// the 8 bytes at `PAD_READS`, then waits long enough for a 6-button pad to
/// restart its sequence, forever.
pub fn pad_reader() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A0_ABS_L_PAD, PORT1_DATA);
    a.emit(&[MOVE_B_IMM_ABS_L, 0x0040]);
    a.emit(&[(PORT1_CTRL >> 16) as u16, PORT1_CTRL as u16]); // TH as output
    a.emit(&[MOVE_B_IMM_A0, 0x0040]); // TH high
    let main = a.here();
    a.long(LEA_A1_ABS_L_PAD, PAD_READS);
    a.emit(&[0x7403]); // moveq #3,d2
    let pulse = a.here();
    for th in [0x0000, 0x0040] {
        a.emit(&[MOVE_B_IMM_A0, th, NOP, NOP, MOVE_B_A0_A1_POSTINC]);
    }
    a.dbf(2, pulse);
    a.emit(&[0x363C, PAD_SETTLE_LOOPS]); // move.w #n,d3
    let delay = a.here();
    a.dbf(3, delay);
    a.branch_short(BRA, main);
    build_rom(&a.words)
}

const LEA_A2_ABS_L: u16 = 0x45F9; // lea (xxx).l,a2
const MOVE_B_IMM_A2: u16 = 0x14BC; // move.b #imm,(a2)
const MOVE_W_A1_D0: u16 = 0x3011; // move.w (a1),d0
const BTST_IMM_D0: u16 = 0x0800; // btst #n,d0
const BEQ_S: u16 = 0x6700;
/// VDP status bits (Plutiedev "VDP status"): 3 = vertical blanking,
/// 1 = DMA in progress.
const STATUS_VBLANK_BIT: u16 = 3;
const STATUS_DMA_BUSY_BIT: u16 = 1;
/// First cell of the button row of `pad_display()`: row 12, column 4.
pub const PAD_DISPLAY_CELL: (u32, u32) = (4, 12);

/// Shows the buttons held on a 3-button pad in port 1: once per frame, at
/// the start of vertical blanking, the pad is read (TH high: `?1CBRLDU`,
/// TH low: `?0SA00DU`, active low) and the 8 buttons drawn on plane A as
/// white cells, every other column from `PAD_DISPLAY_CELL`, in the order
/// Up Down Left Right B C A Start. A press set before frame n is read during
/// frame n and shown from frame n + 1.
pub fn pad_display() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A1_ABS_L, VDP_CTRL);
    a.long(LEA_A0_ABS_L, VDP_DATA);
    a.long(LEA_A2_ABS_L, PORT1_DATA);
    // Same mode as checkerboard(): display off, plane A $C000, H40, 64×32,
    // full palette.
    for reg in [0x8104, 0x8F02, 0x8230, 0x8C81, 0x9001, 0x8004] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    // Palette 0: dark blue backdrop, white.
    a.long(MOVE_L_IMM_A1, vdp_command(0, CRAM_WRITE));
    for color in [0x0400, 0x0EEE] {
        a.emit(&[MOVE_W_IMM_A0, color]);
    }
    a.vdp_fill(vdp_command(32, VRAM_WRITE), 16, 0x1111); // tile 1: colour 1
    // VRAM is not cleared by every emulator at power-on: clear plane A.
    a.vdp_fill(vdp_command(PLANE_A, VRAM_WRITE), PLANE_W * PLANE_H, 0);
    a.emit(&[MOVE_B_IMM_ABS_L, 0x0040]);
    a.emit(&[(PORT1_CTRL >> 16) as u16, PORT1_CTRL as u16]); // TH as output
    a.emit(&[MOVE_B_IMM_A2, 0x0040]); // TH high
    a.emit(&[MOVE_W_IMM_A1, 0x8144]); // display on
    let main = a.here();
    // Wait for the active display, then for the start of vertical blanking.
    let active = a.here();
    a.emit(&[MOVE_W_A1_D0, BTST_IMM_D0, STATUS_VBLANK_BIT]);
    a.branch_short(BNE_S, active);
    let blank = a.here();
    a.emit(&[MOVE_W_A1_D0, BTST_IMM_D0, STATUS_VBLANK_BIT]);
    a.branch_short(BEQ_S, blank);
    a.emit(&[
        MOVE_B_IMM_A2,
        0x0040,
        NOP,
        NOP,
        0x1212, // move.b (a2),d1: ?1CBRLDU
        MOVE_B_IMM_A2,
        0x0000,
        NOP,
        NOP,
        0x1412, // move.b (a2),d2: ?0SA00DU
        MOVE_B_IMM_A2,
        0x0040,
        0x0201,
        0x003F, // andi.b #$3F,d1
        0x0202,
        0x0030, // andi.b #$30,d2
        0xE50A, // lsl.b #2,d2
        0x8202, // or.b d2,d1: SACBRLDU
        0x4601, // not.b d1: 1 = pressed
    ]);
    let (x, y) = PAD_DISPLAY_CELL;
    a.long(
        MOVE_L_IMM_A1,
        vdp_command(PLANE_A + 2 * (y * PLANE_W as u32 + x), VRAM_WRITE),
    );
    a.emit(&[0x7607]); // moveq #7,d3
    let cell = a.here();
    a.emit(&[
        0x3801, // move.w d1,d4
        0x0244,
        0x0001, // andi.w #1,d4: tile 1 if pressed
        0x3084, // move.w d4,(a0)
        MOVE_W_IMM_A0,
        0x0000, // blank column
        0xE209, // lsr.b #1,d1
    ]);
    a.dbf(3, cell);
    a.branch_short(BRA, main);
    build_rom(&a.words)
}

/// Z80 RAM locations written by the program loaded by `z80_loader()`.
pub const Z80_LOOP_COUNTER: usize = 0x1000;
pub const Z80_VBLANK_COUNTER: usize = 0x1001;
pub const Z80_BANK_READ: usize = 0x1002;
/// ROM byte the Z80 reads through its bank window (bank 0, $8000 + offset).
pub const Z80_MARKER_OFFSET: u16 = 0x0F00;
pub const Z80_MARKER: u8 = 0x5A;
const Z80_CODE_ROM: u32 = 0x0C00;
const Z80_BUSREQ: u32 = 0x00A1_1100;
const Z80_RESET: u32 = 0x00A1_1200;
const Z80_RAM_68K: u32 = 0x00A0_0000;
const MOVE_W_IMM_ABS_L: u16 = 0x33FC; // move.w #imm,(xxx).l
const BTST_0_ABS_L: u16 = 0x0839; // btst #n,(xxx).l
const MOVE_B_A0_POSTINC_A1_POSTINC: u16 = 0x12D8; // move.b (a0)+,(a1)+

/// Z80 program: counts in a loop, counts its IM 1 interrupts, and reads a
/// marker byte of the 68000 ROM through the bank window.
pub fn z80_program() -> Vec<u8> {
    let marker = 0x8000 + Z80_MARKER_OFFSET;
    let mut code = vec![
        0xF3, // di
        0x31,
        0x00,
        0x20, // ld sp,$2000
        0xED,
        0x56, // im 1
        0x3A,
        marker as u8,
        (marker >> 8) as u8, // ld a,(bank window marker)
        0x32,
        Z80_BANK_READ as u8,
        (Z80_BANK_READ >> 8) as u8, // ld ($1002),a
        0xFB,                       // ei
        0x21,
        Z80_LOOP_COUNTER as u8,
        (Z80_LOOP_COUNTER >> 8) as u8, // ld hl,$1000
        0x34,                          // loop: inc (hl)
        0x18,
        0xFD, // jr loop
    ];
    code.resize(0x38, 0x00);
    // The /INT line stays asserted for a whole line (228 T-states): wait
    // longer than that before re-enabling interrupts, as sound drivers do.
    let counter = [Z80_VBLANK_COUNTER as u8, (Z80_VBLANK_COUNTER >> 8) as u8];
    code.extend_from_slice(&[0xF5, 0xC5]); // push af ; push bc
    code.extend_from_slice(&[0x3A, counter[0], counter[1]]); // ld a,($1001)
    code.push(0x3C); // inc a
    code.extend_from_slice(&[0x32, counter[0], counter[1]]); // ld ($1001),a
    code.extend_from_slice(&[0x06, 0x20, 0x10, 0xFE]); // ld b,32 ; djnz $ (32 × 13 T)
    code.extend_from_slice(&[0xC1, 0xF1, 0xFB, 0xC9]); // pop bc ; pop af ; ei ; ret
    code
}

/// The 68000 loads `z80_program()` into Z80 RAM (bus request, wait for the
/// grant, copy, reset pulse) and lets it run.
pub fn z80_loader() -> Vec<u8> {
    let mut rom = load_z80_program(&z80_program());
    rom[Z80_MARKER_OFFSET as usize] = Z80_MARKER;
    rom
}

/// ROM whose 68000 code loads `z80` into Z80 RAM and starts the Z80.
fn load_z80_program(z80: &[u8]) -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    let abs = |a: &mut Asm, value: u16, addr: u32| {
        a.emit(&[MOVE_W_IMM_ABS_L, value, (addr >> 16) as u16, addr as u16]);
    };
    abs(&mut a, 0x0100, Z80_BUSREQ);
    abs(&mut a, 0x0100, Z80_RESET);
    let wait = a.here();
    a.emit(&[
        BTST_0_ABS_L,
        0x0000,
        (Z80_BUSREQ >> 16) as u16,
        Z80_BUSREQ as u16,
    ]);
    a.branch_short(BNE_S, wait);
    a.long(LEA_A0_ABS_L, Z80_CODE_ROM);
    a.long(LEA_A1_ABS_L, Z80_RAM_68K);
    a.emit(&[MOVE_W_IMM_D0, z80.len() as u16 - 1]);
    let copy = a.here();
    a.emit(&[MOVE_B_A0_POSTINC_A1_POSTINC]);
    a.dbf(0, copy);
    abs(&mut a, 0x0000, Z80_RESET);
    abs(&mut a, 0x0000, Z80_BUSREQ);
    abs(&mut a, 0x0100, Z80_RESET);
    a.emit(&[BRA_SELF]);
    assert!(a.here() < Z80_CODE_ROM, "program overlaps the Z80 code");
    let mut rom = build_rom(&a.words);
    rom[Z80_CODE_ROM as usize..Z80_CODE_ROM as usize + z80.len()].copy_from_slice(z80);
    rom
}

/// F-number and block of `ym_tone()`: 53267 Hz × 1083 × 2^(4−1) / 2^20 ≈ 440.1 Hz.
pub const YM_TONE_FNUM: u16 = 0x43B;
pub const YM_TONE_BLOCK: u8 = 4;
/// Register writes of `ym_tone()`: channel 1, algorithm 7, only OP1 audible
/// (TL 0, AR 31, RR 15), other operators silent, key on OP1.
const YM_TONE_REGISTERS: [(u8, u8); 14] = [
    (0x30, 0x01),
    (0x40, 0x00),
    (0x44, 0x7F),
    (0x48, 0x7F),
    (0x4C, 0x7F),
    (0x50, 0x1F),
    (0x60, 0x00),
    (0x70, 0x00),
    (0x80, 0x0F),
    (0xB0, 0x07),
    (0xB4, 0xC0),
    (0xA4, (YM_TONE_BLOCK << 3) | (YM_TONE_FNUM >> 8) as u8),
    (0xA0, YM_TONE_FNUM as u8),
    (0x28, 0x10),
];
const YM_TABLE_Z80: u16 = 0x0040;

/// Z80 sound driver: for each (register, value) pair, waits while the YM2612
/// is busy, writes the address to $4000 and the data to $4001; then idles.
pub fn ym_driver() -> Vec<u8> {
    let table = YM_TABLE_Z80.to_le_bytes();
    let mut code = vec![
        0xF3, // di
        0x31,
        0x00,
        0x20, // ld sp,$2000
        0x21,
        table[0],
        table[1], // ld hl,table
        0x06,
        YM_TONE_REGISTERS.len() as u8, // ld b,count
        0x3A,
        0x00,
        0x40, // wait: ld a,($4000)
        0xCB,
        0x7F, // bit 7,a
        0x20,
        0xF9, // jr nz,wait
        0x7E, // ld a,(hl)
        0x32,
        0x00,
        0x40, // ld ($4000),a
        0x23, // inc hl
        0x7E, // ld a,(hl)
        0x32,
        0x01,
        0x40, // ld ($4001),a
        0x23, // inc hl
        0x10,
        0xED, // djnz wait
        0x18,
        0xFE, // jr $
    ];
    code.resize(YM_TABLE_Z80 as usize, 0);
    for (reg, value) in YM_TONE_REGISTERS {
        code.extend_from_slice(&[reg, value]);
    }
    code
}

/// The 68000 loads `ym_driver()` into the Z80, which plays a sine on the YM2612.
pub fn ym_tone() -> Vec<u8> {
    load_z80_program(&ym_driver())
}

/// PSG tone register value of `psg_tone()` (≈ 440.4 Hz:
/// 3579545 / (32 × 254)).
pub const PSG_TONE_REGISTER: u16 = 254;
const PSG_PORT: u32 = 0x00C0_0011;
const MOVE_B_IMM_ABS: u16 = 0x13FC; // move.b #imm,(xxx).l

/// The 68000 sets PSG channel 0 to `PSG_TONE_REGISTER` at full volume.
pub fn psg_tone() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    let low = 0x80 | (PSG_TONE_REGISTER & 0x0F); // latch channel 0 tone, low 4 bits
    let high = (PSG_TONE_REGISTER >> 4) & 0x3F; // data byte, bits 9-4
    for byte in [low, high, 0x90] {
        a.emit(&[
            MOVE_B_IMM_ABS,
            byte,
            (PSG_PORT >> 16) as u16,
            PSG_PORT as u16,
        ]);
    }
    a.emit(&[BRA_SELF]);
    build_rom(&a.words)
}

/// Wraps a program into a ROM image with vectors and a header.
fn build_rom(program: &[u16]) -> Vec<u8> {
    let mut rom = vec![0u8; ROM_SIZE];
    rom[0..4].copy_from_slice(&INITIAL_SSP.to_be_bytes());
    rom[4..8].copy_from_slice(&ENTRY.to_be_bytes());
    rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    rom[0x150..0x160].copy_from_slice(b"MD-RS TEST ROM  ");
    for (i, w) in program.iter().enumerate() {
        let at = ENTRY as usize + 2 * i;
        rom[at..at + 2].copy_from_slice(&w.to_be_bytes());
    }
    rom
}

/// Work RAM byte where `sram_counter()` reports the boot count.
pub const SRAM_COUNTER_RAM: u32 = 0xFF_0000;
/// SRAM byte holding the count (odd-byte chip at $200001-$20FFFF).
pub const SRAM_COUNTER_ADDR: u32 = 0x20_0001;
const MOVE_B_ABS_L_D0: u16 = 0x1039; // move.b (xxx).l,d0
const ADDQ_B_1_D0: u16 = 0x5200; // addq.b #1,d0
const MOVE_B_D0_ABS_L: u16 = 0x13C0; // move.b d0,(xxx).l

/// Boot counter in SRAM: increments the byte at `SRAM_COUNTER_ADDR` and
/// copies it to `SRAM_COUNTER_RAM`. SRAM powers on as $FF (GPGX), so the
/// first boot reports 0, the next one 1 if the SRAM was kept.
pub fn sram_counter() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(MOVE_B_ABS_L_D0, SRAM_COUNTER_ADDR);
    a.emit(&[ADDQ_B_1_D0]);
    a.long(MOVE_B_D0_ABS_L, SRAM_COUNTER_ADDR);
    a.long(MOVE_B_D0_ABS_L, SRAM_COUNTER_RAM);
    a.emit(&[BRA_SELF]);
    let mut rom = build_rom(&a.words);
    // Header extra memory (Plutiedev "ROM header"): saved, odd bytes.
    rom[0x1B0..0x1BC].copy_from_slice(b"RA\xF8\x20\x00\x20\x00\x01\x00\x20\xFF\xFF");
    rom
}

/// Size of `bank_switcher()`: 5 MiB, ten 512 KiB pages.
pub const BANKED_ROM_SIZE: usize = 0x50_0000;
pub const BANK_PAGE_SIZE: usize = 0x8_0000;
/// Work RAM words where `bank_switcher()` stores what it read.
pub const BANK_RESULTS_RAM: u32 = 0xFF_0000;
const MOVE_W_ABS_L_ABS_L: u16 = 0x33F9; // move.w (xxx).l,(xxx).l

/// Sega mapper test: every 512 KiB page starts with the word $B0nn (n =
/// page). The program reads window 7 ($380000), maps page 9 there through
/// $A130FF, reads it again, then maps page 8 into window 1 ($A130F3) and
/// reads $080000; the three words go to `BANK_RESULTS_RAM`.
pub fn bank_switcher() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    let copy = |a: &mut Asm, from: u32, to: u32| {
        a.emit(&[
            MOVE_W_ABS_L_ABS_L,
            (from >> 16) as u16,
            from as u16,
            (to >> 16) as u16,
            to as u16,
        ]);
    };
    copy(&mut a, 0x38_0000, BANK_RESULTS_RAM);
    a.emit(&[MOVE_B_IMM_ABS_L, 9, 0x00A1, 0x30FF]);
    copy(&mut a, 0x38_0000, BANK_RESULTS_RAM + 2);
    a.emit(&[MOVE_B_IMM_ABS_L, 8, 0x00A1, 0x30F3]);
    copy(&mut a, 0x08_0000, BANK_RESULTS_RAM + 4);
    a.emit(&[BRA_SELF]);
    let mut rom = build_rom(&a.words);
    rom.resize(BANKED_ROM_SIZE, 0);
    for page in 1..BANKED_ROM_SIZE / BANK_PAGE_SIZE {
        let at = page * BANK_PAGE_SIZE;
        rom[at..at + 2].copy_from_slice(&(0xB000 | page as u16).to_be_bytes());
    }
    rom
}

/// ROM addresses of the data uploaded by DMA in `checkerboard_dma()`.
const DMA_PALETTE: u32 = 0x800;
const DMA_TILES: u32 = 0x810;
const DMA_TILE_WORDS: u16 = 32;
const VRAM_DMA: u32 = 0x4000_0080;
const CRAM_DMA: u32 = 0xC000_0080;

/// DMA source registers ($95-$97) for a 68000 byte address.
fn dma_source_regs(addr: u32) -> [u16; 3] {
    let words = addr >> 1;
    [
        0x9500 | (words & 0xFF) as u16,
        0x9600 | ((words >> 8) & 0xFF) as u16,
        0x9700 | ((words >> 16) & 0x7F) as u16,
    ]
}

/// Same picture as `checkerboard()`, but the palette and the tiles are
/// uploaded by 68000 → VDP DMA from ROM, and plane B is cleared by a DMA fill.
pub fn checkerboard_dma() -> Vec<u8> {
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A1_ABS_L, VDP_CTRL);
    a.long(LEA_A0_ABS_L, VDP_DATA);
    // Mode 5 + DMA enabled, display off; plane A $C000, plane B $E000, H40, 64×32,
    // full palette.
    for reg in [0x8114, 0x8F02, 0x8230, 0x8407, 0x8C81, 0x9001, 0x8004] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    // Palette: 3 words to CRAM 0.
    for reg in [0x9303, 0x9400]
        .into_iter()
        .chain(dma_source_regs(DMA_PALETTE))
    {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    a.long(MOVE_L_IMM_A1, vdp_command(0, CRAM_DMA));
    // Tiles 1-2: 32 words to VRAM $0020.
    for reg in [0x9300 | DMA_TILE_WORDS, 0x9400]
        .into_iter()
        .chain(dma_source_regs(DMA_TILES))
    {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    a.long(MOVE_L_IMM_A1, vdp_command(32, VRAM_DMA));
    // Fill plane B (4 KiB at $E000) with 0, byte increment.
    for reg in [0x8F01, 0x9300, 0x9410, 0x9780] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    a.long(MOVE_L_IMM_A1, vdp_command(0xE000, VRAM_DMA));
    a.emit(&[MOVE_W_IMM_A0, 0x0000]);
    // A fill runs while the 68000 goes on: wait for the DMA busy status bit
    // to clear before touching the VDP again (Plutiedev "DMA fill").
    let busy = a.here();
    a.emit(&[MOVE_W_A1_D0, BTST_IMM_D0, STATUS_DMA_BUSY_BIT]);
    a.branch_short(BNE_S, busy);
    a.emit(&[MOVE_W_IMM_A1, 0x8F02]);
    // Plane A name table, as in `checkerboard()`.
    a.long(MOVE_L_IMM_A1, vdp_command(PLANE_A, VRAM_WRITE));
    a.emit(&[0x7400]);
    let y_loop = a.here();
    a.emit(&[0x7600]);
    let x_loop = a.here();
    a.emit(&[
        0x3803, 0xB544, 0x0244, 0x0001, 0x5244, 0x3084, 0x5243, 0x0C43, PLANE_W,
    ]);
    a.branch_short(BNE_S, x_loop);
    a.emit(&[0x5242, 0x0C42, PLANE_H]);
    a.branch_short(BNE_S, y_loop);
    a.emit(&[MOVE_W_IMM_A1, 0x8144]);
    a.emit(&[BRA_SELF]);
    assert!(a.here() < DMA_PALETTE, "program overlaps the DMA data");

    let mut rom = build_rom(&a.words);
    let mut put = |at: u32, w: u16| {
        rom[at as usize..at as usize + 2].copy_from_slice(&w.to_be_bytes());
    };
    for (i, color) in [0x0E00, 0x000E, 0x00E0].into_iter().enumerate() {
        put(DMA_PALETTE + 2 * i as u32, color);
    }
    for i in 0..DMA_TILE_WORDS as u32 {
        put(DMA_TILES + 2 * i, if i < 16 { 0x1111 } else { 0x2222 });
    }
    rom
}

/// Layers of `showcase()` (H40, all positions in screen pixels).
pub mod showcase_layout {
    /// Window covers lines 0-15 (register 18 = $02).
    pub const WINDOW_LINES: usize = 16;
    /// Plane A band: cell row 10 (lines 80-87), cells 0-9, high priority,
    /// scrolled 4 pixels right.
    pub const BAND_TOP: usize = 80;
    pub const BAND_HSCROLL: usize = 4;
    pub const BAND_CELLS: usize = 10;
    /// Sprite 0: 2×2 cells from tile 1 (tiles 1-4, column by column), high
    /// priority, at (100, 100).
    pub const HIGH_SPRITE: (usize, usize) = (100, 100);
    /// Sprite 1: 2×2 cells from tile 4 (only that first cell is opaque,
    /// white), low priority, at (50, 76): partly hidden by the band.
    pub const LOW_SPRITE: (usize, usize) = (50, 76);
}

/// Palette 0 of `showcase()`: backdrop blue, red, green, white, yellow.
pub const SHOWCASE_PALETTE: [u16; 6] = [0x0000, 0x0E00, 0x000E, 0x00E0, 0x0EEE, 0x00EE];

/// Plane B (green), a high-priority hscrolled plane A band (red), a window
/// (yellow) and two 2×2 sprites (one high, one low priority), all set up by
/// the CPU.
pub fn showcase() -> Vec<u8> {
    use showcase_layout::*;
    const PLANE_B: u32 = 0xE000;
    const WINDOW: u32 = 0xB000;
    const SAT: u32 = 0xF000;
    const HSCROLL: u32 = 0xFC00;
    const PRIORITY: u16 = 0x8000;
    let mut a = Asm::default();
    a.emit(&[MOVE_W_SR, 0x2700]);
    a.long(LEA_A1_ABS_L, VDP_CTRL);
    a.long(LEA_A0_ABS_L, VDP_DATA);
    // Mode 5, display off; A $C000, window $B000, B $E000, SAT $F000, H40,
    // hscroll $FC00, auto-increment 2, 64×32, backdrop colour 1, window lines 0-15,
    // full palette.
    for reg in [
        0x8004, 0x8104, 0x8230, 0x832C, 0x8407, 0x8578, 0x8C81, 0x8D3F, 0x8F02, 0x9001, 0x8701,
        0x9202,
    ] {
        a.emit(&[MOVE_W_IMM_A1, reg]);
    }
    a.long(MOVE_L_IMM_A1, vdp_command(0, CRAM_WRITE));
    for color in SHOWCASE_PALETTE {
        a.emit(&[MOVE_W_IMM_A0, color]);
    }
    // Tiles 1-4: solid red, green, yellow, white.
    a.long(MOVE_L_IMM_A1, vdp_command(32, VRAM_WRITE));
    for fill in [0x2222, 0x3333, 0x5555, 0x4444] {
        a.emit(&[MOVE_W_IMM_D0, 15]);
        let lp = a.here();
        a.emit(&[MOVE_W_IMM_A0, fill]);
        a.dbf(0, lp);
    }
    a.vdp_fill(vdp_command(PLANE_B, VRAM_WRITE), 64 * 32, 0x0002);
    a.vdp_fill(vdp_command(WINDOW, VRAM_WRITE), 64 * 2, 0x0003);
    let band = PLANE_A + 2 * 64 * (BAND_TOP as u32 / 8);
    a.vdp_fill(
        vdp_command(band, VRAM_WRITE),
        BAND_CELLS as u16,
        PRIORITY | 1,
    );
    a.long(MOVE_L_IMM_A1, vdp_command(HSCROLL, VRAM_WRITE));
    a.emit(&[MOVE_W_IMM_A0, BAND_HSCROLL as u16]);
    // Sprites: Y, size/link, attributes, X (screen + 128).
    a.long(MOVE_L_IMM_A1, vdp_command(SAT, VRAM_WRITE));
    for word in [
        (HIGH_SPRITE.1 + 128) as u16,
        0x0501,
        PRIORITY | 1,
        (HIGH_SPRITE.0 + 128) as u16,
        (LOW_SPRITE.1 + 128) as u16,
        0x0500,
        4,
        (LOW_SPRITE.0 + 128) as u16,
    ] {
        a.emit(&[MOVE_W_IMM_A0, word]);
    }
    a.emit(&[MOVE_W_IMM_A1, 0x8144]); // display on
    a.emit(&[BRA_SELF]);
    build_rom(&a.words)
}

/// Expected colour index (CRAM entry) of the checkerboard at a pixel.
pub fn checkerboard_index(x: usize, y: usize) -> usize {
    1 + ((x / 8 + y / 8) & 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;
    use crate::debug::disassemble_rom;

    #[test]
    fn interrupts_program_disassembles_as_intended() {
        let cart = Cartridge::from_bytes(&interrupts()).unwrap();
        let text: Vec<String> = disassemble_rom(&cart, ENTRY, 12)
            .into_iter()
            .map(|l| l.text)
            .collect();
        for needle in [
            "move.w #$8014,(a1)",
            "move.w #$8A07,(a1)",
            "move #$2000,sr",
            "bra.s $00021E", // $200 + 4 + 6 + 4 * 4 + 4: branch to itself
            "addq.l #1,($00FF0000).l",
            "rte",
            "addq.l #1,($00FF0004).l",
        ] {
            assert!(
                text.iter().any(|l| l == needle),
                "missing `{needle}` in {text:#?}"
            );
        }
    }

    #[test]
    fn pad_reader_program_disassembles_as_intended() {
        let cart = Cartridge::from_bytes(&pad_reader()).unwrap();
        let text: Vec<String> = disassemble_rom(&cart, ENTRY, 24)
            .into_iter()
            .map(|l| l.text)
            .collect();
        for needle in [
            "lea ($00A10003).l,a0",
            "move.b #$40,($00A10009).l",
            "move.b #$40,(a0)",
            "move.b #$00,(a0)",
            "move.b (a0),(a1)+",
            "moveq #$3,d2",
            "move.w #$0FA0,d3",
        ] {
            assert!(
                text.iter().any(|l| l == needle),
                "missing `{needle}` in {text:#?}"
            );
        }
        assert!(text.iter().any(|l| l.starts_with("bra.s")));
    }

    #[test]
    fn pad_display_program_disassembles_as_intended() {
        let cart = Cartridge::from_bytes(&pad_display()).unwrap();
        let text: Vec<String> = disassemble_rom(&cart, ENTRY, 60)
            .into_iter()
            .map(|l| l.text)
            .collect();
        for needle in [
            "lea ($00A10003).l,a2",
            "move.b #$40,($00A10009).l",
            "move.w (a1),d0",
            "btst #3,d0",
            "move.b (a2),d1",
            "move.b (a2),d2",
            "andi.b #$3F,d1",
            "andi.b #$30,d2",
            "lsl.b #2,d2",
            "or.b d2,d1",
            "not.b d1",
            "move.l #$46080003,(a1)", // plane A $C000 + 2 * (12 * 64 + 4)
            "moveq #$7,d3",
            "lsr.b #1,d1",
        ] {
            assert!(
                text.iter().any(|l| l == needle),
                "missing `{needle}` in {text:#?}"
            );
        }
    }

    #[test]
    fn ym_driver_disassembles_as_intended() {
        let code = ym_driver();
        let mut pc = 0u16;
        let mut text = Vec::new();
        while pc < 0x1E {
            let d = z80::disassemble(pc, |a| code[a as usize]);
            text.push(d.text);
            pc += d.len;
        }
        assert_eq!(
            text,
            [
                "di",
                "ld sp,$2000",
                "ld hl,$0040",
                "ld b,$0E",
                "ld a,($4000)",
                "bit 7,a",
                "jr nz,$0009",
                "ld a,(hl)",
                "ld ($4000),a",
                "inc hl",
                "ld a,(hl)",
                "ld ($4001),a",
                "inc hl",
                "djnz $0009",
                "jr $001C",
            ]
        );
    }

    #[test]
    fn sram_and_bank_programs_disassemble_as_intended() {
        let text = |rom: Vec<u8>, n| -> Vec<String> {
            let cart = Cartridge::from_bytes(&rom).unwrap();
            disassemble_rom(&cart, ENTRY, n)
                .into_iter()
                .map(|l| l.text)
                .collect()
        };
        assert_eq!(
            text(sram_counter(), 6),
            [
                "move #$2700,sr",
                "move.b ($00200001).l,d0",
                "addq.b #1,d0",
                "move.b d0,($00200001).l",
                "move.b d0,($00FF0000).l",
                "bra.s $000218",
            ]
        );
        assert_eq!(
            text(bank_switcher(), 7),
            [
                "move #$2700,sr",
                "move.w ($00380000).l,($00FF0000).l",
                "move.b #$09,($00A130FF).l",
                "move.w ($00380000).l,($00FF0002).l",
                "move.b #$08,($00A130F3).l",
                "move.w ($00080000).l,($00FF0004).l",
                "bra.s $000232",
            ]
        );
    }

    /// The hand-assembled words must mean what the comments say.
    #[test]
    fn checkerboard_program_disassembles_as_intended() {
        let cart = Cartridge::from_bytes(&checkerboard()).unwrap();
        let text: Vec<String> = disassemble_rom(&cart, ENTRY, 40)
            .into_iter()
            .map(|l| l.text)
            .collect();
        let expected_prefix = [
            "move #$2700,sr",
            "lea ($00C00004).l,a1",
            "lea ($00C00000).l,a0",
            "move.w #$8104,(a1)",
        ];
        assert_eq!(&text[..4], &expected_prefix);
        let body: Vec<&str> = text.iter().map(String::as_str).collect();
        for needle in [
            "move.l #$C0000000,(a1)",
            "move.l #$40200000,(a1)",
            "moveq #$F,d0",
            "dbf d0,$000242",
            "move.l #$40000003,(a1)",
            "move.w d3,d4",
            "eor.w d2,d4",
            "andi.w #$0001,d4",
            "addq.w #1,d4",
            "move.w d4,(a0)",
            "cmpi.w #$0040,d3",
            "cmpi.w #$0020,d2",
            "move.w #$8144,(a1)",
        ] {
            assert!(body.contains(&needle), "missing `{needle}` in {body:#?}");
        }
        assert!(body.iter().any(|l| l.starts_with("bne.s")));
    }
}

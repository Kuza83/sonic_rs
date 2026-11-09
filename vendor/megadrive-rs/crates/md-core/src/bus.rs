//! The 68000 system bus: routes every access to its device.
//!
//! Reads from unmapped or unimplemented areas return 0. On hardware they return
//! open-bus data (prefetch) or lock up the 68000 (no /DTACK); this is counted in
//! `unmapped_accesses` instead so tests and debugging can detect it.

use m68k::{ADDRESS_MASK, Bus};

use crate::cartridge::Cartridge;
use crate::io::{Io, MdConfig};
use crate::memory_map::*;
use crate::z80_space::Z80Space;

/// BUSREQ / RESET control bit: bit 0 of the even byte, i.e. bit 8 of the word.
const Z80_CTRL_BYTE_BIT: u8 = 0x01;
const Z80_CTRL_WORD_BIT: u16 = 0x0100;

/// VDP port decoding on address bits 2–4 (GPGX core/mem68k.c vdp_read_word).
mod vdp_port {
    pub const MASK: u32 = 0x1C;
    pub const DATA: u32 = 0x00;
    pub const CONTROL: u32 = 0x04;
    pub const HV_COUNTER: u32 = 0x08;
    pub const HV_COUNTER_MIRROR: u32 = 0x0C;
    pub const PSG: u32 = 0x10;
    pub const PSG_MIRROR: u32 = 0x14;
    pub const UNUSED: u32 = 0x18;
    pub const TEST: u32 = 0x1C;
}

#[derive(Debug, Clone)]
pub struct MdBus {
    cart: Cartridge,
    work_ram: Box<[u8; WORK_RAM_SIZE]>,
    z80: Z80Space,
    io: Io,
    vdp: vdp::Vdp,
    psg: psg::Psg,
    fm: crate::fm::FmUnit,
    /// Current master cycle within the line, set by the scheduler before each
    /// CPU instruction (keeps the YM2612 in step with its accesses).
    now: u32,
    unmapped_accesses: u64,
    /// Master cycles the 68000 spent halted by 68000 → VDP DMA, not yet
    /// charged by the scheduler.
    dma_stall: u32,
}

impl MdBus {
    pub fn new(cart: Cartridge, config: MdConfig) -> Self {
        Self {
            cart,
            work_ram: Box::new([0; WORK_RAM_SIZE]),
            z80: Z80Space::new(),
            io: Io::new(config),
            vdp: vdp::Vdp::new(config.region.is_pal()),
            psg: psg::Psg::new(),
            fm: crate::fm::FmUnit::new(),
            now: 0,
            unmapped_accesses: 0,
            dma_stall: 0,
        }
    }

    /// Master cycles the 68000 was halted since the last call: DMA from its
    /// bus, and waits on a full VDP FIFO.
    pub fn take_dma_stall(&mut self) -> u32 {
        std::mem::take(&mut self.dma_stall) + self.vdp.take_fifo_stall()
    }

    /// Word read by the VDP's 68000 → VDP DMA: cartridge ROM or work RAM.
    /// Other sources (Z80 area, I/O) read 0 here; GPGX models them, we do
    /// not need them yet.
    fn dma_read_word(&self, addr: u32) -> u16 {
        let addr = addr & ADDRESS_MASK;
        match addr {
            ROM_START..=ROM_END => u16::from_be_bytes([self.rom8(addr), self.rom8(addr + 1)]),
            WORK_RAM_START..=WORK_RAM_END => {
                let i = Self::ram_index(addr);
                u16::from_be_bytes([self.work_ram[i], self.work_ram[(i + 1) & 0xFFFF]])
            }
            _ => 0,
        }
    }

    /// Runs a 68000 → VDP DMA requested by the last control word: the 68000
    /// is halted while the VDP reads the bus, which the scheduler charges.
    fn service_dma(&mut self) {
        let Some(request) = self.vdp.take_dma_request() else {
            return;
        };
        let per_word = self.vdp.dma_master_cycles_per_word();
        for i in 0..request.length {
            let word = self.dma_read_word(request.word_address(i));
            self.vdp.dma_write(word);
        }
        self.vdp.finish_dma_request();
        self.dma_stall += request.length * per_word;
    }

    pub fn unmapped_accesses(&self) -> u64 {
        self.unmapped_accesses
    }

    pub fn vdp(&self) -> &vdp::Vdp {
        &self.vdp
    }

    pub fn vdp_mut(&mut self) -> &mut vdp::Vdp {
        &mut self.vdp
    }

    pub fn set_master_time(&mut self, now: u32) {
        self.now = now;
    }

    pub(crate) fn fm(&self) -> &crate::fm::FmUnit {
        &self.fm
    }

    pub(crate) fn fm_mut(&mut self) -> &mut crate::fm::FmUnit {
        &mut self.fm
    }

    /// YM2612 port access from either CPU ($4000-$5FFF in the Z80 area).
    pub(crate) fn ym_read(&mut self, addr: u32) -> u8 {
        self.fm.read(self.now, (addr & 3) as u8)
    }

    pub(crate) fn ym_write(&mut self, addr: u32, value: u8) {
        self.fm.write(self.now, (addr & 3) as u8, value);
    }

    pub fn psg(&self) -> &psg::Psg {
        &self.psg
    }

    pub(crate) fn psg_mut(&mut self) -> &mut psg::Psg {
        &mut self.psg
    }

    /// Reset button (/VRES, GPGX gen_reset(0)): the Z80 is held in reset
    /// with its bus released and bank 0, the YM2612 and the cartridge
    /// mapper are reset. VDP, PSG, work RAM and I/O ports are untouched.
    pub fn soft_reset(&mut self) {
        self.io.set_z80_busreq(false);
        self.io.set_z80_reset_released(false);
        self.z80.reset_bank();
        self.fm.reset_chip();
        self.cart.reset();
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.cart
    }

    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.cart
    }

    /// Direct work RAM write (cheats), outside any CPU access.
    pub fn write_work_ram(&mut self, addr: u32, value: u8) {
        self.work_ram[Self::ram_index(addr)] = value;
    }

    /// 68000 work RAM (64 KiB, mirrored over $E00000-$FFFFFF).
    pub fn work_ram(&self) -> &[u8] {
        &self.work_ram[..]
    }

    pub fn io(&self) -> &Io {
        &self.io
    }

    pub fn io_mut(&mut self) -> &mut Io {
        &mut self.io
    }

    pub fn z80_space(&self) -> &Z80Space {
        &self.z80
    }

    pub(crate) fn z80_space_mut(&mut self) -> &mut Z80Space {
        &mut self.z80
    }

    fn unmapped(&mut self) {
        self.unmapped_accesses += 1;
    }

    fn rom8(&self, addr: u32) -> u8 {
        self.cart.read8(addr)
    }

    fn ram_index(addr: u32) -> usize {
        (addr & WORK_RAM_MASK) as usize
    }

    /// The 68000 sees Z80 space only while it owns the Z80 bus.
    fn z80_accessible(&mut self) -> bool {
        let granted = self.io.z80_bus_granted();
        if !granted {
            self.unmapped();
        }
        granted
    }

    /// Without TMSS unlock the real console locks up; we count it instead.
    fn vdp_accessible(&mut self) -> bool {
        let unlocked = self.io.vdp_unlocked();
        if !unlocked {
            self.unmapped();
        }
        unlocked
    }

    fn z80_ctrl_read_bit(&self, addr: u32) -> bool {
        match addr & !1 {
            Z80_BUSREQ => !self.io.z80_bus_granted(),
            // $A11200 reads are open bus.
            _ => false,
        }
    }

    fn z80_ctrl_write(&mut self, addr: u32, bit: bool) {
        match addr & !0xFF {
            Z80_BUSREQ => self.io.set_z80_busreq(bit),
            Z80_RESET => self.io.set_z80_reset_released(bit),
            _ => unreachable!(),
        }
    }

    fn vdp_read16(&mut self, addr: u32) -> u16 {
        if !self.vdp_accessible() {
            return 0;
        }
        match addr & vdp_port::MASK {
            vdp_port::DATA => self.vdp.read_data(),
            vdp_port::CONTROL => self.vdp.read_status(),
            vdp_port::HV_COUNTER | vdp_port::HV_COUNTER_MIRROR => self.vdp.read_hv_counter(),
            _ => {
                self.unmapped();
                0
            }
        }
    }

    fn vdp_write16(&mut self, addr: u32, value: u16) {
        if !self.vdp_accessible() {
            return;
        }
        match addr & vdp_port::MASK {
            vdp_port::DATA => self.vdp.write_data(value),
            vdp_port::CONTROL => {
                self.vdp.write_control(value);
                self.service_dma();
            }
            // Word writes send the low byte to the PSG (GPGX vdp_write_word).
            vdp_port::PSG | vdp_port::PSG_MIRROR => self.psg.write(value as u8),
            vdp_port::UNUSED | vdp_port::TEST => {}
            _ => self.unmapped(),
        }
    }
}

impl Bus for MdBus {
    fn read8(&mut self, addr: u32) -> u8 {
        let addr = addr & ADDRESS_MASK;
        match addr {
            ROM_START..=ROM_END => self.rom8(addr),
            Z80_AREA_START..=Z80_AREA_END => {
                if !self.z80_accessible() {
                    0
                } else if crate::z80_space::is_ym2612(addr) {
                    self.ym_read(addr)
                } else {
                    self.z80.read8(addr)
                }
            }
            IO_START..=IO_END => self.io.read(addr),
            _ if addr & !0xFF == Z80_BUSREQ || addr & !0xFF == Z80_RESET => {
                // Only the even byte carries the bit; other bits are open bus (0 here).
                if addr & 1 == 0 && self.z80_ctrl_read_bit(addr) {
                    Z80_CTRL_BYTE_BIT
                } else {
                    0
                }
            }
            VDP_START..=VDP_END => {
                // Byte reads ignore address bit 1 (GPGX vdp_read_byte masks with 0xFD).
                let word = self.vdp_read16(addr & !1);
                if addr & 1 == 0 {
                    (word >> 8) as u8
                } else {
                    word as u8
                }
            }
            WORK_RAM_START..=WORK_RAM_END => self.work_ram[Self::ram_index(addr)],
            _ => {
                self.unmapped();
                0
            }
        }
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let addr = addr & ADDRESS_MASK;
        match addr {
            ROM_START..=ROM_END => u16::from_be_bytes([self.rom8(addr), self.rom8(addr + 1)]),
            Z80_AREA_START..=Z80_AREA_END => {
                // Word reads return the byte in both halves (GPGX z80_read_word).
                let byte = self.read8(addr) as u16;
                (byte << 8) | byte
            }
            IO_START..=IO_END => {
                let byte = self.io.read(addr) as u16;
                (byte << 8) | byte
            }
            _ if addr & !0xFF == Z80_BUSREQ || addr & !0xFF == Z80_RESET => {
                if self.z80_ctrl_read_bit(addr) {
                    Z80_CTRL_WORD_BIT
                } else {
                    0
                }
            }
            VDP_START..=VDP_END => self.vdp_read16(addr),
            WORK_RAM_START..=WORK_RAM_END => {
                let i = Self::ram_index(addr);
                u16::from_be_bytes([self.work_ram[i], self.work_ram[i + 1]])
            }
            _ => {
                self.unmapped();
                0
            }
        }
    }

    fn write8(&mut self, addr: u32, value: u8) {
        let addr = addr & ADDRESS_MASK;
        match addr {
            ROM_START..=ROM_END => self.cart.write8(addr, value),
            Z80_AREA_START..=Z80_AREA_END => {
                if !self.z80_accessible() {
                } else if crate::z80_space::is_ym2612(addr) {
                    self.ym_write(addr, value);
                } else {
                    self.z80.write8(addr, value);
                }
            }
            IO_START..=IO_END => {
                if addr & 1 == 1 {
                    self.io.write(addr, value);
                }
            }
            _ if addr & !0xFF == Z80_BUSREQ || addr & !0xFF == Z80_RESET => {
                if addr & 1 == 0 {
                    self.z80_ctrl_write(addr, value & Z80_CTRL_BYTE_BIT != 0);
                }
            }
            TIME_START..=TIME_END => self.cart.write_time(addr, value),
            VDP_START..=VDP_END => match addr & vdp_port::MASK {
                // Byte writes to data/control are duplicated on both halves (GPGX vdp_write_byte).
                vdp_port::DATA | vdp_port::CONTROL | vdp_port::TEST => {
                    self.vdp_write16(addr, u16::from_be_bytes([value, value]))
                }
                // Byte writes reach the PSG on odd addresses only (GPGX vdp_write_byte).
                vdp_port::PSG | vdp_port::PSG_MIRROR => {
                    if addr & 1 == 1 {
                        self.psg.write(value);
                    }
                }
                _ => self.vdp_write16(addr, 0),
            },
            WORK_RAM_START..=WORK_RAM_END => self.work_ram[Self::ram_index(addr)] = value,
            _ => self.unmapped(),
        }
    }

    /// Every Mega Drive interrupt is autovectored; acknowledging tells the
    /// VDP to clear the flag being serviced (GPGX vdp_68k_irq_ack).
    fn interrupt_ack(&mut self, _level: u8) -> Option<u8> {
        self.vdp.acknowledge_interrupt();
        None
    }

    fn write16(&mut self, addr: u32, value: u16) {
        let addr = addr & ADDRESS_MASK;
        match addr {
            ROM_START..=ROM_END => {
                let [high, low] = value.to_be_bytes();
                self.cart.write8(addr, high);
                self.cart.write8(addr | 1, low);
            }
            // Registers sit on odd bytes: a word write reaches them through
            // its low byte (D7-D0).
            TIME_START..=TIME_END => self.cart.write_time(addr | 1, value as u8),
            Z80_AREA_START..=Z80_AREA_END => {
                // Word writes only store the high byte (GPGX z80_write_word).
                self.write8(addr, (value >> 8) as u8);
            }
            IO_START..=IO_END => self.io.write(addr, value as u8),
            _ if addr & !0xFF == Z80_BUSREQ || addr & !0xFF == Z80_RESET => {
                self.z80_ctrl_write(addr, value & Z80_CTRL_WORD_BIT != 0);
            }
            TMSS..=TMSS_END => self.io.write_tmss(addr - TMSS, value),
            VDP_START..=VDP_END => self.vdp_write16(addr, value),
            WORK_RAM_START..=WORK_RAM_END => {
                let i = Self::ram_index(addr);
                self.work_ram[i..i + 2].copy_from_slice(&value.to_be_bytes());
            }
            _ => self.unmapped(),
        }
    }
}

impl savestate::State for MdBus {
    fn visit<V: savestate::Visitor>(&mut self, v: &mut V) -> savestate::Result {
        v.section(b"BUS ", |v| {
            v.field(&mut self.work_ram)?;
            v.state(&mut self.z80)?;
            v.state(&mut self.io)?;
            v.state(&mut self.vdp)?;
            v.state(&mut self.psg)?;
            v.state(&mut self.fm)?;
            v.state(&mut self.cart)?;
            v.field(&mut self.now)?;
            v.field(&mut self.unmapped_accesses)?;
            v.field(&mut self.dma_stall)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_rom;
    use crate::io::Region;

    fn bus_with(config: MdConfig) -> MdBus {
        let mut rom = test_rom::build(0x1000);
        rom[0x400..0x404].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        MdBus::new(Cartridge::from_bytes(&rom).unwrap(), config)
    }

    fn bus() -> MdBus {
        bus_with(MdConfig::default())
    }

    fn grant_z80_bus(bus: &mut MdBus) {
        bus.write16(Z80_RESET, 0x0100);
        bus.write16(Z80_BUSREQ, 0x0100);
    }

    #[test]
    fn rom_reads_are_big_endian() {
        let mut bus = bus();
        assert_eq!(bus.read8(0x400), 0x12);
        assert_eq!(bus.read16(0x400), 0x1234);
        assert_eq!(bus.read32(0x400), 0x1234_5678);
        assert_eq!(bus.read32(0x004), 0x0000_0200);
    }

    #[test]
    fn rom_writes_are_ignored() {
        let mut bus = bus();
        bus.write16(0x400, 0xFFFF);
        bus.write8(0x401, 0xFF);
        assert_eq!(bus.read16(0x400), 0x1234);
    }

    #[test]
    fn rom_beyond_dump_reads_zero() {
        let mut bus = bus();
        assert_eq!(bus.read16(0x2000), 0);
        assert_eq!(bus.read8(ROM_END), 0);
    }

    #[test]
    fn work_ram_is_mirrored_every_64k() {
        let mut bus = bus();
        bus.write32(0xFF_0000, 0xCAFE_BABE);
        assert_eq!(bus.read32(0xE0_0000), 0xCAFE_BABE);
        assert_eq!(bus.read32(0xEF_0000), 0xCAFE_BABE);
        assert_eq!(bus.read8(0xFF_0001), 0xFE);
    }

    #[test]
    fn upper_address_byte_is_ignored() {
        let mut bus = bus();
        bus.write16(0x01FF_0000, 0xBEEF);
        assert_eq!(bus.read16(0xFF_0000), 0xBEEF);
    }

    #[test]
    fn z80_bus_not_granted_at_power_on() {
        let mut bus = bus();
        assert_eq!(
            bus.read16(Z80_BUSREQ) & Z80_CTRL_WORD_BIT,
            Z80_CTRL_WORD_BIT
        );
        // Busreq without releasing reset is still not granted.
        bus.write16(Z80_BUSREQ, 0x0100);
        assert_eq!(bus.read8(Z80_BUSREQ) & Z80_CTRL_BYTE_BIT, Z80_CTRL_BYTE_BIT);
    }

    #[test]
    fn z80_busreq_grant_and_release() {
        let mut bus = bus();
        grant_z80_bus(&mut bus);
        assert_eq!(bus.read16(Z80_BUSREQ) & Z80_CTRL_WORD_BIT, 0);
        assert_eq!(bus.read8(Z80_BUSREQ) & Z80_CTRL_BYTE_BIT, 0);
        bus.write8(Z80_BUSREQ, 0);
        assert_eq!(bus.read8(Z80_BUSREQ) & Z80_CTRL_BYTE_BIT, Z80_CTRL_BYTE_BIT);
    }

    /// $A04000-$A04003 reach the YM2612 (not Z80 RAM) once the bus is
    /// granted: a data write sets the busy flag (32 internal cycles = 1344
    /// master cycles), which clears as time advances.
    #[test]
    fn ym2612_ports_from_the_68000() {
        let mut bus = bus();
        grant_z80_bus(&mut bus);
        bus.write8(0xA0_4000, 0x30);
        bus.write8(0xA0_4001, 0x71);
        // The chip sees the write strobe on its next internal cycles.
        bus.set_master_time(4 * 42);
        assert_eq!(bus.read8(0xA0_4000) & 0x80, 0x80, "busy after the write");
        bus.set_master_time(2000);
        assert_eq!(bus.read8(0xA0_4000) & 0x80, 0, "busy over");
        assert_eq!(bus.z80_space().ram()[0], 0, "Z80 RAM untouched");
    }

    #[test]
    fn z80_ram_needs_bus_grant() {
        let mut bus = bus();
        bus.write8(0xA0_0010, 0x42);
        assert_eq!(bus.unmapped_accesses(), 1);
        grant_z80_bus(&mut bus);
        assert_eq!(bus.read8(0xA0_0010), 0);
        bus.write8(0xA0_0010, 0x42);
        assert_eq!(bus.read8(0xA0_0010), 0x42);
    }

    #[test]
    fn z80_ram_is_mirrored_and_word_access_is_byte_wide() {
        let mut bus = bus();
        grant_z80_bus(&mut bus);
        bus.write8(0xA0_0100, 0x5A);
        assert_eq!(bus.read8(0xA0_2100), 0x5A);
        assert_eq!(bus.read16(0xA0_0100), 0x5A5A);
        bus.write16(0xA0_0200, 0xABCD);
        assert_eq!(bus.read8(0xA0_0200), 0xAB);
        assert_eq!(bus.read8(0xA0_0201), 0x00);
    }

    #[test]
    fn z80_bank_register_shifts_lsb_first() {
        let mut bus = bus();
        grant_z80_bus(&mut bus);
        // Nine single-bit writes, LSB first, select bank 0x101.
        for bit in 0..9 {
            bus.write8(0xA0_6000, ((0x101 >> bit) & 1) as u8);
        }
        assert_eq!(bus.z80_space().bank(), 0x101);
    }

    #[test]
    fn version_register_reflects_config() {
        let cases = [
            (Region::JapanNtsc, false, 0x20),
            (Region::JapanPal, false, 0x60),
            (Region::Usa, false, 0xA0),
            (Region::Europe, true, 0xE1),
        ];
        for (region, tmss, expected) in cases {
            let mut bus = bus_with(MdConfig { region, tmss });
            assert_eq!(bus.read8(0xA1_0001), expected, "{region:?} tmss={tmss}");
            assert_eq!(
                bus.read16(0xA1_0000),
                (expected as u16) << 8 | expected as u16
            );
        }
    }

    #[test]
    fn version_register_is_read_only_and_ports_latch() {
        let mut bus = bus();
        bus.write8(0xA1_0001, 0x00);
        assert_eq!(bus.read8(0xA1_0001), 0xA0);
        bus.write8(0xA1_0009, 0x40); // port A ctrl
        assert_eq!(bus.read8(0xA1_0009), 0x40);
        bus.write16(0xA1_000A, 0x1240); // port B ctrl, low byte
        assert_eq!(bus.read8(0xA1_000B), 0x40);
    }

    /// Sets up mode 5 with auto-increment 2 and a VRAM write at $0000.
    fn vdp_vram_write_at_zero(bus: &mut MdBus) {
        bus.write16(0xC0_0004, 0x8104);
        bus.write16(0xC0_0004, 0x8F02);
        bus.write32(0xC0_0004, 0x4000_0000);
    }

    /// A data write finding the VDP FIFO full halts the 68000: the wait is
    /// reported with the DMA halt the console adds to its time.
    #[test]
    fn full_vdp_fifo_halts_the_68000() {
        let mut bus = bus();
        for w in [0x8144, 0x8C81, 0x8F02] {
            bus.write16(0xC0_0004, w); // display on, H40
        }
        bus.write32(0xC0_0004, 0xC000_0000); // CRAM
        bus.vdp_mut().start_line(10);
        bus.vdp_mut().set_line_cycle(0);
        for _ in 0..4 {
            bus.write16(0xC0_0000, 0);
        }
        assert_eq!(bus.take_dma_stall(), 0);
        bus.write32(0xC0_0000, 0); // two more words
        // First H40 slots at 352 and 820 (GPGX fifo_timing_h40).
        assert_eq!(bus.take_dma_stall(), 357 + (820 - 357_u32).div_ceil(7) * 7);
        assert_eq!(bus.take_dma_stall(), 0, "taken once");
    }

    #[test]
    fn vdp_ports_are_routed() {
        let mut bus = bus();
        bus.write16(0xC0_0004, 0x8144);
        assert_eq!(bus.vdp().registers().get(vdp::reg::MODE_2), 0x44);
        // Mirror of the control port at +2.
        bus.write16(0xC0_0006, 0x8F02);
        assert_eq!(bus.vdp().registers().auto_increment(), 2);
        bus.write32(0xC0_0004, 0x4000_0000);
        bus.write16(0xC0_0000, 0x1234);
        bus.write16(0xC0_0002, 0x5678); // data port mirror
        assert_eq!(&bus.vdp().vram()[0..4], &[0x12, 0x34, 0x56, 0x78]);
        // Display on, active line 0: both writes still wait in the FIFO.
        let status = bus.read16(0xC0_0004);
        assert_eq!(status & vdp::status::FIFO_EMPTY, 0);
        assert_eq!(bus.read8(0xC0_0005), status as u8);
        // Read out by the H40/H32 access slots well before cycle 3000.
        bus.vdp_mut().set_line_cycle(3000);
        let status = bus.read16(0xC0_0004);
        assert_eq!(status & vdp::status::FIFO_EMPTY, vdp::status::FIFO_EMPTY);
    }

    #[test]
    fn vdp_byte_write_is_duplicated() {
        let mut bus = bus();
        vdp_vram_write_at_zero(&mut bus);
        bus.write8(0xC0_0000, 0xAB);
        assert_eq!(&bus.vdp().vram()[0..2], &[0xAB, 0xAB]);
    }

    #[test]
    fn vdp_status_pal_bit_follows_region() {
        let mut bus = bus_with(MdConfig {
            region: Region::Europe,
            tmss: false,
        });
        assert_eq!(bus.read16(0xC0_0004) & vdp::status::PAL, vdp::status::PAL);
    }

    #[test]
    fn tmss_locks_vdp_until_sega_written() {
        let mut bus = bus_with(MdConfig {
            region: Region::Usa,
            tmss: true,
        });
        bus.write16(0xC0_0004, 0x8144);
        assert_eq!(bus.vdp().registers().get(vdp::reg::MODE_2), 0);
        assert_eq!(bus.unmapped_accesses(), 1);
        bus.write32(TMSS, u32::from_be_bytes(*b"SEGA"));
        bus.write16(0xC0_0004, 0x8144);
        assert_eq!(bus.vdp().registers().get(vdp::reg::MODE_2), 0x44);
    }

    #[test]
    fn unmapped_access_reads_zero_and_is_counted() {
        let mut bus = bus();
        assert_eq!(bus.read16(0x80_0000), 0);
        bus.write8(0xB0_0000, 1);
        assert_eq!(bus.unmapped_accesses(), 2);
    }
}

//! System bus as seen by the Z80: 64 KiB memory space and 64 Ki I/O ports.

pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
    /// IN: the full 16-bit port address is on the bus (A15-A8 = B or A).
    fn port_in(&mut self, port: u16) -> u8;
    fn port_out(&mut self, port: u16, value: u8);

    /// Byte on the data bus during an interrupt acknowledge (IM 0 opcode, IM 2
    /// vector low byte). An undriven bus reads $FF.
    fn interrupt_data(&mut self) -> u8 {
        0xFF
    }
}

/// Flat 64 KiB RAM with I/O ports returning their address high byte; for unit tests.
pub struct FlatBus {
    pub mem: Vec<u8>,
}

impl FlatBus {
    pub fn new() -> Self {
        Self {
            mem: vec![0; 0x1_0000],
        }
    }
}

impl Default for FlatBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for FlatBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.mem[addr as usize] = value;
    }

    fn port_in(&mut self, port: u16) -> u8 {
        (port >> 8) as u8
    }

    fn port_out(&mut self, _port: u16, _value: u8) {}
}

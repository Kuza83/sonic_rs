//! Flag computations (Zilog UM0080 for the documented flags; "The
//! Undocumented Z80 Documented" for X/Y, the copies of result bits 3 and 5).

use crate::registers::flag;

pub(crate) fn sz_xy(v: u8) -> u8 {
    let mut f = v & (flag::S | flag::X | flag::Y);
    if v == 0 {
        f |= flag::Z;
    }
    f
}

pub(crate) fn parity(v: u8) -> u8 {
    if v.count_ones().is_multiple_of(2) {
        flag::PV
    } else {
        0
    }
}

/// a + b + carry with every flag.
pub(crate) fn add8(a: u8, b: u8, carry: bool) -> (u8, u8) {
    let wide = a as u16 + b as u16 + carry as u16;
    let r = wide as u8;
    let mut f = sz_xy(r) | ((a ^ b ^ r) & flag::H);
    if (a ^ r) & (b ^ r) & 0x80 != 0 {
        f |= flag::PV;
    }
    if wide > 0xFF {
        f |= flag::C;
    }
    (r, f)
}

/// a - b - carry with every flag.
pub(crate) fn sub8(a: u8, b: u8, carry: bool) -> (u8, u8) {
    let wide = (a as u16).wrapping_sub(b as u16).wrapping_sub(carry as u16);
    let r = wide as u8;
    let mut f = sz_xy(r) | ((a ^ b ^ r) & flag::H) | flag::N;
    if (a ^ b) & (a ^ r) & 0x80 != 0 {
        f |= flag::PV;
    }
    if wide > 0xFF {
        f |= flag::C;
    }
    (r, f)
}

/// ALU operation `op` (ADD ADC SUB SBC AND XOR OR CP) on A and `v`; returns
/// the new A and F.
pub(crate) fn alu8(op: u8, a: u8, v: u8, f: u8) -> (u8, u8) {
    let carry = f & flag::C != 0;
    match op & 7 {
        0 => add8(a, v, false),
        1 => add8(a, v, carry),
        2 => sub8(a, v, false),
        3 => sub8(a, v, carry),
        4 => {
            let r = a & v;
            (r, sz_xy(r) | flag::H | parity(r))
        }
        5 => {
            let r = a ^ v;
            (r, sz_xy(r) | parity(r))
        }
        6 => {
            let r = a | v;
            (r, sz_xy(r) | parity(r))
        }
        _ => {
            // CP: X/Y come from the operand, not from the result.
            let (_, f) = sub8(a, v, false);
            (a, (f & !(flag::X | flag::Y)) | (v & (flag::X | flag::Y)))
        }
    }
}

/// INC r: C unaffected.
pub(crate) fn inc8(v: u8, f: u8) -> (u8, u8) {
    let r = v.wrapping_add(1);
    let mut nf = sz_xy(r) | (f & flag::C);
    if v & 0x0F == 0x0F {
        nf |= flag::H;
    }
    if v == 0x7F {
        nf |= flag::PV;
    }
    (r, nf)
}

/// DEC r: C unaffected.
pub(crate) fn dec8(v: u8, f: u8) -> (u8, u8) {
    let r = v.wrapping_sub(1);
    let mut nf = sz_xy(r) | (f & flag::C) | flag::N;
    if v & 0x0F == 0 {
        nf |= flag::H;
    }
    if v == 0x80 {
        nf |= flag::PV;
    }
    (r, nf)
}

/// ADD HL,rr: H from bit 11, C from bit 15, X/Y from the high byte;
/// S, Z and P/V unaffected.
pub(crate) fn add16(a: u16, b: u16, f: u8) -> (u16, u8) {
    let wide = a as u32 + b as u32;
    let r = wide as u16;
    let mut nf = (f & (flag::S | flag::Z | flag::PV)) | ((r >> 8) as u8 & (flag::X | flag::Y));
    if (a ^ b ^ r) & 0x1000 != 0 {
        nf |= flag::H;
    }
    if wide > 0xFFFF {
        nf |= flag::C;
    }
    (r, nf)
}

/// Rotate/shift `op` (RLC RRC RL RR SLA SRA SLL SRL) of the CB page, with
/// full flags.
pub(crate) fn rot8(op: u8, v: u8, f: u8) -> (u8, u8) {
    let carry_in = f & flag::C;
    let (r, c) = match op & 7 {
        0 => (v.rotate_left(1), v >> 7),
        1 => (v.rotate_right(1), v & 1),
        2 => ((v << 1) | carry_in, v >> 7),
        3 => ((v >> 1) | (carry_in << 7), v & 1),
        4 => (v << 1, v >> 7),
        5 => ((v >> 1) | (v & 0x80), v & 1),
        6 => ((v << 1) | 1, v >> 7),
        _ => (v >> 1, v & 1),
    };
    (r, sz_xy(r) | parity(r) | c)
}

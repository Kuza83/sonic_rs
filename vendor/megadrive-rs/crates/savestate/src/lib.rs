//! Save state format: a hand-written, versioned binary encoding.
//!
//! Every stateful component implements [`State`] with a single `visit`
//! function that lists its fields once. The same function saves (with a
//! [`Writer`]) and loads (with a [`Reader`]), so the two directions cannot
//! disagree on order or content. Components are wrapped in tagged sections
//! (4-byte tag + length) so a missing, extra or mis-sized part is reported
//! instead of silently shifting everything after it.
//!
//! Integers are big-endian; `bool` is one byte (0/1, other values rejected);
//! slices and arrays have a fixed length known by both sides; `Vec` and
//! `VecDeque` carry a 32-bit length (bounded, so a corrupt file cannot
//! request a huge allocation).

use std::collections::VecDeque;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateError {
    #[error("state data ends early")]
    UnexpectedEnd,
    #[error("section {expected:?} expected, found {found:?}")]
    BadTag { expected: String, found: String },
    #[error("section {0:?} has the wrong length")]
    SectionLength(String),
    #[error("invalid value for {0}")]
    InvalidValue(&'static str),
    #[error("not a save state")]
    BadMagic,
    #[error("unsupported save state version {0}")]
    Version(u32),
    #[error("save state belongs to another ROM")]
    RomMismatch,
    #[error("trailing data after the save state")]
    TrailingData,
}

pub type Result<T = ()> = std::result::Result<T, StateError>;

/// Largest `Vec`/`VecDeque` accepted when loading.
const MAX_SEQUENCE_LEN: u32 = 1 << 20;

/// One direction of a state transfer.
pub trait Visitor: Sized {
    /// Saves or loads one field.
    fn field<T: Field + ?Sized>(&mut self, value: &mut T) -> Result;

    /// Wraps the fields visited by `body` in a section tagged `tag`.
    fn section(&mut self, tag: &[u8; 4], body: impl FnOnce(&mut Self) -> Result) -> Result;

    /// Visits a nested component.
    fn state<S: State>(&mut self, component: &mut S) -> Result {
        component.visit(self)
    }
}

/// A component whose state can be saved and restored.
pub trait State {
    fn visit<V: Visitor>(&mut self, v: &mut V) -> Result;
}

/// Encoding of a single value.
pub trait Field {
    fn write(&self, w: &mut Writer);
    fn read(&mut self, r: &mut Reader) -> Result;
}

#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

impl Visitor for Writer {
    fn field<T: Field + ?Sized>(&mut self, value: &mut T) -> Result {
        value.write(self);
        Ok(())
    }

    fn section(&mut self, tag: &[u8; 4], body: impl FnOnce(&mut Self) -> Result) -> Result {
        self.bytes(tag);
        let length_at = self.buf.len();
        self.bytes(&[0; 4]);
        body(self)?;
        let length = (self.buf.len() - length_at - 4) as u32;
        self.buf[length_at..length_at + 4].copy_from_slice(&length.to_be_bytes());
        Ok(())
    }
}

#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(StateError::UnexpectedEnd)?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or(StateError::UnexpectedEnd)?;
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    /// Fails unless every byte has been consumed.
    pub fn finish(&self) -> Result {
        if self.pos == self.data.len() {
            Ok(())
        } else {
            Err(StateError::TrailingData)
        }
    }
}

fn tag_name(tag: &[u8]) -> String {
    String::from_utf8_lossy(tag).into_owned()
}

impl Visitor for Reader<'_> {
    fn field<T: Field + ?Sized>(&mut self, value: &mut T) -> Result {
        value.read(self)
    }

    fn section(&mut self, tag: &[u8; 4], body: impl FnOnce(&mut Self) -> Result) -> Result {
        let found = self.take(4)?;
        if found != tag {
            return Err(StateError::BadTag {
                expected: tag_name(tag),
                found: tag_name(found),
            });
        }
        let length = u32::from_be_bytes(self.array()?) as usize;
        let end = self
            .pos
            .checked_add(length)
            .filter(|&end| end <= self.data.len())
            .ok_or(StateError::UnexpectedEnd)?;
        // The body only sees its own bytes.
        let mut inner = Reader {
            data: &self.data[..end],
            pos: self.pos,
        };
        body(&mut inner)?;
        if inner.pos != end {
            return Err(StateError::SectionLength(tag_name(tag)));
        }
        self.pos = end;
        Ok(())
    }
}

macro_rules! int_field {
    ($($t:ty),*) => {$(
        impl Field for $t {
            fn write(&self, w: &mut Writer) {
                w.bytes(&self.to_be_bytes());
            }
            fn read(&mut self, r: &mut Reader) -> Result {
                *self = <$t>::from_be_bytes(r.array()?);
                Ok(())
            }
        }
    )*};
}
int_field!(u8, u16, u32, u64, i8, i16, i32, i64);

/// `usize` travels as a `u64` (portable between hosts).
impl Field for usize {
    fn write(&self, w: &mut Writer) {
        (*self as u64).write(w);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        let mut v = 0u64;
        v.read(r)?;
        *self = usize::try_from(v).map_err(|_| StateError::InvalidValue("usize"))?;
        Ok(())
    }
}

impl Field for bool {
    fn write(&self, w: &mut Writer) {
        w.bytes(&[*self as u8]);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        *self = match r.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(StateError::InvalidValue("bool")),
        };
        Ok(())
    }
}

impl<T: Field> Field for [T] {
    fn write(&self, w: &mut Writer) {
        for item in self {
            item.write(w);
        }
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        self.iter_mut().try_for_each(|item| item.read(r))
    }
}

impl<T: Field, const N: usize> Field for [T; N] {
    fn write(&self, w: &mut Writer) {
        self[..].write(w);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        self[..].read(r)
    }
}

impl<T: Field + ?Sized> Field for Box<T> {
    fn write(&self, w: &mut Writer) {
        (**self).write(w);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        (**self).read(r)
    }
}

impl<A: Field, B: Field> Field for (A, B) {
    fn write(&self, w: &mut Writer) {
        self.0.write(w);
        self.1.write(w);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        self.0.read(r)?;
        self.1.read(r)
    }
}

fn read_len(r: &mut Reader) -> Result<usize> {
    let mut len = 0u32;
    len.read(r)?;
    if len > MAX_SEQUENCE_LEN {
        return Err(StateError::InvalidValue("sequence length"));
    }
    Ok(len as usize)
}

impl<T: Field + Default> Field for Vec<T> {
    fn write(&self, w: &mut Writer) {
        (self.len() as u32).write(w);
        self[..].write(w);
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        let len = read_len(r)?;
        self.clear();
        for _ in 0..len {
            let mut item = T::default();
            item.read(r)?;
            self.push(item);
        }
        Ok(())
    }
}

impl<T: Field + Default> Field for VecDeque<T> {
    fn write(&self, w: &mut Writer) {
        (self.len() as u32).write(w);
        for item in self {
            item.write(w);
        }
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        let len = read_len(r)?;
        self.clear();
        for _ in 0..len {
            let mut item = T::default();
            item.read(r)?;
            self.push_back(item);
        }
        Ok(())
    }
}

impl<T: Field + Default> Field for Option<T> {
    fn write(&self, w: &mut Writer) {
        self.is_some().write(w);
        if let Some(v) = self {
            v.write(w);
        }
    }
    fn read(&mut self, r: &mut Reader) -> Result {
        let mut present = false;
        present.read(r)?;
        *self = if present {
            let mut v = T::default();
            v.read(r)?;
            Some(v)
        } else {
            None
        };
        Ok(())
    }
}

/// Saves or loads an enum through its numeric code: `to_code` gives the
/// code of the current value, `from_code` rebuilds it (None = invalid).
pub fn enum_field<V: Visitor, E: Copy>(
    v: &mut V,
    value: &mut E,
    what: &'static str,
    to_code: impl Fn(E) -> u8,
    from_code: impl Fn(u8) -> Option<E>,
) -> Result {
    let mut code = to_code(*value);
    v.field(&mut code)?;
    *value = from_code(code).ok_or(StateError::InvalidValue(what))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, Clone)]
    struct Sample {
        a: u16,
        flag: bool,
        table: [i16; 3],
        big: Box<[u8; 5]>,
        queue: VecDeque<(i32, i32)>,
        pending: Option<u32>,
        index: usize,
    }

    impl State for Sample {
        fn visit<V: Visitor>(&mut self, v: &mut V) -> Result {
            v.section(b"SMPL", |v| {
                v.field(&mut self.a)?;
                v.field(&mut self.flag)?;
                v.field(&mut self.table)?;
                v.field(&mut self.big)?;
                v.field(&mut self.queue)?;
                v.field(&mut self.pending)?;
                v.field(&mut self.index)
            })
        }
    }

    fn sample() -> Sample {
        Sample {
            a: 0xBEEF,
            flag: true,
            table: [-1, 2, -3],
            big: Box::new([1, 2, 3, 4, 5]),
            queue: [(1, -1), (7, 8)].into_iter().collect(),
            pending: Some(42),
            index: 3,
        }
    }

    fn save(s: &mut Sample) -> Vec<u8> {
        let mut w = Writer::new();
        s.visit(&mut w).unwrap();
        w.into_bytes()
    }

    #[test]
    fn round_trip_and_layout() {
        let mut original = sample();
        let bytes = save(&mut original);
        assert_eq!(&bytes[..4], b"SMPL");
        assert_eq!(
            u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(&bytes[8..10], &[0xBE, 0xEF], "big-endian");
        let mut loaded = Sample::default();
        let mut r = Reader::new(&bytes);
        loaded.visit(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = save(&mut sample());
        for len in 0..bytes.len() {
            let mut s = Sample::default();
            assert!(
                s.visit(&mut Reader::new(&bytes[..len])).is_err(),
                "length {len}"
            );
        }
    }

    #[test]
    fn corruptions_are_reported() {
        let bytes = save(&mut sample());
        let mut wrong_tag = bytes.clone();
        wrong_tag[0] = b'X';
        assert!(matches!(
            Sample::default().visit(&mut Reader::new(&wrong_tag)),
            Err(StateError::BadTag { .. })
        ));
        let mut bad_bool = bytes.clone();
        bad_bool[10] = 2;
        assert_eq!(
            Sample::default().visit(&mut Reader::new(&bad_bool)),
            Err(StateError::InvalidValue("bool"))
        );
        // Section declared one byte longer than its content.
        let mut long = bytes.clone();
        long[7] += 1;
        long.push(0);
        assert!(matches!(
            Sample::default().visit(&mut Reader::new(&long)),
            Err(StateError::SectionLength(_))
        ));
        // Huge sequence length.
        let queue_at = 8 + 2 + 1 + 6 + 5;
        let mut huge = bytes;
        huge[queue_at..queue_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            Sample::default().visit(&mut Reader::new(&huge)),
            Err(StateError::InvalidValue("sequence length"))
        );
    }

    #[test]
    fn trailing_data_is_detected() {
        let mut bytes = save(&mut sample());
        bytes.push(0);
        let mut r = Reader::new(&bytes);
        Sample::default().visit(&mut r).unwrap();
        assert_eq!(r.finish(), Err(StateError::TrailingData));
    }
}

// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Types and traits involving the deserialization of the generated objects.

use std::fmt;
use std::mem::size_of;

use crate::Identifiable;

const ALLOCATION_BUDGET: usize = 64 * 1024 * 1024;
const VECTOR_ITEM_BUDGET: usize = 1024 * 1024;
pub(crate) const NESTING_BUDGET: usize = 24;

/// Errors that can be produced by [`Cursor`].
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Noncanonical byte-string length or nonzero TL padding.
    InvalidEncoding,
    /// Decoding exceeded its resource budget or a checked allocation failed.
    LimitExceeded,
    /// The end of the buffer was reached earlier than anticipated, which
    /// implies there is not enough data to complete the deserialization.
    UnexpectedEof,

    /// The error type indicating an unexpected constructor was found,
    /// for example, when reading data that doesn't represent the
    /// correct type (e.g. reading a `bool` when we expect a `Vec`).
    /// In particular, it can occur in the following situations:
    ///
    /// * When reading a boolean.
    /// * When reading a boxed vector.
    /// * When reading an arbitrary boxed type.
    ///
    /// It is important to note that unboxed or bare [`types`] lack the
    /// constructor information, and as such they cannot be validated.
    ///
    /// [`types`]: crate::types
    UnexpectedConstructor {
        /// The unexpected constructor identifier.
        id: u32,
    },
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidEncoding => write!(f, "invalid TL encoding"),
            Self::LimitExceeded => write!(f, "TL decode resource limit exceeded"),
            Self::UnexpectedEof => write!(f, "unexpected eof"),
            Self::UnexpectedConstructor { id } => write!(f, "unexpected constructor: {id:08x}"),
        }
    }
}

/// Re-implementation of [`std::io::Cursor`] to only work over in-memory buffers and greatly
/// narrow the possible error cases.
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    remaining_allocation: usize,
    remaining_vector_items: usize,
    remaining_nesting: usize,
}

impl<'a> Cursor<'a> {
    /// Constructs a cursor with the default decoding budgets.
    pub fn from_slice(buf: &'a [u8]) -> Self {
        Self::with_budgets(buf, ALLOCATION_BUDGET, VECTOR_ITEM_BUDGET)
    }

    /// Constructs a cursor with tighter ceilings on allocation bytes and vector elements.
    /// Values above the default budgets are clamped to those defaults.
    pub fn with_budgets(buf: &'a [u8], allocation: usize, vector_items: usize) -> Self {
        Self {
            buf,
            pos: 0,
            remaining_allocation: allocation.min(ALLOCATION_BUDGET),
            remaining_vector_items: vector_items.min(VECTOR_ITEM_BUDGET),
            remaining_nesting: NESTING_BUDGET,
        }
    }

    fn charge_allocation(&mut self, bytes: usize) -> Result<()> {
        self.remaining_allocation = self
            .remaining_allocation
            .checked_sub(bytes)
            .ok_or(Error::LimitExceeded)?;
        Ok(())
    }

    fn with_nesting<T>(&mut self, decode: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.remaining_nesting = self
            .remaining_nesting
            .checked_sub(1)
            .ok_or(Error::LimitExceeded)?;
        let result = decode(self);
        self.remaining_nesting += 1;
        result
    }

    #[cfg(feature = "tl-api")]
    pub(crate) fn read_box<T: Deserializable>(&mut self) -> Result<Box<T>> {
        self.with_nesting(|buf| {
            buf.charge_allocation(size_of::<T>())?;
            Ok(Box::new(T::deserialize(buf)?))
        })
    }

    // TODO not a fan we need to expose this (and a way to create `Cursor`),
    //      but crypto needs it because it needs to know where deserialization
    //      of some inner data ends.
    /// Returns the cursor's position into the slice buffer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Reads a single byte from the slice buffer.
    pub fn read_byte(&mut self) -> Result<u8> {
        if self.pos < self.buf.len() {
            let byte = self.buf[self.pos];
            self.pos += 1;
            Ok(byte)
        } else {
            Err(Error::UnexpectedEof)
        }
    }

    /// Reads an exact amount of bytes to fill the input buffer.
    pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        if self.pos + buf.len() > self.buf.len() {
            Err(Error::UnexpectedEof)
        } else {
            buf.copy_from_slice(&self.buf[self.pos..self.pos + buf.len()]);
            self.pos += buf.len();
            Ok(())
        }
    }

    /// Reads an owned byte field of known length, checking availability and
    /// the decode budget before allocating.
    pub fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let end = self.pos.checked_add(len).ok_or(Error::UnexpectedEof)?;
        let bytes = self.buf.get(self.pos..end).ok_or(Error::UnexpectedEof)?;
        let mut result = Vec::new();
        self.extend_bytes(&mut result, bytes)?;
        self.pos = end;
        Ok(result)
    }

    /// Reads all the way to the end of the slice buffer.
    pub fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let rest = &self.buf[self.pos..];
        self.extend_bytes(buf, rest)?;
        self.pos = self.buf.len();
        Ok(rest.len())
    }

    fn extend_bytes(&mut self, output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
        if bytes.len() > output.capacity() - output.len() {
            let new_capacity = output
                .len()
                .checked_add(bytes.len())
                .ok_or(Error::LimitExceeded)?;
            self.charge_allocation(new_capacity)?;
            output
                .try_reserve_exact(bytes.len())
                .map_err(|_| Error::LimitExceeded)?;
        }
        output.extend_from_slice(bytes);
        Ok(())
    }

    /// Reads the elements of a vector whose length was decoded by the caller.
    /// Nested vectors share the cursor's item and allocation budgets.
    pub fn read_vector<T: Deserializable>(&mut self, len: usize) -> Result<Vec<T>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        self.with_nesting(|buf| buf.read_vector_items(len))
    }

    fn read_vector_items<T: Deserializable>(&mut self, len: usize) -> Result<Vec<T>> {
        let item_size = size_of::<T>();
        let bytes = len.checked_mul(item_size).ok_or(Error::LimitExceeded)?;
        if bytes > self.remaining_allocation {
            return Err(Error::LimitExceeded);
        }
        self.remaining_vector_items = self
            .remaining_vector_items
            .checked_sub(len)
            .ok_or(Error::LimitExceeded)?;

        let mut items = Vec::new();
        for _ in 0..len {
            let value = T::deserialize(self)?;
            if items.len() == items.capacity() {
                // Grow only after another item was decoded. Charge the whole
                // capacity because resize can temporarily hold two allocations.
                let capacity = items.len().saturating_mul(2).max(4).min(len);
                self.charge_allocation(capacity * item_size)?;
                items
                    .try_reserve_exact(capacity - items.len())
                    .map_err(|_| Error::LimitExceeded)?;
            }
            items.push(value);
        }
        Ok(items)
    }
}

/// Alias over a mutable reference to a [`Cursor`].
pub(crate) type Buffer<'a, 'b> = &'a mut Cursor<'b>;

/// A specialized `Result` type for deserialization operations.
pub type Result<T> = std::result::Result<T, Error>;

/// This trait allows for data serialized according to the
/// [Binary Data Serialization] to be deserialized into concrete instances.
///
/// [Binary Data Serialization]: https://core.telegram.org/mtproto/serialize
pub trait Deserializable {
    /// Deserializes an instance of the type from a given buffer.
    fn deserialize(buf: Buffer) -> Result<Self>
    where
        Self: std::marker::Sized;

    /// Convenience function to deserialize an instance from a slice of bytes.
    ///
    /// For functions and concrete types, this will only read their body
    /// (i.e. the constructor identifier should not be present in the buffer.)
    ///
    /// For boxed types, the constructor identifier should be present in
    /// the buffer, so that the correct concrete type can be deserialized.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(bool::from_bytes(&[0x37, 0x97, 0x79, 0xbc]).unwrap(), false);
    /// ```
    fn from_bytes(buf: &[u8]) -> Result<Self>
    where
        Self: std::marker::Sized,
    {
        Self::deserialize(&mut Cursor::from_slice(buf))
    }

    /// Convenience function to deserialize a boxed instance from a slice of bytes.
    ///
    /// Unlike [`Self::from_bytes`], the [`Identifiable::CONSTRUCTOR_ID`] will first
    /// be read from the buffer and then compared against the constructor from self.
    /// The instance will only be returned if the identifiers match.
    fn from_boxed_bytes(buf: &[u8]) -> Result<Self>
    where
        Self: Identifiable + std::marker::Sized,
    {
        let buf = &mut Cursor::from_slice(buf);
        match u32::deserialize(buf) {
            Ok(id) => {
                if id == Self::CONSTRUCTOR_ID {
                    Self::deserialize(buf)
                } else {
                    Err(Error::UnexpectedConstructor { id })
                }
            }
            Err(e) => Err(e),
        }
    }
}

impl Deserializable for bool {
    /// Deserializes a boolean according to the following definitions:
    ///
    /// * `boolFalse#bc799737 = Bool;` deserializes into `false`.
    /// * `boolTrue#997275b5 = Bool;` deserializes into `true`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(bool::from_bytes(&[0xb5, 0x75, 0x72, 0x99]).unwrap(), true);
    /// assert_eq!(bool::from_bytes(&[0x37, 0x97, 0x79, 0xbc]).unwrap(), false);
    /// ```
    #[allow(clippy::unreadable_literal)]
    fn deserialize(buf: Buffer) -> Result<Self> {
        let id = u32::deserialize(buf)?;
        match id {
            0x997275b5u32 => Ok(true),
            0xbc799737u32 => Ok(false),
            _ => Err(Error::UnexpectedConstructor { id }),
        }
    }
}

impl Deserializable for i32 {
    /// Deserializes a 32-bit signed integer according to the following
    /// definition:
    ///
    /// * `int ? = Int;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(i32::from_bytes(&[0x00, 0x00, 0x00, 0x00]).unwrap(), 0i32);
    /// assert_eq!(i32::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap(), 1i32);
    /// assert_eq!(i32::from_bytes(&[0xff, 0xff, 0xff, 0xff]).unwrap(), -1i32);
    /// assert_eq!(i32::from_bytes(&[0xff, 0xff, 0xff, 0x7f]).unwrap(), i32::max_value());
    /// assert_eq!(i32::from_bytes(&[0x00, 0x00, 0x00, 0x80]).unwrap(), i32::min_value());
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 4];
        buf.read_exact(&mut buffer)?;
        Ok(Self::from_le_bytes(buffer))
    }
}

impl Deserializable for u32 {
    /// Deserializes a 32-bit unsigned integer according to the following
    /// definition:
    ///
    /// * `int ? = Int;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(u32::from_bytes(&[0x00, 0x00, 0x00, 0x00]).unwrap(), 0u32);
    /// assert_eq!(u32::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap(), 1u32);
    /// assert_eq!(u32::from_bytes(&[0xff, 0xff, 0xff, 0xff]).unwrap(), u32::max_value());
    /// assert_eq!(u32::from_bytes(&[0x00, 0x00, 0x00, 0x00]).unwrap(), u32::min_value());
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 4];
        buf.read_exact(&mut buffer)?;
        Ok(Self::from_le_bytes(buffer))
    }
}

impl Deserializable for i64 {
    /// Deserializes a 64-bit signed integer according to the following
    /// definition:
    ///
    /// * `long ? = Long;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(i64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]).unwrap(), 0i64);
    /// assert_eq!(i64::from_bytes(&[0x1, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]).unwrap(), 1i64);
    /// assert_eq!(i64::from_bytes(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(), (-1i64));
    /// assert_eq!(i64::from_bytes(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]).unwrap(), i64::max_value());
    /// assert_eq!(i64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x80]).unwrap(), i64::min_value());
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 8];
        buf.read_exact(&mut buffer)?;
        Ok(Self::from_le_bytes(buffer))
    }
}

impl Deserializable for [u8; 16] {
    /// Deserializes the 128-bit integer according to the following
    /// definition:
    ///
    /// * `int128 4*[ int ] = Int128;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    ///
    /// assert_eq!(<[u8; 16]>::from_bytes(&data).unwrap(), data);
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 16];
        buf.read_exact(&mut buffer)?;
        Ok(buffer)
    }
}

impl Deserializable for [u8; 32] {
    /// Deserializes the 128-bit integer according to the following
    /// definition:
    ///
    /// * `int256 8*[ int ] = Int256;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,
    ///             18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32];
    ///
    /// assert_eq!(<[u8; 32]>::from_bytes(&data).unwrap(), data);
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 32];
        buf.read_exact(&mut buffer)?;
        Ok(buffer)
    }
}

impl Deserializable for f64 {
    /// Deserializes a 64-bit floating point according to the
    /// following definition:
    ///
    /// * `double ? = Double;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::f64;
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(f64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]).unwrap(), 0f64);
    /// assert_eq!(f64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xf8, 0x3f]).unwrap(), 1.5f64);
    /// assert_eq!(f64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xf8, 0xbf]).unwrap(), -1.5f64);
    /// assert_eq!(f64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xf0, 0x7f]).unwrap(), f64::INFINITY);
    /// assert_eq!(f64::from_bytes(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xf0, 0xff]).unwrap(), f64::NEG_INFINITY);
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let mut buffer = [0u8; 8];
        buf.read_exact(&mut buffer)?;
        Ok(Self::from_le_bytes(buffer))
    }
}

impl<T: Deserializable> Deserializable for Vec<T> {
    /// Deserializes a vector of deserializable items according to the
    /// following definition:
    ///
    /// * `vector#1cb5c415 {t:Type} # [ t ] = Vector t;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// assert_eq!(Vec::<i32>::from_bytes(&[0x15, 0xc4, 0xb5, 0x1c, 0x0, 0x0, 0x0, 0x0]).unwrap(), Vec::new());
    /// assert_eq!(Vec::<i32>::from_bytes(&[0x15, 0xc4, 0xb5, 0x1c, 0x1, 0x0, 0x0, 0x0, 0x7f, 0x0, 0x0, 0x0]).unwrap(),
    ///            vec![0x7f_i32]);
    /// ```
    #[allow(clippy::unreadable_literal)]
    fn deserialize(buf: Buffer) -> Result<Self> {
        let id = u32::deserialize(buf)?;
        if id != 0x1cb5c415u32 {
            return Err(Error::UnexpectedConstructor { id });
        }
        let len = u32::deserialize(buf)?;
        buf.read_vector(len as usize)
    }
}

impl<T: Deserializable> Deserializable for crate::RawVec<T> {
    /// Deserializes a vector of deserializable items according to the
    /// following definition:
    ///
    /// * `vector#1cb5c415 {t:Type} # [ t ] = Vector t;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::{RawVec, Deserializable};
    ///
    /// assert_eq!(RawVec::<i32>::from_bytes(&[0x0, 0x0, 0x0, 0x0]).unwrap().0, Vec::<i32>::new());
    /// assert_eq!(RawVec::<i32>::from_bytes(&[0x1, 0x0, 0x0, 0x0, 0x7f, 0x0, 0x0, 0x0]).unwrap().0, vec![0x7f_i32]);
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let len = u32::deserialize(buf)?;
        Ok(Self(buf.read_vector(len as usize)?))
    }
}

impl Deserializable for String {
    /// Deserializes a UTF-8 string according to the following definition:
    ///
    /// * `string ? = String;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::Deserializable;
    ///
    /// fn test_string(string: &str, prefix: &[u8], suffix: &[u8]) {
    ///    let bytes = {
    ///        let mut tmp = prefix.to_vec();
    ///        tmp.extend(string.as_bytes());
    ///        tmp.extend(suffix);
    ///        tmp
    ///    };
    ///    let expected = string.to_owned();
    ///
    ///    assert_eq!(String::from_bytes(&bytes).unwrap(), expected);
    /// }
    ///
    /// test_string("", &[0x00], &[0x00, 0x00, 0x00]);
    /// test_string("Hi", &[0x02], &[0x0]);
    /// test_string("Hi!", &[0x03], &[]);
    /// test_string("Hello", &[0x05], &[0x0, 0x0]);
    /// test_string("Hello, world!", &[0xd], &[0x0, 0x0]);
    /// test_string(
    ///     "This is a very long string, and it has to be longer than 253 \
    ///      characters, which are quite a few but we can make it! Although, \
    ///      it is quite challenging. The quick brown fox jumps over the lazy \
    ///      fox. There is still some more text we need to type. Oh, this \
    ///      sentence made it past!",
    ///      &[0xfe, 0x11, 0x01, 0x00],
    ///      &[0x00, 0x00, 0x00]
    /// );
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let bytes = read_byte_string(buf)?;
        let mut text = String::new();
        match std::str::from_utf8(bytes) {
            Ok(utf8) => {
                buf.charge_allocation(utf8.len())?;
                text.try_reserve_exact(utf8.len())
                    .map_err(|_| Error::LimitExceeded)?;
                text.push_str(utf8);
            }
            Err(_) => {
                let bytes_needed = bytes
                    .utf8_chunks()
                    .try_fold(0usize, |length, chunk| {
                        let replacement = if chunk.invalid().is_empty() {
                            0
                        } else {
                            '\u{fffd}'.len_utf8()
                        };
                        length
                            .checked_add(chunk.valid().len())?
                            .checked_add(replacement)
                    })
                    .ok_or(Error::LimitExceeded)?;
                buf.charge_allocation(bytes_needed)?;
                text.try_reserve_exact(bytes_needed)
                    .map_err(|_| Error::LimitExceeded)?;
                for chunk in bytes.utf8_chunks() {
                    text.push_str(chunk.valid());
                    if !chunk.invalid().is_empty() {
                        text.push('\u{fffd}');
                    }
                }
            }
        }
        Ok(text)
    }
}

impl Deserializable for Vec<u8> {
    /// Deserializes a vector of bytes as a byte-string according to the
    /// following definition:
    ///
    /// * `string ? = String;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_tl_types::{Deserializable};
    ///
    /// assert_eq!(Vec::<u8>::from_bytes(&[0x00, 0x00, 0x00, 0x00]).unwrap(), Vec::new());
    /// assert_eq!(Vec::<u8>::from_bytes(&[0x01, 0x7f, 0x00, 0x00]).unwrap(), vec![0x7f_u8]);
    /// ```
    fn deserialize(buf: Buffer) -> Result<Self> {
        let bytes = read_byte_string(buf)?;
        let mut result = Vec::new();
        buf.extend_bytes(&mut result, bytes)?;
        Ok(result)
    }
}

fn read_byte_string<'a>(buf: &mut Cursor<'a>) -> Result<&'a [u8]> {
    let first_byte = buf.read_byte()?;
    let (len, padding) = if first_byte == 254 {
        let mut buffer = [0u8; 3];
        buf.read_exact(&mut buffer)?;
        let len = (buffer[0] as usize) | ((buffer[1] as usize) << 8) | ((buffer[2] as usize) << 16);

        if len < 254 {
            return Err(Error::InvalidEncoding);
        }

        (len, len % 4)
    } else {
        if first_byte == 255 {
            return Err(Error::InvalidEncoding);
        }
        let len = first_byte as usize;
        (len, (len + 1) % 4)
    };

    let padding = (4 - padding) % 4;
    let data_end = buf.pos.checked_add(len).ok_or(Error::UnexpectedEof)?;
    let end = data_end.checked_add(padding).ok_or(Error::UnexpectedEof)?;
    if end > buf.buf.len() {
        return Err(Error::UnexpectedEof);
    }
    if buf.buf[data_end..end].iter().any(|byte| *byte != 0) {
        return Err(Error::InvalidEncoding);
    }
    let data = &buf.buf[buf.pos..data_end];
    buf.pos = end;
    Ok(data)
}

#[cfg(test)]
#[path = "tg_decode_tests.rs"]
mod tg_decode_tests;

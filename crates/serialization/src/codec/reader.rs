// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading a payload back. Every length is treated as a claim and bounds-checked first.

use crate::codec::{ClassId, ENVELOPE_LEN, Envelope, MAX_FIELD_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated {
        wanted: usize,
        remaining: usize,
    },
    /// A class id this build does not know. Refused, never skipped:
    /// skipping one in a limit-state replay silently restores budget.
    UnknownClass {
        found: u16,
    },
    /// A class version newer than this build handles. Same rule.
    UnsupportedVersion {
        class: ClassId,
        found: u16,
        known: u16,
    },
    /// A length field beyond the bound, refused before allocating.
    FieldTooLong {
        found: u32,
    },
    /// Bytes remain after the layout was fully read.
    /// An error, not an allowance: ignoring a tail lets two payloads decode alike.
    TrailingBytes {
        remaining: usize,
    },
    NotUtf8,
    InvalidValue {
        field: &'static str,
        found: u64,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::Truncated { wanted, remaining } => {
                write!(f, "wanted {wanted} bytes, {remaining} remain")
            }
            DecodeError::UnknownClass { found } => write!(f, "unknown event class {found:#06x}"),
            DecodeError::UnsupportedVersion { class, found, known } => {
                write!(f, "{class:?} version {found} is newer than {known}")
            }
            DecodeError::FieldTooLong { found } => write!(f, "field length {found} exceeds bound"),
            DecodeError::TrailingBytes { remaining } => {
                write!(f, "{remaining} bytes remain after the layout")
            }
            DecodeError::NotUtf8 => write!(f, "byte string is not valid UTF-8"),
            DecodeError::InvalidValue { field, found } => {
                write!(f, "field {field} does not admit the value {found}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<(Envelope, Self), DecodeError> {
        if bytes.len() < ENVELOPE_LEN {
            return Err(DecodeError::Truncated { wanted: ENVELOPE_LEN, remaining: bytes.len() });
        }
        let raw = u16::from_be_bytes([bytes[0], bytes[1]]);
        let class = ClassId::from_u16(raw).ok_or(DecodeError::UnknownClass { found: raw })?;
        let version = u16::from_be_bytes([bytes[2], bytes[3]]);
        Ok((Envelope { class, version }, Reader { bytes, at: ENVELOPE_LEN }))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let remaining = self.bytes.len() - self.at;
        if remaining < n {
            return Err(DecodeError::Truncated { wanted: n, remaining });
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_be_bytes(a))
    }

    pub fn i128(&mut self) -> Result<i128, DecodeError> {
        let b = self.take(16)?;
        let mut a = [0u8; 16];
        a.copy_from_slice(b);
        Ok(i128::from_be_bytes(a))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.u32()?;
        if len > MAX_FIELD_LEN {
            return Err(DecodeError::FieldTooLong { found: len });
        }
        self.take(len as usize)
    }

    pub fn str(&mut self) -> Result<&'a str, DecodeError> {
        core::str::from_utf8(self.bytes()?).map_err(|_| DecodeError::NotUtf8)
    }

    /// Whether any byte of this payload is still unread.
    /// Lets a decoder of an older version tell an absent field from real damage.
    #[must_use]
    pub fn more(&self) -> bool {
        self.at < self.bytes.len()
    }

    /// Assert the layout is fully consumed.
    /// Assert the layout is fully consumed. Every class decoder ends with it.
    pub fn finish(self) -> Result<(), DecodeError> {
        let remaining = self.bytes.len() - self.at;
        if remaining == 0 { Ok(()) } else { Err(DecodeError::TrailingBytes { remaining }) }
    }
}

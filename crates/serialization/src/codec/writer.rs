// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Building a payload. The call order is the layout.

use crate::codec::{ClassId, Envelope, MAX_FIELD_LEN};

/// Appends fields to a payload buffer. No `f32` or `f64` method exists: no floating point is an operation not offered.
#[derive(Debug)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub fn new(class: ClassId, version: u16) -> Self {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&class.as_u16().to_be_bytes());
        buf.extend_from_slice(&version.to_be_bytes());
        Self { buf }
    }

    #[must_use]
    pub fn from_envelope(envelope: Envelope) -> Self {
        Self::new(envelope.class, envelope.version)
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// A 128-bit signed integer, which is the platform's money and exposure width.
    pub fn i128(&mut self, v: i128) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// A length-prefixed byte string. Errors rather than truncating: a shortened identifier is a different entity.
    pub fn bytes(&mut self, v: &[u8]) -> Result<&mut Self, EncodeError> {
        let len = u32::try_from(v.len()).map_err(|_| EncodeError::FieldTooLong { len: v.len() })?;
        if len > MAX_FIELD_LEN {
            return Err(EncodeError::FieldTooLong { len: v.len() });
        }
        self.buf.extend_from_slice(&len.to_be_bytes());
        self.buf.extend_from_slice(v);
        Ok(self)
    }

    pub fn str(&mut self, v: &str) -> Result<&mut Self, EncodeError> {
        self.bytes(v.as_bytes())
    }

    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    FieldTooLong { len: usize },
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EncodeError::FieldTooLong { len } => {
                write!(f, "field of {len} bytes exceeds the field bound")
            }
        }
    }
}

impl std::error::Error for EncodeError {}

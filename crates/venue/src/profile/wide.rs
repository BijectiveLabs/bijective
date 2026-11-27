// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Wide integers as JSON strings: a JSON number is a double in most parsers and would be silently
//! rounded by an operator's tooling.

use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub fn serialize<S: Serializer>(v: &i128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i128, D::Error> {
    let text = String::deserialize(d)?;
    text.parse::<i128>().map_err(D::Error::custom)
}

pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<i128>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(n) => s.serialize_some(&n.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i128>, D::Error> {
        let text: Option<String> = Option::deserialize(d)?;
        match text {
            Some(t) => t.parse::<i128>().map(Some).map_err(D::Error::custom),
            None => Ok(None),
        }
    }
}

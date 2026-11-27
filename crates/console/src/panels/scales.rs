// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The scales the record declares per venue and instrument (`InstrumentScales`), for the panels
//! that turn an order's scaled integers into the decimals they stand for.

use std::collections::BTreeMap;

use bijective_control_engine::wire::ControlEvent;

use crate::table::Cell;

/// By venue and instrument: the price scale and the quantity scale last recorded.
#[derive(Default)]
pub struct Scales {
    declared: BTreeMap<(String, String), (u8, u8)>,
}

impl Scales {
    pub fn learn(&mut self, event: &ControlEvent) {
        if let ControlEvent::InstrumentScales {
            venue,
            instrument,
            price_scale,
            quantity_scale,
            ..
        } = event
        {
            let key = (venue.clone(), instrument.as_str().to_string());
            self.declared.insert(key, (*price_scale, *quantity_scale));
        }
    }

    pub(crate) fn of(&self, venue: Option<&str>, instrument: &str) -> Option<(u8, u8)> {
        self.declared.get(&(venue?.to_string(), instrument.to_string())).copied()
    }

    /// A price as the decimal it stands for. Absent where the record declared no scale for it,
    /// as in a record written before the scales were.
    #[must_use]
    pub fn price(&self, venue: Option<&str>, instrument: &str, scaled: Option<i128>) -> Cell {
        Cell::decimal(scaled, self.of(venue, instrument).map(|(price, _)| price))
    }

    #[must_use]
    pub fn quantity(&self, venue: Option<&str>, instrument: &str, scaled: i128) -> Cell {
        Cell::decimal(Some(scaled), self.of(venue, instrument).map(|(_, quantity)| quantity))
    }
}

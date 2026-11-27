// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The scales an order's figures are recorded at, declared in the record before the order is.
//!
//! An intent carries scaled integers. Without the instrument's scales a reader of the record
//! alone, a console, an audit or an export, cannot say what they stand for. Recorded once per
//! venue and instrument, and again when the profile declares other scales.

use bijective_model::VenueSymbol;
use bijective_venue::profile::CapabilityDeclaration;

use super::{ControlEvent, OrderPath, PathError};

impl OrderPath {
    /// Records `instrument`'s scales on `venue` unless the record already holds the same.
    /// An instrument the profile does not list records nothing: its order is refused before an
    /// intent is written, by the instrument check.
    pub(super) fn declare_scales(
        &mut self,
        venue: &str,
        declaration: &CapabilityDeclaration,
        instrument: &VenueSymbol,
        at_ns: i128,
    ) -> Result<(), PathError> {
        let Ok(listed) = declaration.instruments().get(instrument) else { return Ok(()) };
        let precision = &listed.precision;
        let declared = crate::orders::Declared {
            price_scale: precision.price_scale,
            quantity_scale: precision.quantity_scale,
            identity: Some(listed.id.as_str().to_string()),
            quote_currency: Some(listed.quote_currency.to_ascii_lowercase()),
        };
        let recorded = self.recorded_orders().and_then(|o| o.declared(venue, instrument));
        if recorded == Some(&declared) {
            return Ok(());
        }
        self.record(&ControlEvent::InstrumentScales {
            venue: venue.to_string(),
            instrument: instrument.clone(),
            price_scale: declared.price_scale,
            quantity_scale: declared.quantity_scale,
            at_ns,
            identity: declared.identity,
            quote_currency: declared.quote_currency,
        })?;
        Ok(())
    }
}

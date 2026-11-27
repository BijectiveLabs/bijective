// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Book lookups at a market position: mid, spread, crossing and the venue's declared fees.

use super::*;

/// Three answers: nothing recorded there and an unreadable entry are different facts to an operator.
pub(crate) enum At {
    Book(Box<BookUpdate>),
    Missing,
    Unreadable,
}

pub(crate) fn book_at(root: &Path, stream: &str, at: u64) -> Result<At, MeasureError> {
    let entries =
        read_range(root, stream, at, at).map_err(|e| MeasureError::Store(format!("{e:?}")))?;
    let Some(entry) = entries.first() else {
        return Ok(At::Missing);
    };
    Ok(match bijective_serialization::signal::market::decode(&entry.payload) {
        Ok(book) => At::Book(Box::new(book)),
        Err(_) => At::Unreadable,
    })
}

/// Mid to mid over the horizon, signed by side, in basis points, rounded down. Gross: the floor
/// owns the crossings, and charging them here would count them twice.
pub(crate) fn gross_of(proposal: &Proposal, entry: &BookUpdate, exit: &BookUpdate) -> Option<i128> {
    if proposal.instrument.as_str() != exit.instrument.as_str() {
        return None;
    }
    let from = mid_of(entry)?;
    let to = mid_of(exit)?;
    let moved = to.checked_sub(from)?;
    let taken = match proposal.side {
        Side::Buy => moved,
        Side::Sell => -moved,
    };
    floor_div(taken.checked_mul(BPS)?.checked_mul(ONE)?, from)
}

pub(crate) fn mid_of(book: &BookUpdate) -> Option<i128> {
    let mid = book.bid_price.checked_add(book.ask_price)? / 2;
    (mid > 0).then_some(mid)
}

/// Touch spread in basis points, rounded up: every rounding in a cost goes against the strategy.
pub(crate) fn spread_of(book: &BookUpdate) -> Option<i128> {
    let mid = book.bid_price.checked_add(book.ask_price)? / 2;
    if mid <= 0 || book.ask_price <= book.bid_price {
        return None;
    }
    let width = book.ask_price.checked_sub(book.bid_price)?;
    ceil_div(width.checked_mul(BPS)?.checked_mul(ONE)?, mid)
}

pub(crate) fn crosses(proposal: &Proposal, book: &BookUpdate) -> bool {
    match proposal.price_scaled {
        None => true,
        Some(limit) => match proposal.side {
            Side::Buy => limit >= book.ask_price,
            Side::Sell => limit <= book.bid_price,
        },
    }
}

pub(crate) fn venue_fees(root: &Path, stream: &str) -> Result<Option<Fees>, MeasureError> {
    let Some(venue) = venue_of(stream) else {
        return Ok(None);
    };
    fees_standing(root, venue).map_err(|e| MeasureError::Store(format!("{e}")))
}

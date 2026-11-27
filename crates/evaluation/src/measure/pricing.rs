// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Prices each admitted proposal against the recorded book where its author decided, and at the claim's horizon.

use super::*;
use bijective_strategy_engine::runs::ledger::Admitted;

/// The book an admission was decided on and the book at its horizon.
pub(crate) type Books = (BookUpdate, Option<BookUpdate>);

/// Prices every proposal of one admission. A proposal that cannot be priced is listed with the
/// reason, never dropped.
pub(crate) fn price(
    root: &Path,
    admission: &Admitted,
    horizon: Horizon,
    placed: &mut Vec<Placed>,
    unplaced: &mut Vec<Unplaced>,
) -> Result<(), MeasureError> {
    if !admission.is_contiguous() {
        unplaced.push(Unplaced::Interleaved {
            from: admission.from,
            through: admission.through,
            count: admission.count,
        });
        return Ok(());
    }
    let books = match admission_books(root, admission, horizon)? {
        Ok(books) => books,
        Err(why) => {
            unplaced.push(why);
            return Ok(());
        }
    };
    let proposals = read_range(root, &admission.proposals, admission.from, admission.through)
        .map_err(|e| MeasureError::Store(format!("{e:?}")))?;
    for record in proposals {
        match price_one(root, admission, horizon, &books, record.seq, &record.payload)? {
            Ok(one) => placed.push(one),
            Err(why) => unplaced.push(why),
        }
    }
    Ok(())
}

/// The book at the admission's market position and at its horizon. A horizon beyond the record
/// leaves the whole admission unpriced: its return would answer a shorter horizon.
pub(crate) fn admission_books(
    root: &Path,
    admission: &Admitted,
    horizon: Horizon,
) -> Result<Result<Books, Unplaced>, MeasureError> {
    let book = match book_at(root, &admission.market, admission.market_through)? {
        At::Book(book) => *book,
        At::Missing => {
            return Ok(Err(Unplaced::MarketMissing {
                proposal_position: admission.from,
                market_position: admission.market_through,
            }));
        }
        At::Unreadable => {
            return Ok(Err(Unplaced::MarketUnreadable {
                market_position: admission.market_through,
            }));
        }
    };
    if spread_of(&book).is_none() {
        return Ok(Err(Unplaced::NoMid { market_position: admission.market_through }));
    }
    let Horizon::After { positions } = horizon else {
        return Ok(Ok((book, None)));
    };
    let at = admission.market_through.saturating_add(positions);
    Ok(match book_at(root, &admission.market, at)? {
        At::Book(exit) => Ok((book, Some(*exit))),
        At::Missing => {
            Err(Unplaced::HorizonBeyondTheRecord { from: admission.market_through, wanted: at })
        }
        At::Unreadable => Err(Unplaced::MarketUnreadable { market_position: at }),
    })
}

/// Prices one proposal where its author decided. A bounded run is sent its whole window first,
/// so pricing at the admission's position would put every proposal at the window's end with no
/// dispersion.
pub(crate) fn price_one(
    root: &Path,
    admission: &Admitted,
    horizon: Horizon,
    books: &Books,
    seq: u64,
    payload: &[u8],
) -> Result<Result<Placed, Unplaced>, MeasureError> {
    let Ok(proposal) = bijective_serialization::signal::decode(payload) else {
        return Ok(Err(Unplaced::ProposalUnreadable { proposal_position: seq }));
    };
    if proposal.instrument.as_str() != books.0.instrument.as_str() {
        return Ok(Err(Unplaced::OtherInstrument {
            proposal_position: seq,
            proposed: proposal.instrument.as_str().to_string(),
            book: books.0.instrument.as_str().to_string(),
        }));
    }
    let (at, book, exit) = match proposal.decided_at_position {
        Some(at) if at != admission.market_through => match book_at(root, &admission.market, at)? {
            At::Book(own) => (at, *own, horizon_book(root, &admission.market, at, horizon)?),
            At::Missing => {
                return Ok(Err(Unplaced::MarketMissing {
                    proposal_position: seq,
                    market_position: at,
                }));
            }
            At::Unreadable => {
                return Ok(Err(Unplaced::MarketUnreadable { market_position: at }));
            }
        },
        _ => (admission.market_through, books.0.clone(), books.1.clone()),
    };
    let Some(spread) = spread_of(&book) else {
        return Ok(Err(Unplaced::NoMid { market_position: at }));
    };
    Ok(Ok(Placed {
        proposal_position: seq,
        market_position: at,
        spread,
        gross: exit.as_ref().and_then(|e| gross_of(&proposal, &book, e)),
        marketable: crosses(&proposal, &book),
    }))
}

/// The book `positions` after `at` when the claim holds a position. `None` when it holds none or
/// the record does not reach that far.
pub(crate) fn horizon_book(
    root: &Path,
    market: &str,
    at: u64,
    horizon: Horizon,
) -> Result<Option<BookUpdate>, MeasureError> {
    let Horizon::After { positions } = horizon else {
        return Ok(None);
    };
    Ok(match book_at(root, market, at.saturating_add(positions))? {
        At::Book(exit) => Some(*exit),
        At::Missing | At::Unreadable => None,
    })
}

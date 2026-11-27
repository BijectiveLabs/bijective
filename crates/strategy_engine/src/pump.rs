// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Feeding a strategy, and taking what it says back.

use std::path::Path;

use crate::protocol::{
    DecidedAt, Inbound, LineError, Sent, book_line, fact_line, read_proposal, to_line,
};
use bijective_event_store::cursor::{Cursor, read_after};
use bijective_event_store::store::{StoreError, StreamWriter, read_range};
use bijective_serialization::signal::facts::{self, FactUpdate};
use bijective_serialization::signal::market::BookUpdate;

/// What a strategy said that the supervisor would not carry, named so an operator can find the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// Which line of the run's output, counted from one as an operator would.
    pub line: usize,
    pub because: LineError,
}

#[derive(Debug)]
pub enum PumpError {
    Render(serde_json::Error),
    Undecodable { at: u64 },
    Store(StoreError),
    Encode(bijective_serialization::codec::writer::EncodeError),
    Fees(bijective_serialization::signal::fees::FeesError),
    Instruments(bijective_serialization::signal::instruments::InstrumentsError),
}

impl core::fmt::Display for PumpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PumpError::Render(e) => write!(f, "an event could not be rendered: {e}"),
            PumpError::Undecodable { at } => {
                write!(f, "the market entry at position {at} could not be read")
            }
            PumpError::Store(e) => write!(f, "store: {e:?}"),
            PumpError::Encode(e) => write!(f, "a proposal could not be encoded: {e:?}"),
            PumpError::Fees(e) => write!(f, "the venue's fees could not be read: {e}"),
            PumpError::Instruments(e) => {
                write!(f, "the venue's instrument rules could not be read: {e}")
            }
        }
    }
}

impl std::error::Error for PumpError {}

/// Ends with `end_of_window` so a run knows its input is exhausted. A live strategy never gets that line.
/// `venue` is the venue the market stream belongs to.
/// The line that opens a session on `stream`: the venue it belongs to, what that venue's
/// standing fee declaration says, and the instrument rules declared for it. `None` for a stream that names no venue. A declaration that
/// cannot be read refuses the run rather than sending it without fees.
pub fn opening(store_root: &Path, stream: &str) -> Result<Option<Vec<u8>>, PumpError> {
    let Some(venue) = bijective_serialization::signal::market::venue_of(stream) else {
        return Ok(None);
    };
    let fees = bijective_serialization::signal::fees::standing(store_root, venue)
        .map_err(PumpError::Fees)?;
    let rules = bijective_serialization::signal::instruments::standing(store_root, venue)
        .map_err(PumpError::Instruments)?;
    let instruments = rules
        .map(|r| r.instruments)
        .unwrap_or_default()
        .into_iter()
        .map(|i| crate::protocol::InstrumentLine {
            instrument: i.instrument,
            price_scale: i.price_scale,
            quantity_scale: i.quantity_scale,
            price_tick: i.price_tick.to_string(),
            quantity_lot: i.quantity_lot.to_string(),
            minimum_size: i.minimum_size.to_string(),
            minimum_notional: i.minimum_notional.to_string(),
            trading: i.trading,
            kind: i.kind,
            quote_currency: i.quote_currency,
            settlement_currency: i.settlement_currency,
            base_currency: i.base_currency,
        })
        .collect();
    let line = Inbound::Venue {
        venue: venue.to_string(),
        taker_fee_bps_scaled: fees.as_ref().map(|f| f.taker.to_string()),
        maker_fee_bps_scaled: fees.as_ref().map(|f| f.maker.to_string()),
        fee_source: fees.map(|f| f.source),
        instruments,
    };
    Ok(Some(to_line(&line).map_err(PumpError::Render)?.into_bytes()))
}

pub fn window(updates: &[(u64, BookUpdate)], venue: Option<&str>) -> Result<Vec<u8>, PumpError> {
    let mut out = String::new();
    for (position, update) in updates {
        out.push_str(&to_line(&book_line(*position, update, venue)).map_err(PumpError::Render)?);
    }
    out.push_str(&to_line(&Inbound::EndOfWindow).map_err(PumpError::Render)?);
    Ok(out.into_bytes())
}

/// Only the supervisor appends proposals. `Recorded` makes research integrity a type, so no output is admitted before its run.
/// Refused lines are returned, never skipped (`docs/concepts/strategies.md`). Admitted proposals
/// are returned as the venue each names, in the order they were appended.
pub fn admit_result(
    stdout: &[u8],
    _entry: &crate::ledger::Recorded,
    admitting: &Admitting<'_>,
    into: &mut StreamWriter,
) -> Result<(Vec<Option<String>>, Vec<Refused>), PumpError> {
    let text = String::from_utf8_lossy(stdout);
    admit_lines(text.lines(), admitting, into)
}

/// What a strategy's lines are admitted against: what it was sent, whether its decisions can be
/// placed on a position, and the name it runs under, which every proposal it makes carries.
#[derive(Debug, Clone, Copy)]
pub struct Admitting<'a> {
    pub sent: &'a Sent,
    pub placeable: bool,
    pub strategy: Option<&'a str>,
}

/// One admission rule for both lifetimes: two would be two definitions of what an operator may say, and the live one is the less tested half.
/// `sent` bounds what the strategy may claim to have decided on, in both directions.
pub fn admit_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
    admitting: &Admitting<'_>,
    into: &mut StreamWriter,
) -> Result<(Vec<Option<String>>, Vec<Refused>), PumpError> {
    let Admitting { sent, placeable, strategy } = *admitting;
    let mut admitted = Vec::new();
    let mut refused = Vec::new();
    for (i, line) in lines.enumerate() {
        // Blank lines are how a buffered writer ends output. Refusing them would refuse every well-behaved run.
        if line.trim().is_empty() {
            continue;
        }
        match read_proposal(line, sent, placeable) {
            // The turn marker is for the supervisor: not a proposal and not a refusal.
            Ok(crate::protocol::Decided::Turn) => continue,
            Ok(crate::protocol::Decided::Proposal { proposal, at }) => {
                let mut proposal = *proposal;
                // Absent stays absent: a silent decision is not position zero. Where the exchange places decisions, the supervisor writes the event it was answering.
                proposal.decided_at_position = match at {
                    DecidedAt::Position(at) => Some(at),
                    DecidedAt::NotSaid if placeable => {
                        sent.for_venue(proposal.venue.as_deref()).map(|b| b.through)
                    }
                    DecidedAt::NotSaid => None,
                };
                // Stamped here, never taken from the line: a strategy cannot speak for another.
                proposal.strategy = strategy.map(str::to_string);
                let payload = bijective_serialization::signal::encode(&proposal)
                    .map_err(PumpError::Encode)?;
                into.append(&payload).map_err(PumpError::Store)?;
                admitted.push(proposal.venue);
            }
            Err(because) => refused.push(Refused { line: i + 1, because }),
        }
    }

    Ok((admitted, refused))
}

/// What one position of a stream a strategy reads holds: a top of book, or another fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fed {
    Book(BookUpdate),
    Fact(FactUpdate),
}

impl Fed {
    /// When it arrived on this node's clock, or, for a top recorded before receive times were
    /// kept, when the venue stamped it.
    #[must_use]
    pub fn arrival(&self) -> i128 {
        match self {
            Fed::Book(top) => top.received_at_ns.unwrap_or(i128::from(top.venue_ts_ms) * 1_000_000),
            Fed::Fact(fact) => fact.received_at_ns,
        }
    }

    /// The line a strategy is sent for it, at `position` on its stream.
    pub fn line(&self, position: u64, venue: Option<&str>) -> Result<String, PumpError> {
        let event = match self {
            Fed::Book(top) => book_line(position, top, venue),
            Fed::Fact(fact) => {
                fact_line(position, fact, venue).ok_or(PumpError::Undecodable { at: position })?
            }
        };
        to_line(&event).map_err(PumpError::Render)
    }
}

/// Whether `stream` is a venue's facts stream rather than its market stream.
#[must_use]
pub fn is_facts(stream: &str) -> bool {
    facts::venue_of(stream).is_some()
}

/// Positions `from` through `to` of a market or facts stream, decoded by what the stream's name
/// says it holds. Refused, never skipped: a hole would be a window nobody declared.
pub fn read_fed(
    store_root: &Path,
    stream: &str,
    from: u64,
    to: u64,
) -> Result<Vec<(u64, Fed)>, PumpError> {
    let entries = read_range(store_root, stream, from, to).map_err(PumpError::Store)?;
    decode_fed(stream, entries)
}

/// Entries by position, and the cursor a read ended at.
pub type FedAfter = (Vec<(u64, Fed)>, Option<Cursor>);

/// What a market or facts stream gained after `cursor` (the whole stream for `None`), decoded as
/// `read_fed` decodes it, with the cursor to read on from. A poll that keeps the cursor verifies
/// each byte once, where reading by position verifies the stream's whole history every time.
pub fn read_fed_after(
    store_root: &Path,
    stream: &str,
    cursor: Option<Cursor>,
) -> Result<FedAfter, PumpError> {
    let (entries, cursor) = read_after(store_root, stream, cursor).map_err(PumpError::Store)?;
    Ok((decode_fed(stream, entries)?, cursor))
}

fn decode_fed(
    stream: &str,
    entries: Vec<bijective_event_store::format::Entry>,
) -> Result<Vec<(u64, Fed)>, PumpError> {
    let holds_facts = is_facts(stream);
    let mut fed = Vec::with_capacity(entries.len());
    for entry in entries {
        let undecodable = |_| PumpError::Undecodable { at: entry.seq };
        let read = if holds_facts {
            Fed::Fact(facts::decode(&entry.payload).map_err(undecodable)?)
        } else {
            Fed::Book(
                bijective_serialization::signal::market::decode(&entry.payload)
                    .map_err(undecodable)?,
            )
        };
        fed.push((entry.seq, read));
    }
    Ok(fed)
}

/// Reads forward from the last poll, so a session up for a week does not re-read the week each pass.
pub fn since(
    store_root: &Path,
    stream: &str,
    from: u64,
) -> Result<Vec<(u64, BookUpdate)>, PumpError> {
    let entries = read_range(store_root, stream, from, u64::MAX).map_err(PumpError::Store)?;

    let mut updates = Vec::with_capacity(entries.len());
    for entry in entries {
        let update = bijective_serialization::signal::market::decode(&entry.payload)
            .map_err(|_| PumpError::Undecodable { at: entry.seq })?;
        updates.push((entry.seq, update));
    }

    Ok(updates)
}

/// The window bounds the read, not only the run: reading all history each pass has starved a node before.
/// `to_position` is inclusive, as research integrity states a window.
pub fn market(
    store_root: &Path,
    stream: &str,
    to_position: u64,
) -> Result<Vec<(u64, BookUpdate)>, PumpError> {
    let entries = read_range(store_root, stream, 0, to_position).map_err(PumpError::Store)?;

    let mut updates = Vec::with_capacity(entries.len());
    for entry in entries {
        // Refused, never skipped: a hole in the window means a verdict measured on an undeclared window.
        updates.push((
            entry.seq,
            bijective_serialization::signal::market::decode(&entry.payload)
                .map_err(|_| PumpError::Undecodable { at: entry.seq })?,
        ));
    }

    Ok(updates)
}

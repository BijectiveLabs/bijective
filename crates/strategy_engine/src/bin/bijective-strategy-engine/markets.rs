// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The streams a run or session reads, merged into the one sequence its strategies are sent.
//!
//! A stream is a venue's market stream (tops) or its facts stream (trades, funding and the rest).
//! Each keeps its own order. Across streams the next event is the one that arrived first on this
//! node's clock, a fact before the top that came in the same frame, then the order named. Runs and
//! sessions share the merge. A session merges per poll, so it can differ from a backtest only
//! among events arriving within one poll interval (`docs/concepts/strategies.md`).

use std::collections::BTreeMap;
use std::path::Path;

use bijective_serialization::signal::{facts, market};
use bijective_strategy_engine::protocol::{Batch, Inbound, Sent, to_line};
use bijective_strategy_engine::{Fed, is_facts, opening, read_fed};

/// One stream a run or session reads, and the venue it belongs to.
pub(crate) struct Feed {
    pub(crate) stream_name: String,
    pub(crate) venue: Option<String>,
    /// A facts stream: its positions are not ones a decision names.
    pub(crate) holds_facts: bool,
    /// The venue's fees and instrument rules, sent before anything else of it. Market streams only.
    pub(crate) opening: Option<Vec<u8>>,
}

/// Each stream named, with its venue and opening. Several streams must each name a venue, at most
/// one market and one facts stream per venue, and every run needs a market stream: decisions are
/// placed on, and measured against, a venue's tops.
pub(crate) fn feeds(store: &Path, streams: &[String]) -> Result<Vec<Feed>, String> {
    let mut feeds: Vec<Feed> = Vec::with_capacity(streams.len());
    for stream in streams {
        let holds_facts = is_facts(stream);
        let venue = if holds_facts { facts::venue_of(stream) } else { market::venue_of(stream) };
        let venue = venue.map(str::to_string);
        if streams.len() > 1 {
            let Some(name) = &venue else {
                return Err(format!(
                    "{stream} names no venue, so a decision on it cannot say where"
                ));
            };
            if feeds.iter().any(|f| f.venue == venue && f.holds_facts == holds_facts) {
                return Err(format!("{name} was named twice; one venue, one stream of each kind"));
            }
        }
        let opening =
            if holds_facts { None } else { opening(store, stream).map_err(|e| format!("{e}"))? };
        feeds.push(Feed { stream_name: stream.clone(), venue, holds_facts, opening });
    }
    if feeds.iter().all(|f| f.holds_facts) {
        return Err(
            "a run reads at least one market stream: decisions are placed on its tops".to_string()
        );
    }
    Ok(feeds)
}

/// One stream of a bounded run: what it is, where it ends and what it holds.
pub(crate) struct Market {
    pub(crate) feed: Feed,
    pub(crate) to_position: u64,
    pub(crate) updates: Vec<(u64, Fed)>,
}

/// Reads every stream named, each through its own end.
pub(crate) fn read(store: &Path, named: &[(String, u64)]) -> Result<Vec<Market>, String> {
    let streams: Vec<String> = named.iter().map(|(stream, _)| stream.clone()).collect();
    let mut markets = Vec::with_capacity(named.len());
    for (feed, (stream, to_position)) in feeds(store, &streams)?.into_iter().zip(named) {
        let updates = read_fed(store, stream, 0, *to_position).map_err(|e| format!("{e}"))?;
        markets.push(Market { feed, to_position: *to_position, updates });
    }
    Ok(markets)
}

/// The send order of a bounded run: each element names a stream and an index into its updates.
pub(crate) fn merged(markets: &[Market]) -> Vec<(usize, usize)> {
    let streams: Vec<&[(u64, Fed)]> = markets.iter().map(|m| m.updates.as_slice()).collect();
    merged_order(&streams)
}

/// The send order of several streams' events, each stream kept in its own order: each element
/// names a stream and an index into its events.
pub(crate) fn merged_order(streams: &[&[(u64, Fed)]]) -> Vec<(usize, usize)> {
    let mut next = vec![0_usize; streams.len()];
    let total = streams.iter().map(|s| s.len()).sum();
    let mut order = Vec::with_capacity(total);
    while order.len() < total {
        let chosen = (0..streams.len())
            .filter(|s| next[*s] < streams[*s].len())
            .min_by_key(|s| order_key(&streams[*s][next[*s]].1, *s));
        let Some(s) = chosen else { break };
        order.push((s, next[s]));
        next[s] += 1;
    }
    order
}

/// Arrival first. A frame's facts reach the store before its top, so at one instant a fact goes
/// first, as the data engine recorded them. Then the order the streams were named.
fn order_key(event: &Fed, stream: usize) -> (i128, u8, usize) {
    let rank = match event {
        Fed::Fact(_) => 0,
        Fed::Book(_) => 1,
    };
    (event.arrival(), rank, stream)
}

/// The stretches of admitted proposals that name one venue, in append order, each with the index
/// of the market stream it is measured on. With one market stream every proposal is measured on
/// it. With several, admission has already refused a proposal naming none of their venues.
pub(crate) fn stretches(venues: &[Option<String>], feeds: &[&Feed]) -> Vec<(usize, u64)> {
    let books: Vec<usize> = (0..feeds.len()).filter(|i| !feeds[*i].holds_facts).collect();
    let mut out: Vec<(usize, u64)> = Vec::new();
    for venue in venues {
        let market = match books.as_slice() {
            [only] => *only,
            several => {
                several.iter().copied().find(|i| feeds[*i].venue == *venue).unwrap_or(several[0])
            }
        };
        match out.last_mut() {
            Some((last, count)) if *last == market => *count += 1,
            _ => out.push((market, 1)),
        }
    }
    out
}

/// What a decision may name, given the batch each market stream was answering: one market's batch,
/// or each venue's. A market with no batch (nothing of it sent yet) is left out, so a decision on
/// it is refused rather than placed. Facts streams name no decision positions.
pub(crate) fn sent(feeds: &[&Feed], batch_of: impl Fn(usize) -> Option<Batch>) -> Sent {
    let books: Vec<usize> = (0..feeds.len()).filter(|i| !feeds[*i].holds_facts).collect();
    if let [only] = books.as_slice()
        && let Some(batch) = batch_of(*only)
    {
        return Sent::One(batch);
    }
    let per_venue: BTreeMap<String, Batch> =
        books.into_iter().filter_map(|i| Some((feeds[i].venue.clone()?, batch_of(i)?))).collect();
    Sent::PerVenue(per_venue)
}

/// The opening lines, in the order the streams were named.
pub(crate) fn openings(feeds: &[&Feed]) -> Vec<u8> {
    feeds.iter().filter_map(|f| f.opening.clone()).flatten().collect()
}

/// One line of the merged order.
pub(crate) fn line(markets: &[Market], (m, i): (usize, usize)) -> Result<String, String> {
    let market = &markets[m];
    let (position, event) = &market.updates[i];
    event.line(*position, market.feed.venue.as_deref()).map_err(|e| format!("{e}"))
}

/// Every opening, every event in merged order, and the end of the window.
pub(crate) fn render(markets: &[Market], order: &[(usize, usize)]) -> Result<Vec<u8>, String> {
    let feeds: Vec<&Feed> = markets.iter().map(|m| &m.feed).collect();
    let mut bytes = openings(&feeds);
    for event in order {
        bytes.extend(line(markets, *event)?.into_bytes());
    }
    bytes.extend(to_line(&Inbound::EndOfWindow).map_err(|e| format!("{e}"))?.into_bytes());
    Ok(bytes)
}

/// One stream a session reads, and where the session picked it up.
pub(crate) struct Source {
    pub(crate) feed: Feed,
    pub(crate) resume_from: u64,
}

/// How far one strategy has been sent through one stream.
#[derive(Clone, Copy)]
pub(crate) struct Along {
    pub(crate) resume_from: u64,
    /// The next position to send.
    pub(crate) next: u64,
    /// The last position sent, if any.
    pub(crate) through: Option<u64>,
    /// The first position of the events not yet answered.
    pub(crate) batch_from: u64,
}

impl Along {
    pub(crate) fn starting_at(resume_from: u64) -> Self {
        Self { resume_from, next: resume_from, through: None, batch_from: resume_from }
    }

    /// The events sent since the strategy's last answer.
    pub(crate) fn unanswered(&self) -> Batch {
        Batch::of(self.batch_from, self.through.unwrap_or(self.resume_from.saturating_sub(1)))
    }
}

/// Each stream named, picked up at its present: found from the newest segment rather than by
/// replaying the stream.
pub(crate) fn sources(store: &Path, streams: &[String]) -> Result<Vec<Source>, String> {
    let mut sources = Vec::with_capacity(streams.len());
    for feed in feeds(store, streams)? {
        let resume_from = bijective_event_store::store::end_of(store, &feed.stream_name)
            .map_err(|e| format!("the stream {} could not be read: {e:?}", feed.stream_name))?
            .map_or(0, |end| end + 1);
        sources.push(Source { feed, resume_from });
    }
    Ok(sources)
}

/// What a decision may name: the events of each market sent since the strategy's last answer.
/// One market answers its batch as it always has. With several, a market nothing of which was
/// sent yet is left out, as in a bounded run.
pub(crate) fn unanswered(sources: &[Source], along: &[Along]) -> Sent {
    let feeds: Vec<&Feed> = sources.iter().map(|s| &s.feed).collect();
    let several = feeds.iter().filter(|f| !f.holds_facts).count() > 1;
    sent(&feeds, |i| (!several || along[i].through.is_some()).then(|| along[i].unanswered()))
}

#[cfg(test)]
#[path = "../../../tests/unit/markets_tests.rs"]
mod tests;

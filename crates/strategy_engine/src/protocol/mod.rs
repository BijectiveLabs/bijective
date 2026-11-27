// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Wire contract between supervisor and strategy: one JSON object per line on stdin and stdout.
//! The strategy lifetime model,. The numbers travel as text. See `docs/concepts/strategies.md`.

#[cfg(test)]
#[path = "../../tests/unit/protocol/tests.rs"]
mod tests;

mod fact;
pub use fact::{FactValues, fact_line};

use bijective_model::{InstrumentId, Proposal, Sequence, Side};
use bijective_serialization::signal::market::BookUpdate;
use serde::{Deserialize, Serialize};

/// Wide integers travel as text: most JSON parsers read a number as a double and drop low digits.
type Wide = String;

/// Tagged, so a line the platform adds later is not mistaken for an existing shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Inbound {
    Book {
        /// Named back in decisions so they are checked against the stream, not the strategy's clock.
        position: Wide,
        /// The platform's identity for the instrument, never a venue symbol.
        instrument: String,
        /// The venue the top was quoted on, so a decision that depends on where can say so.
        /// Absent where the stream names no venue.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        venue: Option<String>,
        /// What the venue numbered it, where it numbers at all. Absent is not zero.
        #[serde(skip_serializing_if = "Option::is_none")]
        venue_sequence: Option<Wide>,
        venue_ts_ms: Wide,
        /// This node's clock when the top arrived, one clock for every venue. Absent on a top
        /// recorded before receive times were kept.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        received_at_ns: Option<Wide>,
        bid_price: Wide,
        bid_quantity: Wide,
        ask_price: Wide,
        ask_quantity: Wide,
        bid_depth: u32,
        ask_depth: u32,
    },
    /// A fact about an instrument other than its top of book, read from a venue's facts stream.
    /// `fact_position` is a position on that stream, not on the market stream a decision names.
    Fact {
        fact_position: Wide,
        instrument: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        venue: Option<String>,
        /// The venue's stamp. Absent where the venue published none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exchange_ts_ns: Option<Wide>,
        received_at_ns: Wide,
        #[serde(flatten)]
        values: FactValues,
    },
    /// Bounded lifetime only. A live strategy never receives it.
    EndOfWindow,
    /// First line of a live session. The strategy is told the interval, not sent it: a replay would
    /// cost the stream's age and could pass for live.
    /// A session reading several markets is sent one per market, each naming its venue.
    Resumed {
        from_position: Wide,
        missed: Missed,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        venue: Option<String>,
        /// The stream the position is on: a venue's market and facts streams resume apart.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<String>,
    },
    /// What the venue a session reads charges, sent once before its first top. Fees are basis
    /// points at the platform's 8-decimal scale, as declared with their source. Absent where
    /// no declaration stands, which is not a fee of zero.
    Venue {
        venue: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        taker_fee_bps_scaled: Option<Wide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maker_fee_bps_scaled: Option<Wide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_source: Option<String>,
        /// The rules the venue sets for each instrument it lists, as the data engine declared
        /// them. Empty where none were declared.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        instruments: Vec<InstrumentLine>,
    },
    /// What became of one of this strategy's proposals, named by its position in the proposal
    /// stream. Sent to live sessions only: nothing executes a paper session's proposals.
    Order {
        proposal_position: Wide,
        /// Absent for a refusal: no order was made.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        /// `refused`, `submitted`, `accepted`, `rejected`, `unknown`, `filled`,
        /// `cancel_requested`, `canceled`, `expired` or `reconciling`. `unknown` is neither placed
        /// nor failed.
        status: String,
        /// When the control engine recorded it, UTC nanoseconds on the platform clock. The time
        /// from `submitted` to the venue's answer is the venue's latency as this node saw it.
        recorded_at_ns: Wide,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill_id: Option<String>,
        /// A fill's quantity and price, at the venue's scales.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill_quantity_scaled: Option<Wide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill_price_scaled: Option<Wide>,
    },
    /// Tolerant on input, strict on output: a new platform event must not kill a strategy, but a
    /// proposal read wrongly costs the operator a position they did not take.
    #[serde(other)]
    Unrecognised,
}

/// One instrument's rules as a strategy reads them. Figures are at the instrument's own scales.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentLine {
    /// The platform's identity, as books and proposals name it.
    pub instrument: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
    pub price_tick: Wide,
    pub quantity_lot: Wide,
    pub minimum_size: Wide,
    pub minimum_notional: Wide,
    /// Whether the venue accepts orders for it.
    pub trading: bool,
    /// What the instrument is (`spot`, `perpetual`, ...), and what it is quoted and settled in.
    /// Absent where the venue's declaration predates them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_currency: Option<String>,
    /// The asset a quantity is counted in. Absent where the venue's declaration predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_currency: Option<String>,
}

/// What a live session was not sent. Never understated: a gap read as smaller makes a strategy
/// confident about a position it does not hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "missed", rename_all = "snake_case")]
pub enum Missed {
    /// Equal to the resume position means nothing was missed, which is not the same as not knowing.
    From { position: Wide },
    /// Nothing records how far the last session was sent. Read as everything before this point.
    NotRecorded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "emit", rename_all = "snake_case")]
// Parsed from one line and dropped at once. Boxing the proposal would add an allocation per line.
#[allow(clippy::large_enum_variant)]
pub enum Outbound {
    /// Turn marker of a stepwise exchange. Without it the supervisor cannot tell deciding from
    /// silent. A run that never sends one reaches its wall bound.
    Answered,
    /// A proposal for the Control Engine to judge, not an order.
    Proposal {
        desk: String,
        instrument: String,
        side: Emitted,
        quantity_scaled: Wide,
        /// Absent is a market order, and is not a price of zero.
        #[serde(skip_serializing_if = "Option::is_none")]
        price_scaled: Option<Wide>,
        /// What a market order's author will be held to. Required where there is no price.
        #[serde(skip_serializing_if = "Option::is_none")]
        cap_scaled: Option<Wide>,
        client_id: String,
        decided_at_ns: Wide,
        /// Checked against what the supervisor sent, never trusted: a later position claims to have
        /// seen what was never sent.
        #[serde(skip_serializing_if = "Option::is_none")]
        decided_at_position: Option<Wide>,
        /// The venue the decision was made for. Absent leaves it to the desk's preference.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        venue: Option<String>,
        /// `gtc` (absent), `ioc`, `fok`, `day`, `at_the_open` or `at_the_close` (decision 0009).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_in_force: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        post_only: bool,
        /// A stop price at the instrument's scale (decision 0012). Absent, the order is not a stop.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_price_scaled: Option<String>,
        /// When the node cancels the order if it still works, UTC nanoseconds (decision 0014).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expire_at_ns: Option<String>,
        /// Only reduces the position the record holds (decision 0012).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reduce_only: bool,
        /// The proposal this one waits on, by its client identifier (decision 0013).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// The one-cancels-other group, scoped to the parent (decision 0013).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oco: Option<String>,
    },
}

/// Market positions answered: the window for a bounded run, everything since the last poll for live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub from: u64,
    pub through: u64,
}

impl Batch {
    #[must_use]
    pub fn of(from: u64, through: u64) -> Self {
        Self { from, through }
    }

    #[must_use]
    pub fn holds(&self, at: u64) -> bool {
        at >= self.from && at <= self.through
    }
}

/// What a strategy had been sent when it answered: one market's batch, or one batch per venue
/// when it reads several. Positions belong to a stream, so with several a decision names the
/// venue its position is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    One(Batch),
    PerVenue(std::collections::BTreeMap<String, Batch>),
}

impl From<Batch> for Sent {
    fn from(batch: Batch) -> Self {
        Self::One(batch)
    }
}

impl Sent {
    /// The batch a decision on `venue` is checked against. `None` where several venues were sent
    /// and the decision names none of them.
    #[must_use]
    pub fn for_venue(&self, venue: Option<&str>) -> Option<Batch> {
        match (self, venue) {
            (Self::One(batch), _) => Some(*batch),
            (Self::PerVenue(batches), Some(venue)) => batches.get(venue).copied(),
            (Self::PerVenue(_), None) => None,
        }
    }
}

/// `NotSaid` is not position zero. That would price a decision against the run's first book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecidedAt {
    Position(u64),
    NotSaid,
}

/// Boxed so the common empty turn does not cost what a full proposal does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decided {
    Proposal { proposal: Box<Proposal>, at: DecidedAt },
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emitted {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    Unreadable {
        detail: String,
    },
    /// A time in force this platform does not name (decision 0009).
    UnknownTimeInForce {
        found: String,
    },
    NotAWholeNumber {
        field: &'static str,
        found: String,
    },
    /// A market order's cost is unbounded, so it is refused at the reader.
    MarketOrderWithoutCap,
    /// A run that sent its whole window cannot place a position. The claim is unverifiable.
    DecisionUnplaceable,
    /// Outside the batch is refused both ways: past it was never sent, before it was already answered.
    DecidedOutsideTheBatch {
        at: u64,
        batch: Batch,
    },
    /// A strategy reading several venues names, in every proposal, one of the venues it was sent:
    /// its position, and the book it is measured against, belong to that venue.
    VenueNotSent {
        venue: Option<String>,
    },
}

impl core::fmt::Display for LineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LineError::Unreadable { detail } => write!(f, "unreadable line: {detail}"),
            LineError::NotAWholeNumber { field, found } => {
                write!(f, "{field} is not a whole number: {found}")
            }
            LineError::UnknownTimeInForce { found } => write!(
                f,
                "time_in_force {found} is not gtc, ioc, fok, day, at_the_open or at_the_close"
            ),
            LineError::MarketOrderWithoutCap => {
                write!(f, "a market order carries no cap, so its cost is unbounded")
            }
            LineError::DecisionUnplaceable => write!(
                f,
                "this run was sent its window whole, so where a decision was made cannot be checked"
            ),
            LineError::DecidedOutsideTheBatch { at, batch } => write!(
                f,
                "the decision claims market position {at}, and it was answering [{}, {}]",
                batch.from, batch.through
            ),
            LineError::VenueNotSent { venue: None } => {
                write!(f, "this strategy reads several venues, so a proposal names its venue")
            }
            LineError::VenueNotSent { venue: Some(venue) } => {
                write!(f, "the proposal names {venue}, which this run was not sent")
            }
        }
    }
}

impl std::error::Error for LineError {}

#[must_use]
/// `venue` is the venue the market stream belongs to, read from its name.
pub fn book_line(position: u64, update: &BookUpdate, venue: Option<&str>) -> Inbound {
    Inbound::Book {
        position: position.to_string(),
        instrument: update.instrument.as_str().to_string(),
        venue: venue.map(str::to_string),
        venue_sequence: sequence_of(update),
        venue_ts_ms: update.venue_ts_ms.to_string(),
        received_at_ns: update.received_at_ns.map(|at| at.to_string()),
        bid_price: update.bid_price.to_string(),
        bid_quantity: update.bid_quantity.to_string(),
        ask_price: update.ask_price.to_string(),
        ask_quantity: update.ask_quantity.to_string(),
        bid_depth: update.bid_depth,
        ask_depth: update.ask_depth,
    }
}

/// The line telling a strategy what became of its proposal.
#[must_use]
pub fn order_line(told: &bijective_serialization::signal::feedback::Feedback) -> Inbound {
    use bijective_serialization::signal::feedback::Outcome;
    let (fill_id, quantity, price) = match &told.outcome {
        Outcome::Filled { fill_id, quantity_scaled, price_scaled } => (
            Some(fill_id.clone()),
            Some(quantity_scaled.to_string()),
            Some(price_scaled.to_string()),
        ),
        _ => (None, None, None),
    };
    Inbound::Order {
        proposal_position: told.proposal_position.to_string(),
        client_id: told.client_id.clone(),
        status: told.outcome.name().to_string(),
        recorded_at_ns: told.at_ns.to_string(),
        reason: told.reason.clone(),
        fill_id,
        fill_quantity_scaled: quantity,
        fill_price_scaled: price,
    }
}

/// A whole line, so a crash mid-write cannot leave a fragment that reads as a message.
pub fn to_line(inbound: &Inbound) -> Result<String, serde_json::Error> {
    Ok(format!("{}\n", serde_json::to_string(inbound)?))
}

/// Tag only: a malformed turn is a refusal reported with the others, not swallowed by the step loop.
#[must_use]
pub fn is_turn(line: &str) -> bool {
    matches!(serde_json::from_str::<Outbound>(line.trim_end()), Ok(Outbound::Answered))
}

/// Unknown lines arrive as `Unrecognised`, not an error, so an older strategy library keeps running.
pub fn read_event(line: &str) -> Result<Inbound, LineError> {
    serde_json::from_str(line.trim_end())
        .map_err(|e| LineError::Unreadable { detail: e.to_string() })
}

/// Refused, never salvaged: the parsed part of a bad line would be a decision the strategy did not send.
/// `sent` is the batch answered. `placeable` says whether the exchange can check a position.
pub fn read_proposal(line: &str, sent: &Sent, placeable: bool) -> Result<Decided, LineError> {
    let outbound: Outbound = serde_json::from_str(line.trim_end())
        .map_err(|e| LineError::Unreadable { detail: e.to_string() })?;

    let Outbound::Proposal {
        desk,
        instrument,
        side,
        quantity_scaled,
        price_scaled,
        cap_scaled,
        client_id,
        decided_at_ns,
        decided_at_position,
        venue,
        time_in_force,
        post_only,
        stop_price_scaled,
        expire_at_ns,
        reduce_only,
        parent,
        oco,
    } = outbound
    else {
        return Ok(Decided::Turn);
    };
    let mut terms = terms_of(time_in_force.as_deref(), post_only, stop_price_scaled.as_deref())?;
    terms.expire_at_ns = optional_whole("expire_at_ns", expire_at_ns.as_deref())?;
    let price_scaled = optional_whole("price_scaled", price_scaled.as_deref())?;
    let cap_scaled = optional_whole("cap_scaled", cap_scaled.as_deref())?;
    if price_scaled.is_none() && cap_scaled.is_none() {
        return Err(LineError::MarketOrderWithoutCap);
    }

    let batch = sent
        .for_venue(venue.as_deref())
        .ok_or_else(|| LineError::VenueNotSent { venue: venue.clone() })?;
    let decided_at = match decided_at_position {
        None => DecidedAt::NotSaid,
        Some(text) => {
            if !placeable {
                return Err(LineError::DecisionUnplaceable);
            }
            let at = whole("decided_at_position", &text)?;
            let at = u64::try_from(at).map_err(|_| LineError::NotAWholeNumber {
                field: "decided_at_position",
                found: text.clone(),
            })?;
            if !batch.holds(at) {
                return Err(LineError::DecidedOutsideTheBatch { at, batch });
            }
            DecidedAt::Position(at)
        }
    };

    let proposal = Proposal {
        desk,
        instrument: InstrumentId::new(&instrument),
        side: match side {
            Emitted::Buy => Side::Buy,
            Emitted::Sell => Side::Sell,
        },
        quantity_scaled: whole("quantity_scaled", &quantity_scaled)?,
        price_scaled,
        cap_scaled,
        client_id,
        // The supervisor writes the position on once it has confirmed the strategy had that far.
        decided_at_position: None,
        decided_at_ns: whole("decided_at_ns", &decided_at_ns)?,
        venue,
        // The supervisor stamps the name it runs the strategy under.
        strategy: None,
        terms,
        reduce_only,
        parent,
        oco,
    };

    Ok(Decided::Proposal { proposal: Box::new(proposal), at: decided_at })
}

/// The order terms a proposal line states. Absent fields are the default terms.
fn terms_of(
    time_in_force: Option<&str>,
    post_only: bool,
    stop_price_scaled: Option<&str>,
) -> Result<bijective_model::OrderTerms, LineError> {
    let time_in_force = match time_in_force {
        None => bijective_model::TimeInForce::default(),
        Some(name) => bijective_model::TimeInForce::parse(name)
            .ok_or_else(|| LineError::UnknownTimeInForce { found: name.to_string() })?,
    };
    let stop_price_scaled = optional_whole("stop_price_scaled", stop_price_scaled)?;
    Ok(bijective_model::OrderTerms {
        time_in_force,
        post_only,
        stop_price_scaled,
        expire_at_ns: None,
    })
}

/// `NotNumbered` is an absent field, not zero, which would read as the first update ever sent.
fn sequence_of(update: &BookUpdate) -> Option<Wide> {
    match update.seq {
        Sequence::Numbered(n) => Some(n.to_string()),
        Sequence::NotNumbered => None,
    }
}

fn whole(field: &'static str, text: &str) -> Result<i128, LineError> {
    text.parse::<i128>().map_err(|_| LineError::NotAWholeNumber { field, found: text.to_string() })
}

fn optional_whole(field: &'static str, text: Option<&str>) -> Result<Option<i128>, LineError> {
    text.map(|t| whole(field, t)).transpose()
}

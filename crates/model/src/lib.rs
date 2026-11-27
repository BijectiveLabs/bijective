// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The platform's own objects: what it intends, what it records as held, and as of when.
//! No dependencies, by design: a type every program links must not drag anything in.

pub mod market;

/// An order as the platform states it. Quantities are scaled integers, never floats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRequest {
    /// The identifier we mint. Checked against the venue's rules before it is sent.
    pub client_id: String,
    /// The venue's own symbol, typed so it cannot be passed where an [`InstrumentId`] belongs.
    pub instrument: VenueSymbol,
    pub side: Side,
    pub quantity_scaled: i128,
    /// `None` for a market order.
    pub price_scaled: Option<i128>,
    pub reduce_only: bool,
    pub terms: OrderTerms,
}

/// How long an order may rest, and whether it may take liquidity (decision 0009). The default is
/// what every limit order was before terms were stated: good till cancelled, free to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OrderTerms {
    pub time_in_force: TimeInForce,
    /// Rests or is cancelled. Never takes liquidity.
    pub post_only: bool,
    /// A stop (decision 0012): the order is held at the venue until the price reaches this, at
    /// the instrument's price scale. A buy when it rises to it, a sell when it falls to it. With
    /// a limit price it is a stop-limit, without one a stop-market.
    pub stop_price_scaled: Option<i128>,
    /// When the node cancels the order if it is still working, in UTC nanoseconds on the node's
    /// clock (good till date, emulated by the platform). The venue holds it as good till
    /// cancelled.
    pub expire_at_ns: Option<i128>,
}

/// Pinned codes, never declaration order: the codes are written to the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeInForce {
    #[default]
    GoodTillCancelled,
    ImmediateOrCancel,
    FillOrKill,
    Day,
    AtTheOpen,
    AtTheClose,
}

impl TimeInForce {
    const ALL: [(Self, u8, &'static str); 6] = [
        (Self::GoodTillCancelled, 0, "gtc"),
        (Self::ImmediateOrCancel, 1, "ioc"),
        (Self::FillOrKill, 2, "fok"),
        (Self::Day, 3, "day"),
        (Self::AtTheOpen, 4, "at_the_open"),
        (Self::AtTheClose, 5, "at_the_close"),
    ];

    #[must_use]
    pub fn code(self) -> u8 {
        Self::ALL.iter().find(|(t, _, _)| *t == self).map_or(0, |(_, c, _)| *c)
    }

    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(t, _, _)| *t)
    }

    /// The name strategies write and records show.
    #[must_use]
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(t, _, _)| *t == self).map_or("gtc", |(_, _, n)| n)
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, _, n)| *n == name).map(|(t, _, _)| *t)
    }

    /// Whether an order under it may rest on the book at all.
    #[must_use]
    pub fn may_rest(self) -> bool {
        !matches!(self, Self::ImmediateOrCancel | Self::FillOrKill)
    }
}

/// A venue's own symbol. Wire use only, kept distinct from [`InstrumentId`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VenueSymbol(String);

impl VenueSymbol {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for VenueSymbol {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for VenueSymbol {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl VenueSymbol {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl core::fmt::Display for VenueSymbol {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The platform's own identity for one economic exposure. Venue symbols map to it by declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstrumentId(String);

impl InstrumentId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for InstrumentId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for InstrumentId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl InstrumentId {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl core::fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// A normalised signal: a producer of proposals, never an actuator. The instrument is the
/// platform's identity. A venue is named only when the decision depends on where it executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Which desk it belongs to. An unassigned proposal is refused, never defaulted.
    pub desk: String,
    pub instrument: InstrumentId,
    pub side: Side,
    /// In the instrument's quantity scale.
    pub quantity_scaled: i128,
    /// The limit price in the instrument's price scale. `None` is a market order.
    pub price_scaled: Option<i128>,
    /// Required for market orders: their cost is unbounded at submission.
    pub cap_scaled: Option<i128>,
    pub client_id: String,
    /// Market position the decision was made on. `None` means unsaid, not position zero.
    pub decided_at_position: Option<u64>,
    /// When the strategy decided, on the run's clock. Never the submission time,
    /// which would fold the submission delay into the execution quality attribution benchmark.
    pub decided_at_ns: i128,
    /// The venue the decision was made for (an arbitrage leg, a venue's fee or liquidity).
    /// `None` leaves the choice to the desk's venue preference. A named venue is never
    /// replaced by another: it is admitted there or refused.
    pub venue: Option<String>,
    /// The strategy that made it, stamped by the strategy engine from the name it runs the
    /// strategy under. Never read from what the strategy wrote. `None` on a proposal written by
    /// anything else, or before proposals carried it.
    pub strategy: Option<String>,
    /// Carried to the order unchanged. Records written before terms read as the default.
    pub terms: OrderTerms,
    /// Only reduces the position the record holds (decision 0012): an exit or a protective stop.
    pub reduce_only: bool,
    /// The proposal this one waits on, by the identifier the strategy proposed it under (decision
    /// 0013): held by the platform until that proposal's orders fill, then released for the
    /// quantity filled. A proposal with a parent reduces only.
    pub parent: Option<String>,
    /// The one-cancels-other group it belongs to, scoped to its parent (decision 0013): once an
    /// order of one member fills, the other members' orders are cancelled and never released.
    pub oco: Option<String>,
}

/// Who an order was placed for: the desk its proposal named and the strategy that made it. The
/// platform's own placement record, kept with the intent, so exposure can be measured per desk
/// and per strategy from the fills of the orders each placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub desk: String,
    pub strategy: Option<String>,
}

impl Proposal {
    /// The price a spend is computed against: the limit, or the cap a market order declared.
    #[must_use]
    pub fn priced_at(&self) -> Option<i128> {
        self.price_scaled.or(self.cap_scaled)
    }
}

/// One position as of a moment. Ours and the venue's are separate types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSnapshot {
    /// Signed: negative is short.
    pub instrument: VenueSymbol,
    pub quantity_scaled: i128,
    pub entry_price_scaled: i128,
}

/// A position as the platform holds it, resolved to the platform's identity.
/// The identity is stamped by the reader from the catalogue it read under, never by the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPosition {
    /// Which venue reported it, stamped by the reader from the map key it iterated.
    pub venue: String,
    pub instrument: InstrumentId,
    pub quantity_scaled: i128,
    pub entry_price_scaled: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSnapshot {
    /// When the reading was taken, by the reader's clock. Reconciliation needs it.
    pub observed_at_ns: i128,
    pub positions: Vec<PositionSnapshot>,
    /// What backs the account's positions, where the venue states it. `None` where it does not.
    pub margin: Option<AccountMargin>,
}

/// What backs an account's positions and what the venue requires to keep them open, as the venue
/// states them for the whole account, in one currency, both at `scale` decimal places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountMargin {
    /// Lowercase, as the platform names assets (`usd`).
    pub currency: String,
    /// The equity the venue counts as margin: below the maintenance figure it may liquidate.
    pub equity_scaled: i128,
    pub maintenance_scaled: i128,
    pub scale: u8,
}

/// One normalised top-of-book observation. Venue specifics have terminated before it exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookTop {
    pub instrument: VenueSymbol,
    pub sequence: Sequence,
    pub venue_ts_ms: u64,
    pub bid_price: i128,
    pub bid_quantity: i128,
    pub ask_price: i128,
    pub ask_quantity: i128,
    pub bid_depth: u32,
    pub ask_depth: u32,
}

/// What a venue numbered an update, where it numbers at all.
/// Not a `u64` with zero for venues that do not: a gap check on a figure that was never observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequence {
    Numbered(u64),
    /// The venue publishes no ordering, so continuity cannot be checked from the data.
    NotNumbered,
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

/// Evidence that a risk evaluation cleared. The only key the order path accepts.
/// Two constructors: `granted` from a clear evaluation, and `for_operator_reduction` for the orders
/// a signed flatten command places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clearance {
    granted_at_ns: i128,
    /// Private, so no struct literal outside this crate can produce one.
    seal: (),
}

impl Clearance {
    /// Mint a clearance, or refuse.
    /// All conditions, not any: an unjudged limit or stale figures mean the platform cannot say it is within limits.
    #[must_use]
    pub fn granted(breaches: usize, unjudged: usize, fresh: bool, at_ns: i128) -> Option<Self> {
        if breaches == 0 && unjudged == 0 && fresh {
            Some(Self { granted_at_ns: at_ns, seal: () })
        } else {
            None
        }
    }

    /// For a reduce-only order an operator's flatten command placed, which the order path has
    /// judged against its record (decision 0010). Not from the figures: a breach or stale
    /// figures are why positions are closed, so they cannot be what stops the closing.
    #[must_use]
    pub fn for_operator_reduction(at_ns: i128) -> Self {
        Self { granted_at_ns: at_ns, seal: () }
    }

    pub fn granted_at_ns(&self) -> i128 {
        self.granted_at_ns
    }
}

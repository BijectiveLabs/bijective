// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order state machine. Every change is an event applied to an order, producing a new order
//! and one audit transition, or a refusal that leaves the order as it was.
//!
//! `Unknown` is a state, not an error: a timeout or an unreadable answer says nothing about whether
//! the order reached the venue, so it is neither filled nor failed until the venue's own evidence
//! moves it, directly or through `Reconciling`. Terminal states still take late fills, because the
//! venue is the record of what executed and a cancel can cross a fill in flight.

use bijective_model::{OrderRequest, Side};

use crate::fill::Fill;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrderState {
    /// Cleared and recorded. Not yet sent.
    Created,
    /// Sent. The venue has not answered.
    Submitted,
    Accepted,
    PartiallyFilled,
    /// A cancel was sent and the venue has not confirmed it. The order may still fill.
    CancelPending,
    Filled,
    Rejected,
    Canceled,
    Expired,
    /// Never sent: the call was held back before it left. No venue ever held it.
    NotSent,
    Unknown,
    Reconciling,
}

impl OrderState {
    /// No further transition but a late fill. `Unknown` and `Reconciling` are never terminal.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Rejected | Self::Canceled | Self::Expired | Self::NotSent
        )
    }

    /// Capital may be at risk: the order may be working, or may have executed unseen.
    pub fn may_execute(self) -> bool {
        matches!(
            self,
            Self::Submitted
                | Self::Accepted
                | Self::PartiallyFilled
                | Self::CancelPending
                | Self::Unknown
                | Self::Reconciling
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Submitted => "submitted",
            Self::Accepted => "accepted",
            Self::PartiallyFilled => "partially-filled",
            Self::CancelPending => "cancel-pending",
            Self::Filled => "filled",
            Self::Rejected => "rejected",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
            Self::NotSent => "not-sent",
            Self::Unknown => "unknown",
            Self::Reconciling => "reconciling",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderEvent {
    Submitted,
    NotSent,
    /// The venue acknowledged it. The venue's identifier, where the answer carried one.
    Accepted {
        venue_order_id: Option<String>,
    },
    Rejected {
        reason: String,
    },
    Filled(Fill),
    CancelRequested,
    CancelRejected {
        reason: String,
    },
    Canceled,
    Expired,
    /// The answer could not settle whether the order reached the venue.
    OutcomeUnknown {
        detail: String,
    },
    ReconciliationStarted,
}

impl OrderEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Submitted => "order_submitted",
            Self::NotSent => "order_not_sent",
            Self::Accepted { .. } => "order_acked",
            Self::Rejected { .. } => "order_rejected",
            Self::Filled(_) => "order_filled",
            Self::CancelRequested => "cancel_requested",
            Self::CancelRejected { .. } => "cancel_rejected",
            Self::Canceled => "order_canceled",
            Self::Expired => "order_expired",
            Self::OutcomeUnknown { .. } => "outcome_unknown",
            Self::ReconciliationStarted => "reconciliation_started",
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::Rejected { reason } | Self::CancelRejected { reason } => reason.clone(),
            Self::OutcomeUnknown { detail } => detail.clone(),
            Self::Filled(fill) => fill.fill_id.clone(),
            _ => String::new(),
        }
    }
}

/// The audit record of one applied event: `(state_from, state_to, reason, venue_order_id, at_ns)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub client_id: String,
    pub from: OrderState,
    pub to: OrderState,
    pub cause: &'static str,
    pub detail: String,
    pub venue_order_id: Option<String>,
    pub at_ns: i128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    /// The event has no meaning in this state, for example an acknowledgement of a filled order.
    NotPermitted { client_id: String, state: OrderState, event: &'static str },
    /// More would be filled than was ordered. Refused: the excess is a venue or a mapping defect.
    Overfill { client_id: String, ordered: i128, filled: i128, offered: i128 },
    /// A fill whose side, figures or venue identifier contradict the order it names.
    Contradiction { client_id: String, detail: String },
    /// A fill that reuses an identity already applied with different content.
    ConflictingFill { client_id: String, fill_id: String },
}

impl core::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotPermitted { client_id, state, event } => {
                write!(f, "{client_id}: {event} is not permitted in state {}", state.name())
            }
            Self::Overfill { client_id, ordered, filled, offered } => write!(
                f,
                "{client_id}: a fill of {offered} on {filled} filled exceeds the {ordered} ordered"
            ),
            Self::Contradiction { client_id, detail } => write!(f, "{client_id}: {detail}"),
            Self::ConflictingFill { client_id, fill_id } => {
                write!(f, "{client_id}: fill {fill_id} was applied before with other content")
            }
        }
    }
}

impl std::error::Error for LifecycleError {}

/// What applying an event did. A fill already applied is a duplicate, never a second fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Boxed: an order carries its fills, and the duplicate case carries nothing.
    Transitioned {
        order: Box<Order>,
        transition: Box<Transition>,
    },
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub request: OrderRequest,
    pub state: OrderState,
    pub venue_order_id: Option<String>,
    pub filled_quantity_scaled: i128,
    /// In the order applied. Invariant: their quantities sum to `filled_quantity_scaled`.
    pub fills: Vec<Fill>,
}

impl Order {
    pub fn new(request: OrderRequest) -> Self {
        Self {
            request,
            state: OrderState::Created,
            venue_order_id: None,
            filled_quantity_scaled: 0,
            fills: Vec::new(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.request.client_id
    }

    pub fn side(&self) -> Side {
        self.request.side
    }

    pub fn remaining_scaled(&self) -> i128 {
        self.request.quantity_scaled - self.filled_quantity_scaled
    }

    /// Pure: the order is not changed. The caller keeps the returned one or discards it.
    pub fn apply(&self, event: &OrderEvent, at_ns: i128) -> Result<Applied, LifecycleError> {
        let mut next = self.clone();
        let to = match event {
            OrderEvent::Filled(fill) => {
                if self.fills.iter().any(|f| f.fill_id == fill.fill_id) {
                    return self.duplicate(fill);
                }
                next.take_fill(fill)?;
                self.state_after_fill(next.filled_quantity_scaled)?
            }
            OrderEvent::Accepted { venue_order_id } => {
                next.adopt_venue_id(venue_order_id.as_deref())?;
                self.next_state(event)?
            }
            _ => self.next_state(event)?,
        };
        next.state = to;
        let transition = Transition {
            client_id: self.client_id().to_string(),
            from: self.state,
            to,
            cause: event.name(),
            detail: event.detail(),
            venue_order_id: next.venue_order_id.clone(),
            at_ns,
        };
        Ok(Applied::Transitioned { order: Box::new(next), transition: Box::new(transition) })
    }

    fn not_permitted(&self, event: &OrderEvent) -> LifecycleError {
        LifecycleError::NotPermitted {
            client_id: self.client_id().to_string(),
            state: self.state,
            event: event.name(),
        }
    }

    /// The transition table for every event but a fill.
    fn next_state(&self, event: &OrderEvent) -> Result<OrderState, LifecycleError> {
        use OrderState as S;
        let working =
            if self.filled_quantity_scaled > 0 { S::PartiallyFilled } else { S::Accepted };
        let to = match (self.state, event) {
            (S::Created, OrderEvent::Submitted) => S::Submitted,
            (S::Created | S::Submitted, OrderEvent::NotSent) => S::NotSent,
            (S::Submitted | S::Unknown | S::Reconciling, OrderEvent::Accepted { .. }) => working,
            (S::Accepted | S::PartiallyFilled, OrderEvent::Accepted { .. }) => self.state,
            (S::Submitted | S::Unknown | S::Reconciling, OrderEvent::Rejected { .. })
                if self.filled_quantity_scaled == 0 =>
            {
                S::Rejected
            }
            (S::Submitted, OrderEvent::OutcomeUnknown { .. }) => S::Unknown,
            (S::Unknown, OrderEvent::ReconciliationStarted) => S::Reconciling,
            (S::Accepted | S::PartiallyFilled, OrderEvent::CancelRequested) => S::CancelPending,
            (S::Unknown | S::Reconciling | S::CancelPending, OrderEvent::CancelRequested) => {
                self.state
            }
            (S::CancelPending, OrderEvent::CancelRejected { .. }) => working,
            (S::Unknown | S::Reconciling, OrderEvent::CancelRejected { .. }) => self.state,
            (
                S::Submitted
                | S::Accepted
                | S::PartiallyFilled
                | S::CancelPending
                | S::Unknown
                | S::Reconciling,
                OrderEvent::Canceled | OrderEvent::Expired,
            ) => {
                if matches!(event, OrderEvent::Canceled) {
                    S::Canceled
                } else {
                    S::Expired
                }
            }
            _ => return Err(self.not_permitted(event)),
        };
        Ok(to)
    }

    /// A fill moves a working order towards `Filled`. On a canceled or expired order it is a late
    /// report of an execution that happened, and changes the state only if it completes the order.
    fn state_after_fill(&self, filled: i128) -> Result<OrderState, LifecycleError> {
        use OrderState as S;
        let complete = filled == self.request.quantity_scaled;
        match self.state {
            S::Submitted | S::Accepted | S::PartiallyFilled | S::Unknown | S::Reconciling => {
                Ok(if complete { S::Filled } else { S::PartiallyFilled })
            }
            S::CancelPending => Ok(if complete { S::Filled } else { S::CancelPending }),
            S::Canceled | S::Expired => Ok(if complete { S::Filled } else { self.state }),
            S::Created | S::Filled | S::Rejected | S::NotSent => {
                Err(LifecycleError::Contradiction {
                    client_id: self.client_id().to_string(),
                    detail: format!("a fill on an order in state {}", self.state.name()),
                })
            }
        }
    }

    fn take_fill(&mut self, fill: &Fill) -> Result<(), LifecycleError> {
        let contradiction = |detail: String| LifecycleError::Contradiction {
            client_id: self.client_id().to_string(),
            detail,
        };
        if fill.fill_id.is_empty() {
            return Err(contradiction("a fill with no identity cannot be deduplicated".into()));
        }
        if fill.side != self.request.side {
            return Err(contradiction(format!("fill {} is on the other side", fill.fill_id)));
        }
        if fill.price_scaled <= 0 || fill.quantity_scaled <= 0 {
            return Err(contradiction(format!(
                "fill {} has a figure that is not positive",
                fill.fill_id
            )));
        }
        let filled = self.filled_quantity_scaled.checked_add(fill.quantity_scaled);
        match filled {
            Some(filled) if filled <= self.request.quantity_scaled => {
                self.adopt_venue_id(fill.venue_order_id.as_deref())?;
                self.filled_quantity_scaled = filled;
                self.fills.push(fill.clone());
                Ok(())
            }
            _ => Err(LifecycleError::Overfill {
                client_id: self.client_id().to_string(),
                ordered: self.request.quantity_scaled,
                filled: self.filled_quantity_scaled,
                offered: fill.quantity_scaled,
            }),
        }
    }

    /// The venue's identifier is learned once. A second, different one is a contradiction.
    fn adopt_venue_id(&mut self, venue_order_id: Option<&str>) -> Result<(), LifecycleError> {
        match (self.venue_order_id.as_deref(), venue_order_id) {
            (_, None) => Ok(()),
            (None, Some(id)) => {
                self.venue_order_id = Some(id.to_string());
                Ok(())
            }
            (Some(held), Some(id)) if held == id => Ok(()),
            (Some(held), Some(id)) => Err(LifecycleError::Contradiction {
                client_id: self.client_id().to_string(),
                detail: format!("venue identifier {id} where {held} is held"),
            }),
        }
    }

    fn duplicate(&self, fill: &Fill) -> Result<Applied, LifecycleError> {
        if self.fills.iter().any(|f| same_execution(f, fill)) {
            Ok(Applied::Duplicate)
        } else {
            Err(LifecycleError::ConflictingFill {
                client_id: self.client_id().to_string(),
                fill_id: fill.fill_id.clone(),
            })
        }
    }
}

/// A second delivery carries its own arrival time. Everything the venue stated must match.
fn same_execution(a: &Fill, b: &Fill) -> bool {
    a.fill_id == b.fill_id
        && a.venue_order_id == b.venue_order_id
        && a.side == b.side
        && a.price_scaled == b.price_scaled
        && a.quantity_scaled == b.quantity_scaled
        && a.fee == b.fee
        && a.liquidity == b.liquidity
        && a.exchange_time == b.exchange_time
}

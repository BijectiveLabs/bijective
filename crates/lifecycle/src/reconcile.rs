// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reconciliation of the ledger against venue evidence, in three phases: orders, then fills, then
//! positions, so each position is judged on orders and fills already brought up to date.
//!
//! The venue is the record of what executed. The ledger is the record of what was intended.
//! Evidence moves an order only through the state machine. What evidence cannot settle is a
//! finding for an operator: no fill is invented to make figures agree, and no unknown order is
//! declared failed for being absent from a report.

use crate::ledger::{Ledger, LedgerChange};
use crate::order::{OrderEvent, OrderState, Transition};
use std::collections::BTreeSet;

use bijective_model::VenueSymbol;
use bijective_model::market::ExchangeTime;

use crate::report::{
    FillReport, OrderStatusReport, PositionEvidence, ReportedStatus, VenueEvidence,
};

/// How long after the venue last changed an order its fills may still be missing from its fill
/// history. A venue publishes an order's status before the executions that explain it (measured
/// 2026-11-18: listed 17 to 127 s after the status showed the order filled), so a gap this young
/// is a read taken too early, not a divergence. Older, it is a divergence.
pub const FILL_LAG_NS: i128 = 180_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The venue holds an order or fill this platform did not place. Capital moved unseen.
    External {
        venue_order_id: String,
        detail: String,
    },
    /// The venue reports more filled than the fills it reported account for.
    MissingFills {
        client_id: String,
        reported_scaled: i128,
        held_scaled: i128,
    },
    /// The venue reports more filled than its fills explain, within `FILL_LAG_NS` of its last
    /// change to the order: the fills are expected on a later read. Not agreement either.
    FillsPending {
        client_id: String,
        reported_scaled: i128,
        held_scaled: i128,
    },
    /// The venue's position differs from the ledger's on an instrument with fills pending.
    PositionAwaitingFills {
        instrument: String,
        venue_scaled: i128,
        ledger_scaled: i128,
    },
    /// The report says less is filled than the ledger holds: an older snapshot, never a correction.
    StaleReport {
        client_id: String,
        reported_scaled: i128,
        held_scaled: i128,
    },
    /// The venue's explicit position differs from the one built from fills.
    PositionMismatch {
        instrument: String,
        venue_scaled: i128,
        ledger_scaled: i128,
    },
    /// The ledger holds a position the venue reported nothing about. Absence is not flat.
    PositionUnconfirmed {
        instrument: String,
        ledger_scaled: i128,
    },
    PositionsNotPublished,
    /// An order the evidence did not settle stays unknown. It is never assumed failed.
    Unresolved {
        client_id: String,
    },
    /// Evidence the state machine refused, with its reason.
    Refused {
        detail: String,
    },
    Incomplete,
}

/// One sentence per finding, with both figures, for the operator who resolves it.
impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::External { venue_order_id, detail } => {
                write!(f, "external {venue_order_id}: {detail}")
            }
            Self::MissingFills { client_id, reported_scaled, held_scaled } => write!(
                f,
                "{client_id}: the venue reports {reported_scaled} filled, fills explain {held_scaled}"
            ),
            Self::FillsPending { client_id, reported_scaled, held_scaled } => write!(
                f,
                "{client_id}: the venue reports {reported_scaled} filled, fills explain {held_scaled} so far"
            ),
            Self::PositionAwaitingFills { instrument, venue_scaled, ledger_scaled } => write!(
                f,
                "{instrument}: the venue holds {venue_scaled}, fills built {ledger_scaled}, fills pending"
            ),
            Self::StaleReport { client_id, reported_scaled, held_scaled } => write!(
                f,
                "{client_id}: a snapshot of {reported_scaled} filled is older than the {held_scaled} held"
            ),
            Self::PositionMismatch { instrument, venue_scaled, ledger_scaled } => write!(
                f,
                "{instrument}: the venue holds {venue_scaled}, fills built {ledger_scaled}"
            ),
            Self::PositionUnconfirmed { instrument, ledger_scaled } => {
                write!(f, "{instrument}: {ledger_scaled} held and not reported by the venue")
            }
            Self::PositionsNotPublished => write!(f, "the venue published no positions"),
            Self::Unresolved { client_id } => {
                write!(f, "{client_id}: still unknown after the evidence")
            }
            Self::Refused { detail } => write!(f, "evidence refused: {detail}"),
            Self::Incomplete => write!(f, "the evidence was incomplete"),
        }
    }
}

impl Finding {
    /// A stale report is information about the venue's snapshot, not a divergence.
    pub fn blocks(&self) -> bool {
        !matches!(self, Self::StaleReport { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every order, fill and position agrees, on complete evidence.
    Aligned,
    /// A finding stands that an operator must resolve.
    Diverged,
    /// Nothing contradicts, but the evidence was incomplete or published no positions.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    pub ledger: Ledger,
    pub applied: Vec<Transition>,
    /// The events that moved an order, in the order applied. Replaying them on the ledger passed
    /// in reproduces `ledger`, so a caller records these and its replay agrees with this pass.
    pub events: Vec<(String, OrderEvent)>,
    pub findings: Vec<Finding>,
    pub verdict: Verdict,
}

/// Pure: the ledger passed in is not changed. The result carries the reconciled copy.
pub fn reconcile(ledger: &Ledger, evidence: &VenueEvidence, at_ns: i128) -> Reconciliation {
    let mut pass = Pass {
        ledger: ledger.clone(),
        applied: Vec::new(),
        events: Vec::new(),
        findings: Vec::new(),
        awaiting: BTreeSet::new(),
        at_ns,
    };
    for report in &evidence.orders {
        pass.acknowledge(report);
    }
    for report in in_event_order(&evidence.fills) {
        match &report.client_id {
            Some(id) if pass.ledger.order(id).is_some() => {
                pass.apply(id, &OrderEvent::Filled(report.fill.clone()));
            }
            _ => pass.findings.push(Finding::External {
                venue_order_id: report.fill.venue_order_id.clone().unwrap_or_default(),
                detail: format!("fill {} names no order of this platform", report.fill.fill_id),
            }),
        }
    }
    for report in &evidence.orders {
        pass.close(report);
    }
    pass.positions(&evidence.positions);
    pass.unresolved();
    if !evidence.complete {
        pass.findings.push(Finding::Incomplete);
    }
    pass.finish()
}

/// Fills in the order they executed at the venue, the identity breaking ties, so positions built
/// from FIFO lots do not depend on the order an adapter listed them in. A fill whose venue stamp is
/// not published is placed by when it was received.
fn in_event_order(fills: &[FillReport]) -> Vec<&FillReport> {
    let mut ordered: Vec<&FillReport> = fills.iter().collect();
    ordered.sort_by(|a, b| {
        let at = |f: &FillReport| match f.fill.exchange_time {
            ExchangeTime::At { at_ns } => at_ns,
            ExchangeTime::NotPublished => f.fill.received_at_ns,
        };
        at(a).cmp(&at(b)).then_with(|| a.fill.fill_id.cmp(&b.fill.fill_id))
    });
    ordered
}

struct Pass {
    ledger: Ledger,
    applied: Vec<Transition>,
    events: Vec<(String, OrderEvent)>,
    findings: Vec<Finding>,
    /// Instruments with an order whose fills are pending. Their positions cannot agree yet.
    awaiting: BTreeSet<VenueSymbol>,
    at_ns: i128,
}

impl Pass {
    fn apply(&mut self, client_id: &str, event: &OrderEvent) {
        match self.ledger.apply(client_id, event, self.at_ns) {
            Ok(LedgerChange::Applied { transition, .. }) => {
                self.applied.push(*transition);
                self.events.push((client_id.to_string(), event.clone()));
            }
            Ok(LedgerChange::Duplicate) => {}
            Err(e) => self.findings.push(Finding::Refused { detail: e.to_string() }),
        }
    }

    fn state_of(&self, client_id: &str) -> Option<OrderState> {
        self.ledger.order(client_id).map(|o| o.state)
    }

    /// Phase one: the venue holds the order, or refused it. An unknown order enters
    /// reconciliation first, so its record shows evidence, not a guess, settled it.
    fn acknowledge(&mut self, report: &OrderStatusReport) {
        let Some(id) = report.client_id.as_deref().filter(|id| self.ledger.order(id).is_some())
        else {
            self.findings.push(Finding::External {
                venue_order_id: report.venue_order_id.clone(),
                detail: format!(
                    "{:?} order on {} names no order of this platform",
                    report.status, report.instrument
                ),
            });
            return;
        };
        if self.state_of(id) == Some(OrderState::Unknown) {
            self.apply(id, &OrderEvent::ReconciliationStarted);
        }
        let event = match report.status {
            ReportedStatus::Rejected => {
                OrderEvent::Rejected { reason: "reported rejected by the venue".into() }
            }
            _ => OrderEvent::Accepted { venue_order_id: Some(report.venue_order_id.clone()) },
        };
        let settled = matches!(self.state_of(id), Some(s) if s.is_terminal());
        let already = matches!(
            self.state_of(id),
            Some(OrderState::Accepted | OrderState::PartiallyFilled | OrderState::CancelPending)
        );
        if !settled && !(already && matches!(event, OrderEvent::Accepted { .. })) {
            self.apply(id, &event);
        }
    }

    /// Phase three for orders: terminal statuses after fills, so a late fill counts first. Then
    /// the reported filled quantity against the fills that explain it.
    fn close(&mut self, report: &OrderStatusReport) {
        let Some(id) = report.client_id.as_deref() else { return };
        let Some(order) = self.ledger.order(id) else { return };
        if report.ordered_quantity_scaled != order.request.quantity_scaled {
            self.findings.push(Finding::Refused {
                detail: format!(
                    "{id}: the venue holds an order of {}, the platform placed {}",
                    report.ordered_quantity_scaled, order.request.quantity_scaled
                ),
            });
        }
        let held = order.filled_quantity_scaled;
        if report.filled_quantity_scaled > held && self.is_young(report) {
            self.awaiting.insert(report.instrument.clone());
            self.findings.push(Finding::FillsPending {
                client_id: id.to_string(),
                reported_scaled: report.filled_quantity_scaled,
                held_scaled: held,
            });
        } else if report.filled_quantity_scaled > held {
            self.findings.push(Finding::MissingFills {
                client_id: id.to_string(),
                reported_scaled: report.filled_quantity_scaled,
                held_scaled: held,
            });
        } else if report.filled_quantity_scaled < held {
            self.findings.push(Finding::StaleReport {
                client_id: id.to_string(),
                reported_scaled: report.filled_quantity_scaled,
                held_scaled: held,
            });
            return;
        }
        let event = match report.status {
            ReportedStatus::Canceled => OrderEvent::Canceled,
            ReportedStatus::Expired => OrderEvent::Expired,
            ReportedStatus::Working | ReportedStatus::Filled | ReportedStatus::Rejected => return,
        };
        if !order.state.is_terminal() {
            self.apply(id, &event);
        }
    }

    /// The venue changed the order within `FILL_LAG_NS`. An unstamped report is never young: no
    /// age is assumed for it.
    fn is_young(&self, report: &OrderStatusReport) -> bool {
        match report.reported_at {
            ExchangeTime::At { at_ns } => self.at_ns - at_ns < FILL_LAG_NS,
            ExchangeTime::NotPublished => false,
        }
    }

    fn positions(&mut self, evidence: &PositionEvidence) {
        let PositionEvidence::Reported(reports) = evidence else {
            self.findings.push(Finding::PositionsNotPublished);
            return;
        };
        for report in reports {
            let ours = self.ledger.position(&report.instrument).map_or(0, |p| p.quantity_scaled());
            if ours != report.quantity_scaled && self.awaiting.contains(&report.instrument) {
                self.findings.push(Finding::PositionAwaitingFills {
                    instrument: report.instrument.to_string(),
                    venue_scaled: report.quantity_scaled,
                    ledger_scaled: ours,
                });
            } else if ours != report.quantity_scaled {
                self.findings.push(Finding::PositionMismatch {
                    instrument: report.instrument.to_string(),
                    venue_scaled: report.quantity_scaled,
                    ledger_scaled: ours,
                });
            }
        }
        for (instrument, quantity) in self.ledger.open_positions() {
            if !reports.iter().any(|r| &r.instrument == instrument) {
                self.findings.push(Finding::PositionUnconfirmed {
                    instrument: instrument.to_string(),
                    ledger_scaled: quantity,
                });
            }
        }
    }

    fn unresolved(&mut self) {
        let open: Vec<String> = [OrderState::Unknown, OrderState::Reconciling]
            .into_iter()
            .flat_map(|s| {
                self.ledger.orders_in(s).map(|o| o.client_id().to_string()).collect::<Vec<_>>()
            })
            .collect();
        self.findings.extend(open.into_iter().map(|client_id| Finding::Unresolved { client_id }));
    }

    fn finish(self) -> Reconciliation {
        let blocking = self.findings.iter().any(Finding::blocks);
        let unverified = self.findings.iter().all(|f| {
            matches!(
                f,
                Finding::Incomplete
                    | Finding::PositionsNotPublished
                    | Finding::StaleReport { .. }
                    | Finding::FillsPending { .. }
                    | Finding::PositionAwaitingFills { .. }
            )
        });
        let verdict = match (blocking, unverified) {
            (false, _) => Verdict::Aligned,
            (true, true) => Verdict::Unverified,
            (true, false) => Verdict::Diverged,
        };
        Reconciliation {
            ledger: self.ledger,
            applied: self.applied,
            events: self.events,
            findings: self.findings,
            verdict,
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The reads reconciliation and exposure rest on: the venue's order and trade lists, single orders
//! this client placed, and the data service's positions, each paged to its end.

use std::collections::BTreeMap;

use bijective_lifecycle::{OrderStatusReport, PositionEvidence, ReportedStatus};
use bijective_venue::contract::{AccountSnapshot, ConnectError, VenueEvidence};
use bijective_venue::resilience::CallClass;
use reqwest::Method;
use serde_json::Value;

use super::{RestClient, query_value};
use crate::evidence::{
    ClientOf, RowRefused, hold_unconfirmed, order_report, position_page, read_page, trade_reading,
};

const PATH_ORDERS: &str = "/data/orders";
const PATH_ONE_ORDER: &str = "/data/order/";
const PATH_TRADES: &str = "/data/trades";
const PATH_POSITIONS: &str = "/v2/positions";

/// A read longer than this many pages is refused rather than taken partially.
const MAX_PAGES: usize = 50;

/// The data service's largest page (Positions, `limit` at most 1000). `filter_amount=0` asks it
/// not to hide holdings under its default floor of 0.1 shares.
const POSITION_QUERY: &str = "limit=500&filter_type=TOKENS&filter_amount=0";

fn transport(path: &str, detail: impl core::fmt::Display) -> ConnectError {
    ConnectError::Transport { detail: format!("{path}: {detail}") }
}

fn with_cursor(base: &str, name: &str, cursor: Option<&str>) -> String {
    match cursor {
        None => base.to_string(),
        Some(cursor) if base.is_empty() => format!("{name}={}", query_value(cursor)),
        Some(cursor) => format!("{base}&{name}={}", query_value(cursor)),
    }
}

impl RestClient {
    /// Every row of one signed CLOB list, page after page until the venue gives no cursor.
    async fn clob_rows(&self, path: &str, base: &str) -> Result<Vec<Value>, ConnectError> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let query = with_cursor(base, "next_cursor", cursor.as_deref());
            let reply = self
                .signed(CallClass::Read, Method::GET, path, &query, "")
                .await
                .map_err(|e| transport(path, e))?;
            if reply.status != 200 {
                return Err(transport(path, format!("answered {}", reply.status)));
            }
            let page = read_page(&reply.body).map_err(|e| transport(path, e))?;
            rows.extend(page.rows);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(rows),
            }
        }
        Err(transport(path, format!("more than {MAX_PAGES} pages")))
    }

    /// The wallet's holdings from the data service. A wallet that holds an outcome outside the
    /// node's catalogue is withdrawn by the reader, so the wallet should trade for this node only.
    pub async fn positions(&self) -> Result<AccountSnapshot, ConnectError> {
        let base = format!("user={}&{POSITION_QUERY}", self.wallet());
        let mut positions = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let query = with_cursor(&base, "cursor", cursor.as_deref());
            let reply = self
                .public(&self.data_endpoint, PATH_POSITIONS, &query)
                .await
                .map_err(|held| transport(PATH_POSITIONS, format!("held back: {held}")))?;
            if reply.status != 200 {
                return Err(transport(PATH_POSITIONS, format!("answered {}", reply.status)));
            }
            let page = position_page(&reply.body, self.scales)
                .map_err(|e| transport(PATH_POSITIONS, e))?;
            positions.extend(page.positions);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => {
                    return Ok(AccountSnapshot {
                        observed_at_ns: self.clock.now_ns(),
                        positions,
                        margin: None,
                    });
                }
            }
        }
        Err(transport(PATH_POSITIONS, format!("more than {MAX_PAGES} pages")))
    }

    fn clients(&self) -> ClientOf {
        self.placed.iter().map(|(hash, placed)| (hash.clone(), placed.client_id.clone())).collect()
    }

    /// One order this client placed, read by its hash. `None` when the venue holds no such order:
    /// for an order whose submit went unanswered that settles nothing, so it stays unresolved.
    async fn one_order(&self, hash: &str) -> Result<Option<Value>, ConnectError> {
        let path = format!("{PATH_ONE_ORDER}{hash}");
        let reply = self
            .signed(CallClass::Read, Method::GET, &path, "", "")
            .await
            .map_err(|e| transport(PATH_ONE_ORDER, e))?;
        match reply.status {
            200 => serde_json::from_str(&reply.body)
                .map(Some)
                .map_err(|_| transport(PATH_ONE_ORDER, "the order could not be read")),
            404 => Ok(None),
            status => Err(transport(PATH_ONE_ORDER, format!("answered {status}"))),
        }
    }

    /// Orders: the live list, then every order this client placed that the list no longer holds,
    /// read one by one until the venue reports it terminal.
    async fn order_reports(
        &mut self,
        clients: &ClientOf,
        refused: &mut Vec<RowRefused>,
    ) -> Result<Vec<OrderStatusReport>, ConnectError> {
        let mut orders = Vec::new();
        for row in self.clob_rows(PATH_ORDERS, "").await? {
            match order_report(&row, clients, self.scales) {
                Ok(report) => orders.push(report),
                Err(e) => refused.push(e),
            }
        }
        let unsettled: Vec<String> = self
            .placed
            .iter()
            .filter(|(hash, placed)| {
                !placed.settled && !orders.iter().any(|o| &o.venue_order_id == *hash)
            })
            .map(|(hash, _)| hash.clone())
            .collect();
        for hash in unsettled {
            let Some(row) = self.one_order(&hash).await? else { continue };
            match order_report(&row, clients, self.scales) {
                Ok(report) => {
                    if report.status != ReportedStatus::Working
                        && let Some(placed) = self.placed.get_mut(&hash)
                    {
                        placed.settled = true;
                    }
                    orders.push(report);
                }
                Err(e) => refused.push(e),
            }
        }
        Ok(orders)
    }

    /// Orders, the wallet's confirmed trades since `since_ns` as fills, and no positions: the
    /// data service's holdings settle on chain after a match and would read as divergence in
    /// between, so they are not offered as evidence. A trade not yet confirmed is held out of the
    /// fills and its order (decision 0002, rule 6), and the evidence is incomplete until it is.
    pub async fn evidence(
        &mut self,
        since_ns: i128,
    ) -> Result<(VenueEvidence, Vec<RowRefused>), ConnectError> {
        let received_at_ns = self.clock.now_ns();
        let clients = self.clients();
        let mut refused = Vec::new();
        let mut orders = self.order_reports(&clients, &mut refused).await?;

        // One second earlier: the venue's bound is in whole seconds, and a fill read twice is
        // recognised by its identity.
        let after = since_ns.div_euclid(1_000_000_000).saturating_sub(1).max(0);
        let wallet = self.wallet();
        let base = format!("maker_address={wallet}&after={after}");
        let mut fills = Vec::new();
        let mut pending: BTreeMap<String, i128> = BTreeMap::new();
        for row in self.clob_rows(PATH_TRADES, &base).await? {
            match trade_reading(&row, &wallet, &clients, self.scales, received_at_ns) {
                Ok(reading) if reading.settled => fills.extend(reading.fills),
                Ok(reading) => {
                    for report in reading.fills {
                        let hash = report.fill.venue_order_id.unwrap_or_default();
                        *pending.entry(hash).or_default() += report.fill.quantity_scaled;
                    }
                }
                Err(e) => refused.push(e),
            }
        }
        hold_unconfirmed(&mut orders, &pending);
        let named: Vec<String> = orders
            .iter()
            .map(|o| o.venue_order_id.clone())
            .chain(fills.iter().filter_map(|f| f.fill.venue_order_id.clone()))
            .collect();
        for hash in &named {
            self.resolve(hash);
        }
        let evidence = VenueEvidence {
            charges: Vec::new(),
            orders,
            fills,
            positions: PositionEvidence::NotPublished,
            history_from_ns: Some(since_ns),
            complete: refused.is_empty() && pending.is_empty(),
        };
        Ok((evidence, refused))
    }
}

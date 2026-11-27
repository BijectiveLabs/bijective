// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The reads reconciliation and exposure rest on: the open orders, every order this client placed
//! that the open list no longer holds, the account's fill activities and its positions.

use bijective_lifecycle::{OrderStatusReport, PositionEvidence, ReportedStatus};
use bijective_venue::contract::{AccountSnapshot, ConnectError, VenueEvidence};
use bijective_venue::resilience::CallClass;
use reqwest::Method;
use serde_json::Value;

use super::{PATH_ORDERS, RestClient};
use crate::evidence::{
    RowRefused, account_margin, charge_report, fill_report, order_report, position_report,
    reported, utc_text,
};

const PATH_FILLS: &str = "/v2/account/activities/FILL";
/// Regulatory fees, charged once a day and naming no order (`docs/integrations/alpaca.md`).
const PATH_FEES: &str = "/v2/account/activities/FEE";
const PATH_POSITIONS: &str = "/v2/positions";
const PATH_ACCOUNT: &str = "/v2/account";

/// The open list's largest page (List orders: `limit` at most 500). A full page may hide more,
/// so it is read as incomplete rather than whole.
const OPEN_LIMIT: usize = 500;

/// Activities come at most 100 to a page (`page_size`), each page after the last one's `id`.
const ACTIVITY_PAGE: usize = 100;

/// A read longer than this many pages is refused rather than taken partially.
const MAX_PAGES: usize = 50;

pub(super) fn transport(path: &str, detail: impl core::fmt::Display) -> ConnectError {
    ConnectError::Transport { detail: format!("{path}: {detail}") }
}

impl RestClient {
    /// One read that must answer 200 with a JSON list.
    async fn list(&self, path: &str, query: &str) -> Result<Vec<Value>, ConnectError> {
        let reply = self
            .call(CallClass::Read, Method::GET, path, query, "")
            .await
            .map_err(|held| transport(path, format!("held back: {held}")))?;
        if reply.status != 200 {
            return Err(transport(path, format!("answered {}", reply.status)));
        }
        match serde_json::from_str(&reply.body) {
            Ok(Value::Array(rows)) => Ok(rows),
            _ => Err(transport(path, "the answer is not a list")),
        }
    }

    /// The account's margin. An answer that is not one readable account refuses the read.
    pub async fn margin(&self) -> Result<bijective_model::AccountMargin, ConnectError> {
        let reply = self
            .call(CallClass::Read, Method::GET, PATH_ACCOUNT, "", "")
            .await
            .map_err(|held| transport(PATH_ACCOUNT, format!("held back: {held}")))?;
        if reply.status != 200 {
            return Err(transport(PATH_ACCOUNT, format!("answered {}", reply.status)));
        }
        let account: Value = serde_json::from_str(&reply.body)
            .map_err(|_| transport(PATH_ACCOUNT, "the answer is not an object"))?;
        account_margin(&account).map_err(|e| transport(PATH_ACCOUNT, e))
    }

    /// The account's positions. A row that cannot be read refuses the snapshot: a partial account
    /// would be compared as if it were whole.
    pub async fn positions(&self) -> Result<AccountSnapshot, ConnectError> {
        let rows = self.list(PATH_POSITIONS, "").await?;
        let observed_at_ns = self.clock.now_ns();
        let positions = rows
            .iter()
            .map(|row| position_report(row, self.scales))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| transport(PATH_POSITIONS, e))?;
        Ok(AccountSnapshot { observed_at_ns, positions, margin: None })
    }

    /// Open orders, then every tracked order the open list no longer holds, read by its client
    /// identifier until the venue reports it finished.
    async fn order_reports(
        &mut self,
        refused: &mut Vec<RowRefused>,
    ) -> Result<Vec<OrderStatusReport>, ConnectError> {
        let query = format!("status=open&limit={OPEN_LIMIT}&direction=asc");
        let rows = self.list(PATH_ORDERS, &query).await?;
        if rows.len() >= OPEN_LIMIT {
            refused.push(RowRefused {
                field: "orders",
                detail: format!("{OPEN_LIMIT} open orders: the list may hold more"),
            });
        }
        let mut orders = Vec::new();
        for row in &rows {
            self.read_order(row, &mut orders, refused);
        }
        let unsettled: Vec<String> = self
            .tracked
            .iter()
            .filter(|(client_id, t)| {
                !t.settled && !orders.iter().any(|o| o.client_id.as_deref() == Some(client_id))
            })
            .map(|(client_id, _)| client_id.clone())
            .collect();
        for client_id in unsettled {
            let Some(row) = self.order_by_client(&client_id).await? else { continue };
            let before = orders.len();
            self.read_order(&row, &mut orders, refused);
            let finished = orders.get(before).is_some_and(|o| o.status != ReportedStatus::Working);
            if let Some(tracked) = self.tracked.get_mut(&client_id) {
                tracked.settled = finished;
            }
        }
        Ok(orders)
    }

    fn read_order(
        &mut self,
        row: &Value,
        orders: &mut Vec<OrderStatusReport>,
        refused: &mut Vec<RowRefused>,
    ) {
        match order_report(row, self.scales) {
            Ok(report) => {
                if let Some(client_id) = &report.client_id {
                    self.client_of.insert(report.venue_order_id.clone(), client_id.clone());
                    if let Some(tracked) = self.tracked.get_mut(client_id) {
                        tracked.order_id = Some(report.venue_order_id.clone());
                    }
                }
                orders.push(report);
            }
            Err(e) => refused.push(e),
        }
    }

    /// Every activity at `path` created after `since_ns`, oldest first, page after page.
    async fn activity_rows(&self, path: &str, since_ns: i128) -> Result<Vec<Value>, ConnectError> {
        // One second earlier: the bound is in whole seconds, and a fill read twice is recognised
        // by its identity.
        let after = utc_text(since_ns.saturating_sub(1_000_000_000));
        let base = format!("after={after}&direction=asc&page_size={ACTIVITY_PAGE}");
        let mut rows = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let query = match &token {
                Some(token) => format!("{base}&page_token={}", super::query_value(token)),
                None => base.clone(),
            };
            let page = self.list(path, &query).await?;
            let last = page.last().and_then(|r| r.get("id")).and_then(Value::as_str);
            token = last.map(str::to_string);
            let full = page.len() >= ACTIVITY_PAGE;
            rows.extend(page);
            if !full {
                return Ok(rows);
            }
        }
        Err(transport(path, format!("more than {MAX_PAGES} pages")))
    }

    /// The client identifier of an order a fill names that no read has shown yet.
    async fn learn_order(&mut self, order_id: &str) -> Result<(), RowRefused> {
        let path = format!("{PATH_ORDERS}/{order_id}");
        let reply = self
            .call(CallClass::Read, Method::GET, &path, "", "")
            .await
            .map_err(|held| RowRefused { field: "order_id", detail: format!("held: {held}") })?;
        if reply.status != 200 {
            let detail = format!("{order_id}: the order read answered {}", reply.status);
            return Err(RowRefused { field: "order_id", detail });
        }
        self.learn_named(&reply.body);
        Ok(())
    }

    fn learn_named(&mut self, body: &str) {
        let order: Option<Value> = serde_json::from_str(body).ok();
        let field = |name: &str| {
            order.as_ref().and_then(|o| o.get(name)).and_then(Value::as_str).map(str::to_string)
        };
        if let (Some(id), Some(client_id)) = (field("id"), field("client_order_id")) {
            self.client_of.insert(id, client_id);
        }
    }

    /// Orders, fills and fee charges since `since_ns`, and positions. Positions are evidence here: the account's
    /// position moves with each fill, with nothing settling in between.
    pub async fn evidence(
        &mut self,
        since_ns: i128,
    ) -> Result<(VenueEvidence, Vec<RowRefused>), ConnectError> {
        let received_at_ns = self.clock.now_ns();
        let mut refused = Vec::new();
        let orders = self.order_reports(&mut refused).await?;
        let rows = self.activity_rows(PATH_FILLS, since_ns).await?;
        for row in &rows {
            let order_id = row.get("order_id").and_then(Value::as_str).unwrap_or_default();
            if !order_id.is_empty()
                && !self.client_of.contains_key(order_id)
                && let Err(e) = self.learn_order(order_id).await
            {
                refused.push(e);
            }
        }
        let mut fills = Vec::new();
        for row in &rows {
            match fill_report(row, &self.client_of, self.scales, received_at_ns) {
                Ok(fill) => fills.push(fill),
                Err(e) => refused.push(e),
            }
        }
        let mut charges = Vec::new();
        for row in &self.activity_rows(PATH_FEES, since_ns).await? {
            match charge_report(row) {
                Ok(charge) => charges.push(charge),
                Err(e) => refused.push(e),
            }
        }
        let account = self.positions().await?;
        let evidence = VenueEvidence {
            orders,
            fills,
            charges,
            positions: PositionEvidence::Reported(account.positions.iter().map(reported).collect()),
            history_from_ns: Some(since_ns),
            complete: refused.is_empty(),
        };
        Ok((evidence, refused))
    }
}

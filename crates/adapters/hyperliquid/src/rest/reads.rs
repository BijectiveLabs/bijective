// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account's records read as venue evidence: the order history, the platform's own orders
//! the history no longer lists asked for by cloid, fills and funding since a bound, and
//! positions. A read the venue refuses fails the pass. A row that cannot be judged is returned
//! beside the evidence and marks it incomplete.

use std::collections::BTreeSet;

use bijective_lifecycle::{
    OrderStatusReport, PositionEvidence, PositionReport, ReportedStatus, VenueEvidence,
};
use bijective_model::market::ExchangeTime;
use bijective_venue::contract::{ConnectError, VenueSymbol};
use bijective_venue::resilience::CallClass;
use serde_json::Value;

use super::{PATH_INFO, RestClient};
use crate::evidence::{
    Clients, RowRefused, fill_report, funding_charge, keep_latest, order_report,
};

/// Pages read per time-ranged list in one pass. A page holds at least 500 rows (Info endpoint,
/// read 2026-11-23), so 25 cover more than the 10,000 fills the venue keeps.
const MAX_PAGES: usize = 25;
const SMALLEST_PAGE: usize = 500;
/// The venue keeps only the 10,000 most recent fills. A read that reaches as many may be missing
/// older ones, so it is not complete.
const FILLS_KEPT: usize = 10_000;
/// The platform's orders asked for by cloid in one pass. More is left for the next pass and the
/// evidence is incomplete meanwhile.
const MAX_LOOKUPS: usize = 20;

fn transport(detail: impl Into<String>) -> ConnectError {
    ConnectError::Transport { detail: detail.into() }
}

fn millis(ns: i128) -> Result<u64, ConnectError> {
    u64::try_from(ns.div_euclid(1_000_000)).map_err(|_| transport("a bound before the epoch"))
}

fn fill_key(row: &Value) -> String {
    row.get("tid").map(Value::to_string).unwrap_or_default()
}

fn funding_key(row: &Value) -> String {
    let coin = row.pointer("/delta/coin").map(Value::to_string).unwrap_or_default();
    let time = row.get("time").map(Value::to_string).unwrap_or_default();
    format!("{coin}:{time}")
}

/// Kept: an order still working, or one whose latest change is inside the window. One that
/// finished before the window moved nothing the window covers.
fn in_window(order: &OrderStatusReport, since_ns: i128) -> bool {
    match order.reported_at {
        ExchangeTime::At { at_ns } => order.status == ReportedStatus::Working || at_ns >= since_ns,
        ExchangeTime::NotPublished => true,
    }
}

impl RestClient {
    async fn info(&self, body: &str) -> Result<Value, ConnectError> {
        let reply = self
            .post(CallClass::Read, PATH_INFO, body)
            .await
            .map_err(|held| transport(format!("an evidence read was held back: {held}")))?;
        if !(200..=299).contains(&reply.status) {
            return Err(transport(format!("an evidence read answered {}", reply.status)));
        }
        serde_json::from_str(&reply.body)
            .map_err(|_| transport("an evidence read could not be understood"))
    }

    /// The rows of a time-ranged list from `since_ms` to `now_ms`, oldest first, each once by
    /// `key`. Each page starts at the newest stamp of the one before, since the bound is
    /// inclusive. `false`: the read stopped before the venue ran out of rows.
    async fn ranged(
        &self,
        kind: &str,
        since_ms: u64,
        now_ms: u64,
        key: fn(&Value) -> String,
    ) -> Result<(Vec<Value>, bool), ConnectError> {
        let (mut rows, mut seen, mut start) = (Vec::new(), BTreeSet::new(), since_ms);
        for _ in 0..MAX_PAGES {
            let body = format!(
                "{{\"type\":\"{kind}\",\"user\":\"{}\",\"startTime\":{start},\"endTime\":{now_ms}}}",
                self.account_address
            );
            let answer = self.info(&body).await?;
            let page = answer.as_array().ok_or_else(|| transport(format!("{kind}: no list")))?;
            let before = rows.len();
            rows.extend(page.iter().filter(|row| seen.insert(key(row))).cloned());
            if rows.len() == before || page.len() < SMALLEST_PAGE {
                return Ok((rows, page.len() < SMALLEST_PAGE));
            }
            let newest = page.iter().filter_map(|r| r.get("time").and_then(Value::as_u64)).max();
            start = newest.unwrap_or(start);
        }
        Ok((rows, false))
    }

    /// The history list, then by cloid each order of the platform's that may be live and that
    /// the list no longer holds. `false`: some were left for the next pass.
    async fn orders(
        &self,
        clients: &Clients,
        since_ns: i128,
        refused: &mut Vec<RowRefused>,
    ) -> Result<(Vec<OrderStatusReport>, bool), ConnectError> {
        let body =
            format!("{{\"type\":\"historicalOrders\",\"user\":\"{}\"}}", self.account_address);
        let answer = self.info(&body).await?;
        let listed = answer.as_array().ok_or_else(|| transport("historicalOrders: no list"))?;
        let mut orders = Vec::new();
        for entry in listed {
            match order_report(entry, clients, self.quantity) {
                Ok(report) => keep_latest(&mut orders, report),
                Err(e) => refused.push(e),
            }
        }
        orders.retain(|o| in_window(o, since_ns));
        let named: BTreeSet<String> = orders.iter().filter_map(|o| o.client_id.clone()).collect();
        let missing: Vec<String> = clients
            .unsettled()
            .into_iter()
            .filter(|cloid| clients.client_of(Some(cloid)).is_some_and(|c| !named.contains(&c)))
            .collect();
        for cloid in missing.iter().take(MAX_LOOKUPS) {
            let body = format!(
                "{{\"type\":\"orderStatus\",\"user\":\"{}\",\"oid\":\"{cloid}\"}}",
                self.account_address
            );
            let answer = self.info(&body).await?;
            match answer.get("status").and_then(Value::as_str) {
                Some("order") => match answer
                    .get("order")
                    .map(|o| order_report(o, clients, self.quantity))
                {
                    Some(Ok(report)) => keep_latest(&mut orders, report),
                    Some(Err(e)) => refused.push(e),
                    None => refused.push(RowRefused { field: "order", detail: "absent".into() }),
                },
                Some("unknownOid") => {}
                other => refused.push(RowRefused {
                    field: "status",
                    detail: format!("orderStatus answered {other:?}"),
                }),
            }
        }
        Ok((orders, missing.len() <= MAX_LOOKUPS))
    }

    /// Positions as the account read gives them. Only a held position is listed, so an instrument
    /// of the catalogue the read leaves out is an explicit flat.
    async fn position_reports(
        &self,
        catalogue: &[VenueSymbol],
    ) -> Result<Vec<PositionReport>, ConnectError> {
        let mut positions: Vec<PositionReport> = self
            .positions()
            .await?
            .positions
            .into_iter()
            .map(|p| PositionReport {
                instrument: p.instrument,
                quantity_scaled: p.quantity_scaled,
            })
            .collect();
        for symbol in catalogue {
            if !positions.iter().any(|p| &p.instrument == symbol) {
                positions.push(PositionReport { instrument: symbol.clone(), quantity_scaled: 0 });
            }
        }
        Ok(positions)
    }

    /// Orders, fills and funding since `since_ns`, and positions, judged into venue evidence.
    pub async fn evidence(
        &self,
        clients: &Clients,
        catalogue: &[VenueSymbol],
        since_ns: i128,
        now_ns: i128,
    ) -> Result<(VenueEvidence, Vec<RowRefused>), ConnectError> {
        let (since_ms, now_ms) = (millis(since_ns)?, millis(now_ns)?);
        let mut refused = Vec::new();
        let (orders, looked_up) = self.orders(clients, since_ns, &mut refused).await?;
        let (fill_rows, fills_whole) =
            self.ranged("userFillsByTime", since_ms, now_ms, fill_key).await?;
        let (funding_rows, funding_whole) =
            self.ranged("userFunding", since_ms, now_ms, funding_key).await?;
        let received_at_ns = self.clock.now_ns();
        let mut fills = Vec::new();
        for row in &fill_rows {
            match fill_report(row, clients, self.price, self.quantity, received_at_ns) {
                Ok(report) => fills.push(report),
                Err(e) => refused.push(e),
            }
        }
        let mut charges = Vec::new();
        for row in &funding_rows {
            match funding_charge(row) {
                Ok(charge) => charges.push(charge),
                Err(e) => refused.push(e),
            }
        }
        let positions = PositionEvidence::Reported(self.position_reports(catalogue).await?);
        let whole = looked_up && fills_whole && funding_whole && fill_rows.len() < FILLS_KEPT;
        let evidence = VenueEvidence {
            orders,
            fills,
            charges,
            positions,
            history_from_ns: Some(since_ns),
            complete: whole && refused.is_empty(),
        };
        Ok((evidence, refused))
    }
}

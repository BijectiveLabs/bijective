// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The signed list queries that gather venue evidence, each paged to its end.

use bijective_venue::contract::{ConnectError, VenueEvidence, VenueSymbol};
use bijective_venue::resilience::CallClass;
use serde_json::Value;

use super::RestClient;
use crate::evidence::{
    Gathered, MAX_PAGES, RowRefused, assemble, changed_since, page_query, read_page,
};

const PATH_OPEN_ORDERS: &str = "/v5/order/realtime";
const PATH_ORDER_HISTORY: &str = "/v5/order/history";
const PATH_EXECUTIONS: &str = "/v5/execution/list";
pub(super) const PATH_POSITIONS: &str = "/v5/position/list";
const PATH_WALLET: &str = "/v5/account/wallet-balance";

/// The per-page maxima the V5 list queries accept: 50 orders, 100 executions, 200 positions.
const ORDER_PAGE: u32 = 50;
const EXECUTION_PAGE: u32 = 100;
pub(super) const POSITION_PAGE: u32 = 200;

/// Execution history is queried in windows of at most seven days (V5 Get Trade History).
const WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// About thirteen weeks. An older start is refused rather than read partially.
const MAX_WINDOWS: u64 = 13;

/// The settle coin as the venue requires it: uppercase only (V5 query parameters).
pub(super) fn settle(coin: &str) -> String {
    coin.to_ascii_uppercase()
}

impl RestClient {
    /// Every row of one list query, page after page until the venue gives no cursor.
    pub(super) async fn rows(&self, path: &str, base: &str) -> Result<Vec<Value>, ConnectError> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let query = page_query(base, cursor.as_deref());
            let reply = self
                .signed(CallClass::Read, reqwest::Method::GET, path, &query)
                .await
                .map_err(|held| ConnectError::Transport { detail: format!("{path}: {held}") })?;
            let page = read_page(reply.status, &reply.body).map_err(|e| transport(path, &e))?;
            rows.extend(page.rows);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(rows),
            }
        }
        Err(ConnectError::Transport { detail: format!("{path}: more than {MAX_PAGES} pages") })
    }

    /// The unified account's margin, `None` where the account states none (isolated margin). One
    /// row is expected. None, several, or one that cannot be read is an error.
    pub async fn margin(&self) -> Result<Option<bijective_model::AccountMargin>, ConnectError> {
        let rows = self.rows(PATH_WALLET, "accountType=UNIFIED").await?;
        let [row] = rows.as_slice() else {
            let detail = format!("{PATH_WALLET}: {} account rows, one expected", rows.len());
            return Err(ConnectError::Transport { detail });
        };
        crate::evidence::account_margin(row).map_err(|e| transport(PATH_WALLET, &e))
    }

    /// Orders open and closed, executions since `since_ns`, and positions, judged into
    /// venue evidence. A page the venue refuses fails the read. A row that cannot be judged is
    /// returned beside the evidence and marks it incomplete.
    pub async fn evidence(
        &self,
        settle_coin: &str,
        catalogue: &[VenueSymbol],
        since_ns: i128,
        now_ns: i128,
    ) -> Result<(VenueEvidence, Vec<RowRefused>), ConnectError> {
        let coin = settle(settle_coin);
        let mut gathered = Gathered::default();
        let listed = format!("category=linear&settleCoin={coin}&limit={ORDER_PAGE}");
        gathered.orders.extend(self.rows(PATH_OPEN_ORDERS, &format!("{listed}&openOnly=0")).await?);
        let closed = self.rows(PATH_OPEN_ORDERS, &format!("{listed}&openOnly=1")).await?;
        let since_ms = since_ns.div_euclid(1_000_000);
        gathered.orders.extend(closed.into_iter().filter(|row| changed_since(row, since_ms)));
        for (start, end) in windows(since_ns, now_ns)? {
            let range =
                format!("category=linear&settleCoin={coin}&startTime={start}&endTime={end}");
            // The recent closed list omitted a filled order (observed 2026-11-17). The venue names
            // order history as the place a unified account's closed orders are read.
            let orders = format!("{range}&limit={ORDER_PAGE}");
            gathered.orders.extend(self.rows(PATH_ORDER_HISTORY, &orders).await?);
            let executions = format!("{range}&limit={EXECUTION_PAGE}");
            gathered.executions.extend(self.rows(PATH_EXECUTIONS, &executions).await?);
        }
        let base = format!("category=linear&settleCoin={coin}&limit={POSITION_PAGE}");
        gathered.positions = self.rows(PATH_POSITIONS, &base).await?;
        Ok(assemble(&gathered, catalogue, &self.scales, since_ns, self.clock.now_ns()))
    }
}

/// Seven-day windows covering `[since, now]`, in milliseconds.
pub fn windows(since_ns: i128, now_ns: i128) -> Result<Vec<(u64, u64)>, ConnectError> {
    let to_ms = |ns: i128| u64::try_from(ns.div_euclid(1_000_000)).ok();
    let (Some(since), Some(now)) = (to_ms(since_ns), to_ms(now_ns)) else {
        return Err(ConnectError::Transport { detail: "a history bound before the epoch".into() });
    };
    if since > now {
        return Err(ConnectError::Transport { detail: "the history starts after now".into() });
    }
    let mut out = Vec::new();
    let mut start = since;
    while start <= now {
        if out.len() as u64 == MAX_WINDOWS {
            return Err(ConnectError::Transport {
                detail: format!("history of more than {MAX_WINDOWS} weeks is not read partially"),
            });
        }
        let end = now.min(start + WINDOW_MS);
        out.push((start, end));
        start = end + 1;
    }
    Ok(out)
}

fn transport(path: &str, refused: &RowRefused) -> ConnectError {
    ConnectError::Transport { detail: format!("{path}: {refused}") }
}

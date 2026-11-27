// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Signed REST path. Design notes: `docs/integrations/bybit.md`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bijective_common::clock::Clock;

use bijective_secrets_broker::credential::Credential;
use bijective_venue::contract::{
    AccountSnapshot, Answer, Clearance, ConnectError, OrderRequest, PositionSnapshot, Pushed,
    VenueEvidence, VenueExecution, VenueSymbol,
};
use bijective_venue::normalise::{Rejection, VenueOutcome, parse_scaled};
use bijective_venue::profile::CapabilityDeclaration;
use bijective_venue::resilience::{
    BreakerConfig, CallClass, CallGate, CallResult, GateConfig, Held, HttpReply, Quota,
    call_result_for_http, retry_after_from_header,
};

use crate::evidence::{InstrumentScales, Scales};
use crate::execution::{RET_CODES_RATE_LIMITED, RequestError, order_body, read_http_response};
use crate::sign::{RECV_WINDOW_MS, sign};

mod reads;
pub use reads::windows;

/// A hung submit is an `Unknown` that never resolves. The bound sits near the venue 5s receive window.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Half of the venue's published IP limit of 600 requests in 5 seconds
/// (`https://bybit-exchange.github.io/docs/v5/rate-limit`, read 2026-11-04), spent on reads only:
/// submits are bounded by the control engine's order rate. Five consecutive failures open a path's
/// breaker for 30 seconds. Both numbers are this platform's choice, not the venue's.
/// Time allowed to open the connection, separate from the whole-request bound: a peer that never
/// answers the handshake must not be waited on for the full request time.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest answer body taken. Every answer this client reads is a few kilobytes to a few
/// hundred. A body past this is a defect or an attack, never a larger answer.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The body, or empty when it is longer than the bound or breaks off. An empty body is unreadable to
/// every reader here, which is the honest reading of an answer that could not be taken whole.
async fn body_within_bound(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_BODY_BYTES => {
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return String::from_utf8(body).unwrap_or_default(),
            _ => return String::new(),
        }
    }
}

pub const GATE: GateConfig = GateConfig {
    breaker: BreakerConfig { failures_to_open: 5, open_for_ns: 30_000_000_000 },
    reads: Quota { requests: 300, per_ns: 5_000_000_000 },
};

/// The venue's statement for HTTP 403 "access too frequent": the IP is blocked and every HTTP
/// session should stop for at least ten minutes (same page).
const IP_BLOCK_NS: i128 = 600_000_000_000;

/// The window the IP limit is stated over (600 requests in 5 seconds).
const LIMIT_WINDOW_NS: i128 = 5_000_000_000;

const PATH_CREATE: &str = "/v5/order/create";
const PATH_CANCEL: &str = "/v5/order/cancel";
const PATH_AMEND: &str = "/v5/order/amend";

pub struct RestClient {
    endpoint: String,
    api_key: String,
    secret: Credential,
    /// Each instrument's scales, from the profile (decision 0007).
    scales: InstrumentScales,
    http: reqwest::Client,
    /// Injected so a replay controls every signed timestamp.
    clock: Arc<dyn Clock>,
    /// Limits on this client's own calls. A cancel is never held.
    gate: Mutex<CallGate>,
}

impl RestClient {
    /// The credential is handed in. `Credential` zeroes itself on drop.
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        secret: Credential,
        scales: InstrumentScales,
        timeout: Duration,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ConnectError> {
        // One TLS provider (ring) for the whole adapter keeps a single audit surface.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .use_rustls_tls()
            .build()
            .map_err(|e| ConnectError::Transport { detail: e.to_string() })?;
        let gate =
            CallGate::new(GATE).map_err(|e| ConnectError::Transport { detail: e.to_string() })?;

        Ok(Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            secret,
            scales,
            http,
            clock,
            gate: Mutex::new(gate),
        })
    }

    /// An order on an instrument the profile does not list has no scale to be written at.
    fn listed(&self, instrument: &VenueSymbol) -> Result<Scales, RequestError> {
        self.scales.of(instrument).ok_or(RequestError::UnlistedInstrument)
    }

    fn now_ms(&self) -> u64 {
        (self.clock.now_ns() / 1_000_000) as u64
    }

    /// Every signed call goes through here. `Err` means the call was held back and nothing was sent.
    async fn signed(
        &self,
        class: CallClass,
        method: reqwest::Method,
        path: &str,
        payload: &str,
    ) -> Result<HttpReply, Held> {
        self.gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(class, self.clock.now_ns())?;

        let ts = self.now_ms();
        let signature = sign(self.secret.expose(), ts, &self.api_key, payload);

        let url = if method == reqwest::Method::GET && !payload.is_empty() {
            format!("{}{path}?{payload}", self.endpoint)
        } else {
            format!("{}{path}", self.endpoint)
        };

        let mut request = self
            .http
            .request(method.clone(), url)
            .header("X-BAPI-API-KEY", &self.api_key)
            .header("X-BAPI-TIMESTAMP", ts.to_string())
            .header("X-BAPI-RECV-WINDOW", RECV_WINDOW_MS.to_string())
            .header("X-BAPI-SIGN", signature);

        if method != reqwest::Method::GET {
            request = request.header("Content-Type", "application/json").body(payload.to_string());
        }

        // Transport failure maps to status 0, classified Unknown: the request may have arrived.
        let reply = match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let retry_after_ns = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(retry_after_from_header);
                let body = body_within_bound(response).await;
                HttpReply { status, body, retry_after_ns }
            }
            // The cause is kept for the operator. The body of an answered call is the venue's.
            Err(e) => HttpReply { status: 0, body: e.to_string(), retry_after_ns: None },
        };

        self.gate.lock().unwrap_or_else(PoisonError::into_inner).report(
            class,
            self.clock.now_ns(),
            health_of(&reply),
        );
        Ok(reply)
    }

    /// The venue's own account for reconciliation. Read-only in both directions. Every page is
    /// read, and a row that cannot be read refuses the snapshot: a partial account would be
    /// reconciled as if it were whole.
    pub async fn positions(&self, settle_coin: &str) -> Result<AccountSnapshot, ConnectError> {
        let base = format!(
            "category=linear&settleCoin={}&limit={}",
            reads::settle(settle_coin),
            reads::POSITION_PAGE
        );
        let rows = self.rows(reads::PATH_POSITIONS, &base).await?;
        let observed_at_ns = self.clock.now_ns();
        let mut positions = Vec::new();
        for row in &rows {
            if let Some(position) = self.position_row(row)? {
                positions.push(position);
            }
        }
        Ok(AccountSnapshot { observed_at_ns, positions, margin: None })
    }

    fn position_row(
        &self,
        row: &serde_json::Value,
    ) -> Result<Option<PositionSnapshot>, ConnectError> {
        let refused = |e: crate::evidence::RowRefused| ConnectError::Transport {
            detail: format!("position row: {e}"),
        };
        let report = crate::evidence::position_report(row, &self.scales).map_err(refused)?;
        let price = crate::evidence::scales_of(row, &self.scales).map_err(refused)?.price;
        if report.quantity_scaled == 0 {
            return Ok(None);
        }
        let entry = row
            .get("avgPrice")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ConnectError::Transport {
                detail: "position row: avgPrice absent".into(),
            })
            .and_then(|t| {
                parse_scaled(t, price).map_err(|e| ConnectError::Transport {
                    detail: format!("position row: avgPrice {e:?}"),
                })
            })?;
        Ok(Some(PositionSnapshot {
            instrument: report.instrument,
            quantity_scaled: report.quantity_scaled,
            entry_price_scaled: entry,
        }))
    }

    pub async fn submit(&self, order: &OrderRequest) -> Result<VenueOutcome, RequestError> {
        let scales = self.listed(&order.instrument)?;
        let body = order_body(order, scales.price, scales.quantity)?;
        // Held back, nothing was sent: the platform's own answer, not the venue's.
        let Ok(reply) =
            self.signed(CallClass::Submit, reqwest::Method::POST, PATH_CREATE, &body).await
        else {
            return Ok(VenueOutcome::NotSent);
        };
        Ok(outcome_of(&reply))
    }

    /// An amend that changes nothing is refused here: it would cost queue position with no decision behind it.
    pub async fn amend(
        &self,
        instrument: &VenueSymbol,
        client_id: &str,
        price_scaled: Option<i128>,
        quantity_scaled: Option<i128>,
    ) -> Result<VenueOutcome, RequestError> {
        if price_scaled.is_none() && quantity_scaled.is_none() {
            return Err(RequestError::AmendChangesNothing);
        }
        if quantity_scaled.is_some_and(|q| q <= 0) {
            return Err(RequestError::NonPositiveQuantity);
        }

        let scales = self.listed(instrument)?;
        let mut body = serde_json::json!({
            "category": "linear",
            "symbol": instrument.as_str(),
            "orderLinkId": client_id,
        });
        if let Some(price) = price_scaled {
            body["price"] = serde_json::Value::String(bijective_venue::normalise::render_scaled(
                price,
                scales.price,
            ));
        }
        if let Some(qty) = quantity_scaled {
            body["qty"] = serde_json::Value::String(bijective_venue::normalise::render_scaled(
                qty,
                scales.quantity,
            ));
        }

        let Ok(reply) = self
            .signed(CallClass::Submit, reqwest::Method::POST, PATH_AMEND, &body.to_string())
            .await
        else {
            return Ok(VenueOutcome::NotSent);
        };
        Ok(outcome_of(&reply))
    }

    pub async fn cancel(&self, instrument: &VenueSymbol, client_id: &str) -> VenueOutcome {
        let body = serde_json::json!({
            "category": "linear",
            "symbol": instrument.as_str(),
            "orderLinkId": client_id,
        })
        .to_string();

        // A cancel is never held, so the `Err` arm is not reachable. It answers truthfully anyway.
        match self.signed(CallClass::Cancel, reqwest::Method::POST, PATH_CANCEL, &body).await {
            Ok(reply) => outcome_of(&reply),
            Err(_) => VenueOutcome::NotSent,
        }
    }
}

/// Execution side of the adapter contract over signed REST. The declaration is injected. Venue assurance owns it.
pub struct Execution {
    client: RestClient,
    declaration: CapabilityDeclaration,
    settle_coin: String,
    /// The private stream, when the endpoint has one: it prompts evidence reads and supplies the
    /// fills the venue pushed before its history lists them.
    private_stream: Option<crate::private_stream::PrivateLink>,
}

impl Execution {
    pub fn new(
        client: RestClient,
        declaration: CapabilityDeclaration,
        settle_coin: impl Into<String>,
    ) -> Self {
        Self { client, declaration, settle_coin: settle_coin.into(), private_stream: None }
    }

    /// Keeps the private stream for the life of this adapter.
    #[must_use]
    pub fn with_private_stream(mut self, link: crate::private_stream::PrivateLink) -> Self {
        self.private_stream = Some(link);
        self
    }
}

impl VenueExecution for Execution {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        order: &'a OrderRequest,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move {
            match self.client.submit(order).await {
                Ok(outcome) => outcome,
                // Never reached the venue, so a rejection and not an unknown.
                Err(e) => VenueOutcome::Rejected(Rejection::Unclassified {
                    venue_code: format!("not sent: {e}"),
                }),
            }
        })
    }

    fn cancel<'a>(
        &'a mut self,
        instrument: &'a VenueSymbol,
        client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { self.client.cancel(instrument, client_id).await })
    }

    fn account(&mut self, _now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move {
            let mut snapshot = self.client.positions(&self.settle_coin).await?;
            // Positions do not wait on margin: a margin that cannot be read leaves it unstated,
            // which leaves a declared leverage or margin-use limit unjudged, and that halts.
            snapshot.margin = self.client.margin().await.ok().flatten();
            Ok(snapshot)
        })
    }

    fn pushed(&mut self) -> Pushed {
        self.private_stream
            .as_ref()
            .map_or(Pushed::NotKept, crate::private_stream::PrivateLink::pushed)
    }

    /// Refused rows are carried as the evidence's incompleteness. Each is reported here too, so an
    /// operator sees which field the venue changed.
    fn evidence(
        &mut self,
        since_ns: i128,
        now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        Box::pin(async move {
            let catalogue: Vec<VenueSymbol> =
                self.declaration.instruments().iter().map(|i| i.symbol.clone()).collect();
            let (mut evidence, refused) =
                self.client.evidence(&self.settle_coin, &catalogue, since_ns, now_ns).await?;
            for row in &refused {
                eprintln!("venue evidence row refused: {row}");
            }
            if let Some(link) = &self.private_stream {
                crate::evidence::with_pushed(&mut evidence, &link.pushed_fills(), since_ns);
            }
            Ok(evidence)
        })
    }
}

/// The venue's answer read as an outcome, with the venue's own wait added to a rate limit.
fn outcome_of(reply: &HttpReply) -> VenueOutcome {
    read_http_response(reply.status, &reply.body).with_retry_after(reply.retry_after_ns)
}

/// What the answer says about the venue's health. A 403 is the venue's IP block, not a refusal,
/// and a rate-limit code inside a 200 answer is a request to slow down before the block comes.
fn health_of(reply: &HttpReply) -> CallResult {
    if reply.status == 403 {
        return CallResult::Throttled { retry_after_ns: Some(IP_BLOCK_NS) };
    }
    if reply.status == 200 && is_rate_limit_code(&reply.body) {
        return CallResult::Throttled { retry_after_ns: Some(LIMIT_WINDOW_NS) };
    }
    call_result_for_http(reply.status, reply.retry_after_ns)
}

fn is_rate_limit_code(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|parsed| parsed.get("retCode").and_then(serde_json::Value::as_i64))
        .is_some_and(|code| RET_CODES_RATE_LIMITED.contains(&code))
}

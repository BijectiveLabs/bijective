// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Trading API client: orders placed and cancelled under the account's key headers, and the
//! reads that gather venue evidence. Sources and the questions still open:
//! `docs/integrations/alpaca.md`.
//!
//! The venue carries the platform's client identifier, so an order this process did not place is
//! found by it: the orders a previous process placed are handed back from the record (`adopt`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bijective_common::clock::Clock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    AccountSnapshot, Answer, CapabilityDeclaration, Clearance, ConnectError, OrderRequest, Pushed,
    Recorded, VenueEvidence, VenueExecution, VenueSymbol,
};
use bijective_venue::normalise::{Rejection, Scale, VenueOutcome};
use bijective_venue::resilience::{
    BreakerConfig, CallClass, CallGate, GateConfig, Held, HttpReply, Quota, call_result_for_http,
    retry_after_from_header,
};
use reqwest::Method;

use crate::evidence::{ClientOf, Scales};
use crate::order::{OrderError, order_body, read_cancel, read_submit};

mod reads;
mod session_gate;

const PATH_ORDERS: &str = "/v2/orders";
const PATH_BY_CLIENT: &str = "/v2/orders:by_client_order_id";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Every answer this client reads is a few kilobytes to a few hundred. A body past this is a
/// defect or an attack, never a larger answer.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The Trading API states no request limit on the pages read (its `X-RateLimit-*` headers are
/// documented with example values only), so reads are held to 100 a minute, the figure its
/// examples show. Submits are bounded by the control engine's order rate. Five failures opening a
/// path for 30 seconds is this platform's choice.
pub const GATE: GateConfig = GateConfig {
    breaker: BreakerConfig { failures_to_open: 5, open_for_ns: 30_000_000_000 },
    reads: Quota { requests: 100, per_ns: 60_000_000_000 },
};

/// The finest price grid the venue quotes, four decimals below one dollar (Orders at Alpaca).
const MIN_PRICE_SCALE: u8 = 4;

/// One order this client placed or took back from the record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tracked {
    /// The venue's identifier, once an answer or a read named it.
    order_id: Option<String>,
    /// The venue reported it finished. It is not asked about again.
    settled: bool,
}

/// A struct so same-typed arguments cannot be swapped at a call site unnoticed.
pub struct ClientSetup<'a> {
    pub endpoint: &'a str,
    pub key_id: &'a str,
    pub secret: &'a Credential,
    /// At least four places: the venue's grid below one dollar.
    pub price: Scale,
    /// Zero: whole shares only in this build (decision 0004).
    pub quantity: Scale,
    pub timeout: Duration,
    pub clock: Arc<dyn Clock>,
}

/// No `Debug`: it holds the secret key.
pub struct RestClient {
    endpoint: String,
    key_id: String,
    secret: String,
    scales: Scales,
    http: reqwest::Client,
    clock: Arc<dyn Clock>,
    gate: Mutex<CallGate>,
    tracked: BTreeMap<String, Tracked>,
    /// The client identifier of every venue order read, so a fill, which names only the venue's
    /// order, is linked. Order identifiers never change, so an entry is never read twice.
    client_of: ClientOf,
    /// The market phase the venue's clock last stated, held until it changes.
    phase: Option<session_gate::Phase>,
}

fn not_permitted(detail: impl Into<String>) -> ConnectError {
    ConnectError::NotPermitted { detail: detail.into() }
}

fn scales_of(price: Scale, quantity: Scale) -> Result<Scales, ConnectError> {
    if price.digits() < MIN_PRICE_SCALE {
        return Err(not_permitted(format!(
            "the venue quotes to four places below one dollar; a price scale of {} cannot hold it",
            price.digits()
        )));
    }
    if quantity.digits() != 0 {
        return Err(not_permitted("this build trades whole shares: the quantity scale must be 0"));
    }
    Ok(Scales { price, quantity })
}

impl RestClient {
    /// Refuses scales this build cannot trade on before any call is made.
    pub fn new(setup: ClientSetup<'_>) -> Result<Self, ConnectError> {
        // One TLS provider for the whole adapter.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let scales = scales_of(setup.price, setup.quantity)?;
        let secret = String::from_utf8(setup.secret.expose().to_vec())
            .map_err(|_| not_permitted("the secret key is not text"))?;
        let http = reqwest::Client::builder()
            .timeout(setup.timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .use_rustls_tls()
            .build()
            .map_err(|e| ConnectError::Transport { detail: e.to_string() })?;
        let gate =
            CallGate::new(GATE).map_err(|e| ConnectError::Transport { detail: e.to_string() })?;
        Ok(Self {
            endpoint: setup.endpoint.trim_end_matches('/').to_string(),
            key_id: setup.key_id.to_string(),
            secret,
            scales,
            http,
            clock: setup.clock,
            gate: Mutex::new(gate),
            tracked: BTreeMap::new(),
            client_of: BTreeMap::new(),
            phase: None,
        })
    }

    /// One request under the two key headers (Authentication). A transport failure becomes
    /// status 0, read as unknown: the request may have executed. `Err` means it was held back and
    /// nothing was sent.
    async fn call(
        &self,
        class: CallClass,
        method: Method,
        path: &str,
        query: &str,
        body: &str,
    ) -> Result<HttpReply, Held> {
        self.gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(class, self.clock.now_ns())?;
        let url = if query.is_empty() {
            format!("{}{path}", self.endpoint)
        } else {
            format!("{}{path}?{query}", self.endpoint)
        };
        let mut request = self
            .http
            .request(method, url)
            .header("APCA-API-KEY-ID", &self.key_id)
            .header("APCA-API-SECRET-KEY", &self.secret);
        if !body.is_empty() {
            request = request.header("Content-Type", "application/json").body(body.to_string());
        }
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
            Err(e) => HttpReply { status: 0, body: e.to_string(), retry_after_ns: None },
        };
        self.gate.lock().unwrap_or_else(PoisonError::into_inner).report(
            class,
            self.clock.now_ns(),
            call_result_for_http(reply.status, reply.retry_after_ns),
        );
        Ok(reply)
    }

    /// A client identifier is sent once in this process: a second submit is answered as a
    /// duplicate without a request. The venue refuses a reused identifier too (code 40010001),
    /// which reads as `Duplicate` after a restart.
    pub async fn submit(&mut self, order: &OrderRequest) -> Result<VenueOutcome, OrderError> {
        if self.tracked.contains_key(&order.client_id) {
            return Ok(VenueOutcome::Rejected(Rejection::Duplicate));
        }
        let body = order_body(order, self.scales.price, self.scales.quantity)?;
        // An exit is never held by the session: a platform must always be able to reduce.
        if !order.reduce_only
            && let Err(why) = self.core_session().await
        {
            return Ok(unsent(&why));
        }
        // Held before the call: a timeout leaves an order reconciliation must ask for.
        self.tracked.insert(order.client_id.clone(), Tracked { order_id: None, settled: false });
        let Ok(reply) = self.call(CallClass::Submit, Method::POST, PATH_ORDERS, "", &body).await
        else {
            self.tracked.remove(&order.client_id);
            return Ok(VenueOutcome::NotSent);
        };
        let outcome = read_submit(reply.status, &reply.body, &order.client_id)
            .with_retry_after(reply.retry_after_ns);
        match &outcome {
            VenueOutcome::Accepted => self.learn(&order.client_id, &reply.body),
            VenueOutcome::Rejected(_) => {
                self.tracked.remove(&order.client_id);
            }
            _ => {}
        }
        Ok(outcome)
    }

    /// The venue's identifier named in an order object, kept for cancels and fills.
    fn learn(&mut self, client_id: &str, order: &str) {
        let id = serde_json::from_str::<serde_json::Value>(order)
            .ok()
            .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string));
        if let Some(id) = id {
            self.client_of.insert(id.clone(), client_id.to_string());
            if let Some(tracked) = self.tracked.get_mut(client_id) {
                tracked.order_id = Some(id);
            }
        }
    }

    /// Never held: an exit must always be attempted. The venue cancels by its own identifier, so
    /// one this process has not seen is read by the client identifier first.
    pub async fn cancel(&mut self, client_id: &str) -> VenueOutcome {
        let known = self.tracked.get(client_id).and_then(|t| t.order_id.clone());
        let order_id = match known {
            Some(id) => id,
            None => match self.order_by_client(client_id).await {
                Ok(Some(order)) => {
                    self.learn(client_id, &order.to_string());
                    match order.get("id").and_then(|id| id.as_str()) {
                        Some(id) => id.to_string(),
                        None => return unsent("the venue's order names no identifier"),
                    }
                }
                Ok(None) => return unsent("the venue holds no order under this identifier"),
                Err(e) => return unsent(&e.to_string()),
            },
        };
        let path = format!("{PATH_ORDERS}/{order_id}");
        match self.call(CallClass::Cancel, Method::DELETE, &path, "", "").await {
            Ok(reply) => {
                read_cancel(reply.status, &reply.body).with_retry_after(reply.retry_after_ns)
            }
            Err(_) => VenueOutcome::NotSent,
        }
    }

    /// One order read by the platform's identifier. `None` on a 404, the venue's answer for an
    /// identifier it does not hold (observed 2026-11-17, the page documents only a found order).
    /// That settles nothing for an order whose submit went unanswered.
    async fn order_by_client(
        &self,
        client_id: &str,
    ) -> Result<Option<serde_json::Value>, ConnectError> {
        let query = format!("client_order_id={}", query_value(client_id));
        let reply = self
            .call(CallClass::Read, Method::GET, PATH_BY_CLIENT, &query, "")
            .await
            .map_err(|held| reads::transport(PATH_BY_CLIENT, format!("held back: {held}")))?;
        match reply.status {
            200 => serde_json::from_str(&reply.body)
                .map(Some)
                .map_err(|_| reads::transport(PATH_BY_CLIENT, "the order could not be read")),
            404 => Ok(None),
            status => Err(reads::transport(PATH_BY_CLIENT, format!("answered {status}"))),
        }
    }

    /// Orders a previous process placed, from the record. Nothing is requested: the venue holds
    /// them under the same client identifiers.
    pub fn adopt(&mut self, recorded: &[Recorded]) {
        for order in recorded {
            let client_id = order.request.client_id.clone();
            if self.tracked.contains_key(&client_id) {
                continue;
            }
            if let Some(id) = &order.venue_order_id {
                self.client_of.insert(id.clone(), client_id.clone());
            }
            let tracked =
                Tracked { order_id: order.venue_order_id.clone(), settled: order.terminal };
            self.tracked.insert(client_id, tracked);
        }
    }
}

/// Refused before a request existed, so the order's state is unchanged.
fn unsent(detail: &str) -> VenueOutcome {
    VenueOutcome::Rejected(Rejection::Unclassified { venue_code: format!("not sent: {detail}") })
}

/// Percent-encodes everything outside the unreserved set.
fn query_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The body, or empty when it is longer than the bound or breaks off. Empty is unreadable to every
/// reader here, the honest reading of an answer that could not be taken whole.
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

/// The execution side of the adapter contract over the Trading API.
pub struct Execution {
    client: RestClient,
    declaration: CapabilityDeclaration,
    /// The trade updates stream, when the endpoint has one. It prompts evidence reads only.
    trade_stream: Option<crate::trade_stream::TradeLink>,
}

impl Execution {
    /// The declaration is handed in. Venue assurance owns what a venue is declared to do.
    pub fn new(client: RestClient, declaration: CapabilityDeclaration) -> Self {
        Self { client, declaration, trade_stream: None }
    }

    /// Keeps the trade updates stream for the life of this adapter.
    #[must_use]
    pub fn with_trade_stream(mut self, link: crate::trade_stream::TradeLink) -> Self {
        self.trade_stream = Some(link);
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
            // Refused before a request existed: a rejection, not an unknown.
            self.client.submit(order).await.unwrap_or_else(|e| unsent(&e.to_string()))
        })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { self.client.cancel(client_id).await })
    }

    fn account(&mut self, _now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move {
            let mut snapshot = self.client.positions().await?;
            // Positions do not wait on margin: a margin that cannot be read leaves it unstated,
            // which leaves a declared leverage or margin-use limit unjudged, and that halts.
            snapshot.margin = self.client.margin().await.ok();
            Ok(snapshot)
        })
    }

    fn adopt(&mut self, recorded: &[Recorded]) {
        self.client.adopt(recorded);
    }

    fn pushed(&mut self) -> Pushed {
        self.trade_stream.as_ref().map_or(Pushed::NotKept, crate::trade_stream::TradeLink::pushed)
    }

    /// Refused rows are carried as the evidence's incompleteness and reported here too, so an
    /// operator sees which field the venue changed.
    fn evidence(
        &mut self,
        since_ns: i128,
        _now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        Box::pin(async move {
            let (evidence, refused) = self.client.evidence(since_ns).await?;
            for row in &refused {
                eprintln!("venue evidence row refused: {row}");
            }
            Ok(evidence)
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/rest_tests.rs"]
mod tests;

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Signed REST client. Asset index and nonce state live here because both are venue facts
//! and the adapter contract terminates those at the adapter.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bijective_common::clock::Clock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    AccountSnapshot, Answer, CapabilityDeclaration, Clearance, ConnectError, OrderRequest,
    PositionSnapshot, Recorded, Side, VenueEvidence, VenueExecution, VenueSymbol,
};
use bijective_venue::normalise::{Rejection, Scale, VenueOutcome, parse_scaled};
use bijective_venue::resilience::{
    BreakerConfig, CallClass, CallGate, GateConfig, Held, HttpReply, Quota, call_result_for_http,
    retry_after_from_header,
};

use crate::evidence::Clients;
use crate::exchange::read_answer;
use crate::signing::{
    Signer, action_hash, agent_struct_hash, exchange_domain_separator, typed_data_digest,
};
use crate::wire::{Placement, Value, cancel_action, order_action, pack, to_json, wire_decimal};

mod reads;

const PATH_EXCHANGE: &str = "/exchange";
const PATH_INFO: &str = "/info";

const TESTNET_MARKER: &str = "testnet";

/// Refused, not defaulted: the environments sign over different bytes, and a wrong guess
/// surfaces as a missing account.
pub fn environment_of(endpoint: &str) -> Option<bool> {
    let lowered = endpoint.to_ascii_lowercase();
    if !lowered.contains("hyperliquid") {
        return None;
    }
    Some(!lowered.contains(TESTNET_MARKER))
}

/// The venue publishes 1200 REST weight per minute per IP, most `info` requests at weight 20
/// (`docs/integrations/hyperliquid.md`, Limits): 60 reads a minute stay inside it whichever read is
/// made. Exchange actions draw on a separate, earned budget the control engine bounds with the
/// order rate, so submits are not charged here. Five consecutive failures open a path's breaker for
/// 30 seconds. Both numbers are this platform's choice, not the venue's.
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
    reads: Quota { requests: 60, per_ns: 60_000_000_000 },
};

/// Separate from an outcome: a request never built never reached the venue, so it settles the
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    EmptyClientId,
    NonPositiveQuantity,
    /// No market order on this venue.
    NoPrice,
    UnresolvedAsset,
    /// The client's own limits held the call back. Nothing was sent.
    Held(Held),
    Signing(crate::signing::SigningError),
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyClientId => write!(f, "an order must carry a client identifier"),
            Self::NonPositiveQuantity => write!(f, "an order must carry a positive quantity"),
            Self::NoPrice => write!(f, "this venue has no market order, so a price is required"),
            Self::UnresolvedAsset => write!(f, "the venue's list does not name this instrument"),
            Self::Held(held) => write!(f, "the call was held back: {held}"),
            Self::Signing(e) => write!(f, "the action could not be signed: {e}"),
        }
    }
}

/// A struct so same-typed arguments cannot be swapped at a call site unnoticed.
pub struct ClientSetup<'a> {
    pub endpoint: &'a str,
    pub wallet_key: &'a Credential,
    /// Not the signer's.
    pub account_address: &'a str,
    pub price: Scale,
    pub quantity: Scale,
    pub timeout: Duration,
    pub clock: Arc<dyn Clock>,
    pub mainnet: bool,
}

pub struct RestClient {
    endpoint: String,
    signer: Signer,
    price: Scale,
    quantity: Scale,
    http: reqwest::Client,
    /// Injected so a replay is deterministic. Every nonce derives from it.
    clock: Arc<dyn Clock>,
    /// The two environments sign over different bytes.
    mainnet: bool,
    /// Venue asset list by position. Read once because the index differs per environment.
    assets: BTreeMap<String, u64>,
    /// Not the signer's: an API wallet's query returns empty rather than an error, which
    /// reads as a flat book.
    account_address: String,
    /// Venue keeps the hundred highest per signer and refuses anything not above the smallest
    ///, so a clock-only counter would collide within one millisecond.
    highest_nonce: u64,
    /// Limits on this client's own calls. A cancel is never held.
    gate: Mutex<CallGate>,
}

impl RestClient {
    /// The wallet key comes from the broker. Only the signer is kept.
    pub fn new(setup: ClientSetup<'_>) -> Result<Self, ConnectError> {
        // One TLS provider for the whole adapter, as the market transport already installs.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let signer = Signer::from_secret(setup.wallet_key.expose())
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() })?;

        // A malformed address returns nothing from this venue, which looks like a flat account.
        let account_address = normalised_address(setup.account_address).ok_or_else(|| {
            ConnectError::NotPermitted {
                detail: format!("{} is not a 20-byte account address", setup.account_address),
            }
        })?;

        let http = reqwest::Client::builder()
            .timeout(setup.timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .use_rustls_tls()
            .build()
            .map_err(|e| ConnectError::Transport { detail: e.to_string() })?;
        let gate =
            CallGate::new(GATE).map_err(|e| ConnectError::Transport { detail: e.to_string() })?;

        Ok(Self {
            endpoint: setup.endpoint.to_string(),
            signer,
            price: setup.price,
            quantity: setup.quantity,
            http,
            clock: setup.clock,
            mainnet: setup.mainnet,
            account_address,
            assets: BTreeMap::new(),
            highest_nonce: 0,
            gate: Mutex::new(gate),
        })
    }

    /// The account every read and order is for, lowercased. Never the signer's.
    #[must_use]
    pub fn account_address(&self) -> &str {
        &self.account_address
    }

    /// Positions only: nothing compares margin or leverage, and an uncompared field drifts.
    pub async fn positions(&self) -> Result<AccountSnapshot, ConnectError> {
        let body =
            format!("{{\"type\":\"clearinghouseState\",\"user\":\"{}\"}}", self.account_address);
        let reply = self.post(CallClass::Read, PATH_INFO, &body).await.map_err(|held| {
            ConnectError::Transport { detail: format!("the account read was held back: {held}") }
        })?;
        let (status, response) = (reply.status, reply.body);
        if !(200..=299).contains(&status) {
            return Err(ConnectError::Transport {
                detail: format!("the account read answered {status}"),
            });
        }

        Ok(AccountSnapshot {
            observed_at_ns: self.clock.now_ns(),
            positions: read_positions(&response, self.price, self.quantity)?,
            margin: None,
        })
    }

    fn now_ms(&self) -> u64 {
        (self.clock.now_ns() / 1_000_000) as u64
    }

    fn next_nonce(&mut self) -> u64 {
        let candidate = self.now_ms().max(self.highest_nonce.saturating_add(1));
        self.highest_nonce = candidate;
        candidate
    }

    async fn asset(&mut self, symbol: &VenueSymbol) -> Result<u64, RequestError> {
        if let Some(index) = self.assets.get(symbol.as_str()) {
            return Ok(*index);
        }
        let reply = self
            .post(CallClass::Read, PATH_INFO, "{\"type\":\"meta\"}")
            .await
            .map_err(RequestError::Held)?;
        let (status, body) = (reply.status, reply.body);
        if status != 200 {
            return Err(RequestError::UnresolvedAsset);
        }
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
            return Err(RequestError::UnresolvedAsset);
        };
        let Some(universe) = parsed.get("universe").and_then(serde_json::Value::as_array) else {
            return Err(RequestError::UnresolvedAsset);
        };
        self.assets.clear();
        for (position, entry) in universe.iter().enumerate() {
            if let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) {
                self.assets.insert(name.to_string(), position as u64);
            }
        }
        self.assets.get(symbol.as_str()).copied().ok_or(RequestError::UnresolvedAsset)
    }

    /// A transport failure becomes status 0, read as unknown: the request may have executed.
    /// `Err` means the call was held back and nothing was sent.
    async fn post(&self, class: CallClass, path: &str, body: &str) -> Result<HttpReply, Held> {
        self.gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(class, self.clock.now_ns())?;

        let request = self
            .http
            .post(format!("{}{path}", self.endpoint))
            .header("Content-Type", "application/json")
            .body(body.to_string());

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

    async fn send_action(
        &mut self,
        class: CallClass,
        action: &Value,
    ) -> Result<VenueOutcome, RequestError> {
        let nonce = self.next_nonce();
        let connection_id = action_hash(&pack(action), nonce);
        let digest = typed_data_digest(
            &exchange_domain_separator(),
            &agent_struct_hash(&connection_id, self.mainnet),
        );
        let signed = self.signer.sign(&digest).map_err(RequestError::Signing)?;

        let body = format!(
            "{{\"action\":{},\"nonce\":{nonce},\"signature\":{{\"r\":\"0x{}\",\"s\":\"0x{}\",\"v\":{}}},\"vaultAddress\":null}}",
            to_json(action),
            hex_lower(&signed.r),
            hex_lower(&signed.s),
            signed.v
        );

        // Held back, nothing was sent: the platform's own answer, not the venue's.
        let Ok(reply) = self.post(class, PATH_EXCHANGE, &body).await else {
            return Ok(VenueOutcome::NotSent);
        };
        Ok(read_answer(reply.status, &reply.body).with_retry_after(reply.retry_after_ns))
    }

    pub async fn submit(&mut self, order: &OrderRequest) -> Result<VenueOutcome, RequestError> {
        if order.client_id.is_empty() {
            return Err(RequestError::EmptyClientId);
        }
        if order.quantity_scaled <= 0 {
            return Err(RequestError::NonPositiveQuantity);
        }
        let price = order.price_scaled.ok_or(RequestError::NoPrice)?;
        let asset = self.asset(&order.instrument).await?;

        let action = order_action(&[Placement {
            asset,
            buy: order.side == Side::Buy,
            price: wire_decimal(price, self.price),
            size: wire_decimal(order.quantity_scaled, self.quantity),
            reduce_only: order.reduce_only,
            client_id: &cloid_for(&order.client_id),
        }]);
        self.send_action(CallClass::Submit, &action).await
    }

    pub async fn cancel(
        &mut self,
        instrument: &VenueSymbol,
        client_id: &str,
    ) -> Result<VenueOutcome, RequestError> {
        if client_id.is_empty() {
            return Err(RequestError::EmptyClientId);
        }
        let asset = self.asset(instrument).await?;
        self.send_action(CallClass::Cancel, &cancel_action(asset, &cloid_for(client_id))).await
    }
}

/// The venue takes a client order id only as a 128-bit hex string (exchange endpoint, place an
/// order, read 2026-11-18). The platform's identifier is not one. The first 16 bytes of its
/// keccak-256 stand for it: the same identifier always gives the same cloid, so a cancel or a
/// resubmit after a restart names the order the submit placed.
#[must_use]
pub fn cloid_for(client_id: &str) -> String {
    let digest = crate::signing::keccak256(client_id.as_bytes());
    format!("0x{}", hex_lower(&digest[..16]))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// The execution side of the adapter contract's contract over this venue's signed path.
pub struct Execution {
    client: RestClient,
    declaration: CapabilityDeclaration,
    /// The account stream, when the endpoint has one. It prompts evidence reads and nothing else.
    account_stream: Option<crate::user_stream::UserLink>,
    /// The platform's orders by the cloid the venue knows them by.
    clients: Clients,
}

impl Execution {
    /// Declaration is handed in. Venue assurance owns what a venue is declared to do.
    pub fn new(client: RestClient, declaration: CapabilityDeclaration) -> Self {
        Self { client, declaration, account_stream: None, clients: Clients::default() }
    }

    /// Keeps the account stream for the life of this adapter.
    #[must_use]
    pub fn with_account_stream(mut self, link: crate::user_stream::UserLink) -> Self {
        self.account_stream = Some(link);
        self
    }
}

impl VenueExecution for Execution {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn pushed(&mut self) -> bijective_venue::contract::Pushed {
        self.account_stream.as_ref().map_or(
            bijective_venue::contract::Pushed::NotKept,
            crate::user_stream::UserLink::pushed,
        )
    }

    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        order: &'a OrderRequest,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move {
            // The default terms only, until this venue's mapping is read (decision 0009).
            if order.terms != bijective_venue::contract::OrderTerms::default() {
                return bijective_venue::normalise::terms_not_taken(order.terms);
            }
            self.clients.placed(&order.client_id);
            match self.client.submit(order).await {
                Ok(outcome) => outcome,
                // Held back by the client's own limits: nothing was sent, and no venue said so.
                Err(RequestError::Held(_)) => VenueOutcome::NotSent,
                // Never sent, so a rejection and not an unknown.
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
        Box::pin(async move {
            match self.client.cancel(instrument, client_id).await {
                Ok(outcome) => outcome,
                Err(RequestError::Held(_)) => VenueOutcome::NotSent,
                Err(e) => VenueOutcome::Rejected(Rejection::Unclassified {
                    venue_code: format!("not sent: {e}"),
                }),
            }
        })
    }

    fn account(&mut self, _now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move { self.client.positions().await })
    }

    /// The venue knows an order only by its cloid, so the record's orders are mapped before the
    /// first evidence read names them.
    fn adopt(&mut self, recorded: &[Recorded]) {
        self.clients.adopt(recorded);
    }

    /// Refused rows are carried as the evidence's incompleteness and reported here too, so an
    /// operator sees which field the venue changed.
    fn evidence(
        &mut self,
        since_ns: i128,
        now_ns: i128,
    ) -> Answer<'_, Result<VenueEvidence, ConnectError>> {
        Box::pin(async move {
            let catalogue: Vec<VenueSymbol> =
                self.declaration.instruments().iter().map(|i| i.symbol.clone()).collect();
            let (evidence, refused) =
                self.client.evidence(&self.clients, &catalogue, since_ns, now_ns).await?;
            for row in &refused {
                eprintln!("venue evidence row refused: {row}");
            }
            self.clients.settle(&evidence.orders);
            Ok(evidence)
        })
    }
}

/// Holdings only. Separate from the client so captured answers can be checked offline.
pub fn read_positions(
    body: &str,
    price: Scale,
    quantity: Scale,
) -> Result<Vec<PositionSnapshot>, ConnectError> {
    let parsed: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        ConnectError::Transport { detail: "the account read could not be understood".to_string() }
    })?;

    // An absent list is refused, not read as flat: reconciliation would find every position
    // missing.
    let entries =
        parsed.get("assetPositions").and_then(serde_json::Value::as_array).ok_or_else(|| {
            ConnectError::Transport {
                detail: "the account read names no positions, not even none".to_string(),
            }
        })?;

    let mut positions = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(held) = entry.get("position") else {
            continue;
        };
        let (Some(coin), Some(size)) = (
            held.get("coin").and_then(serde_json::Value::as_str),
            held.get("szi").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let quantity_scaled =
            parse_scaled(size, quantity).map_err(|_| ConnectError::Transport {
                detail: format!("{coin} reports a size this build cannot read: {size}"),
            })?;
        // No entry price is acceptable only at zero size. Otherwise the holding cannot be valued.
        let entry_price_scaled = match held.get("entryPx").and_then(serde_json::Value::as_str) {
            Some(text) => parse_scaled(text, price).map_err(|_| ConnectError::Transport {
                detail: format!("{coin} reports a price this build cannot read: {text}"),
            })?,
            None if quantity_scaled == 0 => 0,
            None => {
                return Err(ConnectError::Transport {
                    detail: format!("{coin} holds {size} at no entry price"),
                });
            }
        };
        positions.push(PositionSnapshot {
            instrument: VenueSymbol::new(coin),
            quantity_scaled,
            entry_price_scaled,
        });
    }
    Ok(positions)
}

/// Lower case: the venue may lowercase the field in transit, so mixed case signs one thing and
/// arrives as another.
fn normalised_address(given: &str) -> Option<String> {
    let body = given.strip_prefix("0x").or_else(|| given.strip_prefix("0X"))?;
    if body.len() != 40 || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("0x{}", body.to_ascii_lowercase()))
}

// Flat file: a submodule would otherwise be looked up in a directory this crate lacks.
#[cfg(test)]
#[path = "../tests/unit/rest_tests.rs"]
mod rest_tests;

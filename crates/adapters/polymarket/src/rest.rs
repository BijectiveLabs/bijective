// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The signed CLOB client: orders placed and cancelled under the venue's L2 request headers, the
//! reads that gather venue evidence, and the account read from the venue's data service.
//! Sources and the questions still open: `docs/integrations/polymarket.md`.
//!
//! The venue knows an order only by its hash. This client keeps the hash of every order it signed
//! against the platform's client identifier, and takes back the orders a previous process placed
//! from the record at start (`adopt`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bijective_common::clock::Clock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    AccountSnapshot, Answer, CapabilityDeclaration, Clearance, ConnectError, OrderRequest,
    Recorded, VenueEvidence, VenueExecution, VenueSymbol,
};
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome};
use bijective_venue::resilience::{
    BreakerConfig, CallClass, CallGate, GateConfig, Held, HttpReply, Quota, call_result_for_http,
    retry_after_from_header,
};
use reqwest::Method;

use crate::evidence::Scales;
use crate::order::{
    DEPOSIT_WALLET, EOA, Maker, OrderError, PRICE_SCALE, QUANTITY_SCALE, exchange_of_book,
    read_cancel, read_submit, sign,
};
use crate::signing::{Exchange, Signer, SigningError, checksummed, hex, request_signature};

mod adopt;
mod reads;

const PATH_ORDER: &str = "/order";
const PATH_BOOK: &str = "/book";

/// The production hosts (CLOB OpenAPI `servers`, Wallet activity). The data service has no
/// documented staging host, so an execution side is opened against production only.
const CLOB_HOST: &str = "https://clob.polymarket.com";
const DATA_HOST: &str = "https://data-api.polymarket.com";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Every answer this client reads is a few kilobytes to a few hundred. A body past this is a
/// defect or an attack, never a larger answer.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The venue publishes 500 requests per 10 seconds for its order and trade lists and 200 for the
/// data service's positions (Rate limits). 600 reads a minute stays far inside both. Submits are
/// bounded by the control engine's order rate. Five failures opening a path for 30 seconds is
/// this platform's choice.
pub const GATE: GateConfig = GateConfig {
    breaker: BreakerConfig { failures_to_open: 5, open_for_ns: 30_000_000_000 },
    reads: Quota { requests: 600, per_ns: 60_000_000_000 },
};

/// The data service host paired with a CLOB host, or `None` for a host this build does not know.
#[must_use]
pub fn data_endpoint_for(clob: &str) -> Option<&'static str> {
    (clob.trim_end_matches('/') == CLOB_HOST).then_some(DATA_HOST)
}

/// A request never built never reached the venue, so it settles the order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    EmptyClientId,
    /// A cancel for an identifier this client signed no order for, in this process.
    UnknownOrder,
    /// A cancel for an order a previous process placed whose hash the venue has not yet named.
    UnresolvedOrder,
    /// The book of the outcome could not be read, so the signing exchange is not known.
    UnresolvedBook,
    Order(OrderError),
    Signing(SigningError),
    /// The client's own limits held the call back. Nothing was sent.
    Held(Held),
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyClientId => write!(f, "an order must carry a client identifier"),
            Self::UnknownOrder => write!(f, "no order hash is held for this client identifier"),
            Self::UnresolvedOrder => {
                write!(f, "the venue has not yet named which hash this restored order holds")
            }
            Self::UnresolvedBook => write!(f, "the outcome's book names no signing exchange"),
            Self::Order(e) => write!(f, "{e}"),
            Self::Signing(e) => write!(f, "the request could not be signed: {e}"),
            Self::Held(held) => write!(f, "the call was held back: {held}"),
        }
    }
}

/// The venue's L2 credentials, created once from the wallet key (Wallets and authentication).
pub struct ApiKey<'a> {
    pub key: &'a str,
    pub secret: &'a Credential,
    pub passphrase: &'a Credential,
}

/// A struct so same-typed arguments cannot be swapped at a call site unnoticed.
pub struct ClientSetup<'a> {
    pub endpoint: &'a str,
    pub data_endpoint: &'a str,
    pub wallet_key: &'a Credential,
    /// The Deposit Wallet the key signs for, or `None` for a key that holds its own funds.
    pub deposit_wallet: Option<&'a str>,
    pub api: ApiKey<'a>,
    /// Must be the venue's own scales, 4 and 6. Any other is refused.
    pub price: Scale,
    pub quantity: Scale,
    pub timeout: Duration,
    pub clock: Arc<dyn Clock>,
}

/// One order this client signed and sent.
struct Placed {
    client_id: String,
    /// The venue reported it terminal. It is not asked about again.
    settled: bool,
}

/// No `Debug`: it holds the wallet key and the API secret.
pub struct RestClient {
    endpoint: String,
    data_endpoint: String,
    signer: Signer,
    /// The key's address: the API key belongs to it, so requests are made in its name.
    signer_address: [u8; 20],
    /// Whose orders, trades and holdings these are: the key's own, or its Deposit Wallet's.
    maker: [u8; 20],
    signature_type: u8,
    api_key: String,
    secret: String,
    passphrase: String,
    scales: Scales,
    http: reqwest::Client,
    clock: Arc<dyn Clock>,
    gate: Mutex<CallGate>,
    /// Per outcome. Read from its book once.
    exchanges: BTreeMap<String, Exchange>,
    hash_of: BTreeMap<String, String>,
    /// Orders taken back from the record, each under the hash it signs to on every exchange,
    /// until the venue's evidence names one of them.
    unresolved: BTreeMap<String, Vec<String>>,
    placed: BTreeMap<String, Placed>,
}

fn text_of(credential: &Credential, what: &str) -> Result<String, ConnectError> {
    String::from_utf8(credential.expose().to_vec())
        .map_err(|_| ConnectError::NotPermitted { detail: format!("the {what} is not text") })
}

/// The maker and its wallet kind. A Deposit Wallet at the key's own address is refused: it would
/// sign the key's orders as a contract's.
fn maker_of(
    signer: [u8; 20],
    deposit_wallet: Option<&str>,
) -> Result<([u8; 20], u8), ConnectError> {
    let Some(text) = deposit_wallet else { return Ok((signer, EOA)) };
    let refused = |detail: String| ConnectError::NotPermitted { detail };
    let digits =
        text.strip_prefix("0x").ok_or_else(|| refused(format!("{text} is not 0x-prefixed")))?;
    if digits.len() != 40 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(refused(format!("{text} is not a 20-byte address")));
    }
    let mut wallet = [0u8; 20];
    for (i, byte) in wallet.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[2 * i..2 * i + 2], 16)
            .map_err(|_| refused(format!("{text} is not hexadecimal")))?;
    }
    if wallet == signer {
        return Err(refused("the Deposit Wallet named is the key's own address".to_string()));
    }
    Ok((wallet, DEPOSIT_WALLET))
}

fn scales_of(price: Scale, quantity: Scale) -> Result<Scales, ConnectError> {
    if u32::from(price.digits()) != PRICE_SCALE || u32::from(quantity.digits()) != QUANTITY_SCALE {
        return Err(ConnectError::NotPermitted {
            detail: format!(
                "this venue prices at {PRICE_SCALE} places and sizes at {QUANTITY_SCALE}; {} and {} \
                 were configured",
                price.digits(),
                quantity.digits()
            ),
        });
    }
    Ok(Scales { price, quantity })
}

impl RestClient {
    /// Refuses credentials that cannot sign before any call is made.
    pub fn new(setup: ClientSetup<'_>) -> Result<Self, ConnectError> {
        // One TLS provider for the whole adapter.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let scales = scales_of(setup.price, setup.quantity)?;
        let signer = Signer::from_secret(setup.wallet_key.expose())
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() })?;
        let (maker, signature_type) = maker_of(signer.address(), setup.deposit_wallet)?;
        let secret = text_of(setup.api.secret, "API secret")?;
        request_signature(&secret, 0, "GET", "/", "")
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() })?;
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
            data_endpoint: setup.data_endpoint.trim_end_matches('/').to_string(),
            signer_address: signer.address(),
            maker,
            signature_type,
            signer,
            api_key: setup.api.key.to_string(),
            secret,
            passphrase: text_of(setup.api.passphrase, "API passphrase")?,
            scales,
            http,
            clock: setup.clock,
            gate: Mutex::new(gate),
            exchanges: BTreeMap::new(),
            hash_of: BTreeMap::new(),
            unresolved: BTreeMap::new(),
            placed: BTreeMap::new(),
        })
    }

    /// The wallet as the venue's list queries take it.
    fn wallet(&self) -> String {
        format!("0x{}", hex(&self.maker))
    }

    /// Sends `request` through the gate. A transport failure becomes status 0, read as unknown:
    /// the request may have executed. `Err` means it was held back and nothing was sent.
    async fn send(
        &self,
        class: CallClass,
        request: reqwest::RequestBuilder,
    ) -> Result<HttpReply, Held> {
        self.gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(class, self.clock.now_ns())?;
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

    /// A CLOB request under the five L2 headers. The signed message is the timestamp, method,
    /// path without its query, and body (API authentication).
    async fn signed(
        &self,
        class: CallClass,
        method: Method,
        path: &str,
        query: &str,
        body: &str,
    ) -> Result<HttpReply, RequestError> {
        let timestamp = u64::try_from(self.clock.now_ns().div_euclid(1_000_000_000)).unwrap_or(0);
        let signature = request_signature(&self.secret, timestamp, method.as_str(), path, body)
            .map_err(RequestError::Signing)?;
        let mut request = self
            .http
            .request(method, url(&self.endpoint, path, query))
            .header("POLY_ADDRESS", checksummed(&self.signer_address))
            .header("POLY_API_KEY", &self.api_key)
            .header("POLY_PASSPHRASE", &self.passphrase)
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp.to_string());
        if !body.is_empty() {
            request = request.header("Content-Type", "application/json").body(body.to_string());
        }
        self.send(class, request).await.map_err(RequestError::Held)
    }

    /// An unauthenticated read.
    async fn public(&self, base: &str, path: &str, query: &str) -> Result<HttpReply, Held> {
        self.send(CallClass::Read, self.http.get(url(base, path, query))).await
    }

    /// The exchange an outcome's orders are signed for: a book stating `version: "v2"` is the
    /// protocol V2 exchange, any other is a CTF book whose `neg_risk` decides (Place orders).
    async fn exchange_for(&mut self, instrument: &VenueSymbol) -> Result<Exchange, RequestError> {
        if let Some(exchange) = self.exchanges.get(instrument.as_str()) {
            return Ok(*exchange);
        }
        let query = format!("token_id={}", query_value(instrument.as_str()));
        let reply =
            self.public(&self.endpoint, PATH_BOOK, &query).await.map_err(RequestError::Held)?;
        if reply.status != 200 {
            return Err(RequestError::UnresolvedBook);
        }
        let exchange = exchange_of_book(&reply.body).ok_or(RequestError::UnresolvedBook)?;
        self.exchanges.insert(instrument.as_str().to_string(), exchange);
        Ok(exchange)
    }

    /// A client identifier signs one order, ever, in this process: a second submit of the same
    /// identifier is answered as a duplicate and never signed again, because another timestamp
    /// would make it a second order with a second hash. `intent_ns` is the instant the intent was
    /// recorded, so the hash follows from the record (decision 0002, rule 7).
    pub async fn submit(
        &mut self,
        order: &OrderRequest,
        intent_ns: i128,
    ) -> Result<VenueOutcome, RequestError> {
        if order.client_id.is_empty() {
            return Err(RequestError::EmptyClientId);
        }
        if self.hash_of.contains_key(&order.client_id)
            || self.unresolved.contains_key(&order.client_id)
        {
            return Ok(VenueOutcome::Rejected(Rejection::Duplicate));
        }
        let exchange = self.exchange_for(&order.instrument).await?;
        let maker = Maker { address: self.maker, signature_type: self.signature_type, exchange };
        let signed = sign(order, &maker, &self.signer, &self.api_key, intent_ns)
            .map_err(RequestError::Order)?;
        let hash = signed.order_id.to_ascii_lowercase();
        // Held before the call: a timeout leaves an order whose hash reconciliation must ask for.
        self.remember(&order.client_id, &hash);

        let reply = match self
            .signed(CallClass::Submit, Method::POST, PATH_ORDER, "", &signed.body)
            .await
        {
            Ok(reply) => reply,
            Err(e) => {
                self.forget(&order.client_id);
                return Err(e);
            }
        };
        let outcome = read_submit(reply.status, &reply.body).with_retry_after(reply.retry_after_ns);
        match outcome {
            VenueOutcome::Accepted if !names_order(&reply.body, &hash) => {
                Ok(VenueOutcome::Unknown(Unknown::Unreadable))
            }
            VenueOutcome::Rejected(_) => {
                self.forget(&order.client_id);
                Ok(outcome)
            }
            other => Ok(other),
        }
    }

    /// Never held: an exit must always be attempted. The body is the exact text the venue signs.
    pub async fn cancel(&mut self, client_id: &str) -> Result<VenueOutcome, RequestError> {
        if self.unresolved.contains_key(client_id) {
            return Err(RequestError::UnresolvedOrder);
        }
        let hash = self.hash_of.get(client_id).cloned().ok_or(RequestError::UnknownOrder)?;
        let body = format!("{{\"orderID\":\"{hash}\"}}");
        let reply = self.signed(CallClass::Cancel, Method::DELETE, PATH_ORDER, "", &body).await?;
        Ok(read_cancel(reply.status, &reply.body, &hash).with_retry_after(reply.retry_after_ns))
    }

    fn remember(&mut self, client_id: &str, hash: &str) {
        self.hash_of.insert(client_id.to_string(), hash.to_string());
        self.placed
            .insert(hash.to_string(), Placed { client_id: client_id.to_string(), settled: false });
    }

    /// Only for an order the venue cannot hold: never sent, or refused.
    fn forget(&mut self, client_id: &str) {
        if let Some(hash) = self.hash_of.remove(client_id) {
            self.placed.remove(&hash);
        }
    }
}

/// An accepted answer must name the hash this client signed. Another hash means the order the
/// venue holds is not the one signed here.
fn names_order(body: &str, hash: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("orderID").and_then(serde_json::Value::as_str).map(str::to_string))
        .is_some_and(|named| named.eq_ignore_ascii_case(hash))
}

fn url(base: &str, path: &str, query: &str) -> String {
    if query.is_empty() { format!("{base}{path}") } else { format!("{base}{path}?{query}") }
}

/// Percent-encodes everything outside the unreserved set: cursors are base64 and carry `=`.
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

/// The execution side of the adapter contract over this venue's signed path.
pub struct Execution {
    client: RestClient,
    declaration: CapabilityDeclaration,
}

impl Execution {
    /// The declaration is handed in. Venue assurance owns what a venue is declared to do.
    pub fn new(client: RestClient, declaration: CapabilityDeclaration) -> Self {
        Self { client, declaration }
    }
}

/// Held back by the client's own limits: nothing was sent and no venue said so. Any other error
/// was raised before a request existed, so it is a rejection and not an unknown.
fn not_sent(error: RequestError) -> VenueOutcome {
    match error {
        RequestError::Held(_) => VenueOutcome::NotSent,
        other => VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: format!("not sent: {other}"),
        }),
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
        now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move {
            // The default terms only, until this venue's mapping is read (decision 0009).
            if order.terms != bijective_model::OrderTerms::default() {
                return bijective_venue::normalise::terms_not_taken(order.terms);
            }
            self.client.submit(order, now_ns).await.unwrap_or_else(not_sent)
        })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { self.client.cancel(client_id).await.unwrap_or_else(not_sent) })
    }

    fn account(&mut self, _now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move { self.client.positions().await })
    }

    fn adopt(&mut self, recorded: &[Recorded]) {
        self.client.adopt(recorded);
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

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A platform order request turned into the venue's signed order and request body, and the
//! venue's answer turned into an outcome (Place orders, CLOB API, read 2026-11-15).
//!
//! The venue carries no client identifier: an order's identity is its hash. The salt is derived
//! from the platform's client identifier, so one request always signs one order and its hash is
//! known before the call (decision 0002, rule 7).

use bijective_model::{OrderRequest, Side};
use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};

use crate::signing::{
    Exchange, Signer, SigningError, UnsignedOrder, deposit_wallet_signature, hex, keccak256,
    order_digest, uint256,
};

/// Prices carried at scale 4 and amounts at scale 6 (`docs/integrations/polymarket.md`).
pub const PRICE_SCALE: u32 = 4;
pub const QUANTITY_SCALE: u32 = 6;

/// A salt the venue's JSON number keeps exactly: within `Number.MAX_SAFE_INTEGER` (Place orders).
const SAFE_SALT: u64 = (1 << 53) - 1;

const ZERO_WORD: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

/// A key that holds its own funds (`WalletType.EOA`).
pub const EOA: u8 = 0;
/// The smart wallet polymarket.com deploys for every account since 2026-05-04 (`WalletType`
/// `DEPOSIT_WALLET`). The key signs for it. Proxy and Safe wallets (1 and 2) are not built.
pub const DEPOSIT_WALLET: u8 = 3;

/// Who the order is for, and on which exchange. For both kinds built, the order's maker and
/// signer are this address (Place orders): the key's own, or the Deposit Wallet's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Maker {
    pub address: [u8; 20],
    pub signature_type: u8,
    pub exchange: Exchange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// Every order on this venue is a limit order. A request without a price cannot be signed.
    NoPrice,
    /// The asset id is not the decimal integer the venue issues.
    NotAnAsset,
    /// Price times quantity does not land on the venue's six-decimal amounts.
    NotRepresentable,
    /// A wallet kind this build does not sign for.
    WalletKind(u8),
    Signing(SigningError),
}

impl core::fmt::Display for OrderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPrice => write!(f, "the venue takes limit orders only"),
            Self::NotAnAsset => write!(f, "the instrument is not one of the venue's asset ids"),
            Self::NotRepresentable => {
                write!(f, "the amount is finer than the venue's six decimals")
            }
            Self::WalletKind(kind) => write!(f, "wallet kind {kind} is not signed for"),
            Self::Signing(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OrderError {}

/// One signed order, its identity, and the request body that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// The venue's `orderID`: the order hash, `0x`-prefixed.
    pub order_id: String,
    pub body: String,
}

/// The salt for `client_id`: the first bytes of its hash, kept within the safe integer range.
#[must_use]
pub fn salt_for(client_id: &str) -> u64 {
    let hash = keccak256(client_id.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash[..8]);
    u64::from_be_bytes(first) & SAFE_SALT
}

/// The unsigned order. For a buy the maker gives collateral and takes shares. For a sell the
/// reverse (Place orders). `now_ns` is the intent's instant, recorded before the call.
pub fn unsigned(
    request: &OrderRequest,
    maker: &Maker,
    now_ns: i128,
) -> Result<UnsignedOrder, OrderError> {
    let price = request.price_scaled.ok_or(OrderError::NoPrice)?;
    let token_id = uint256(request.instrument.as_str()).ok_or(OrderError::NotAnAsset)?;
    let shares =
        u128::try_from(request.quantity_scaled).map_err(|_| OrderError::NotRepresentable)?;
    let price = u128::try_from(price).map_err(|_| OrderError::NotRepresentable)?;
    let product = price.checked_mul(shares).ok_or(OrderError::NotRepresentable)?;
    let unit = 10u128.pow(PRICE_SCALE);
    if product % unit != 0 {
        return Err(OrderError::NotRepresentable);
    }
    let collateral = product / unit;
    let (maker_amount, taker_amount, side) = match request.side {
        Side::Buy => (collateral, shares, 0),
        Side::Sell => (shares, collateral, 1),
    };
    Ok(UnsignedOrder {
        salt: salt_for(&request.client_id),
        maker: maker.address,
        signer: maker.address,
        token_id,
        maker_amount,
        taker_amount,
        side,
        signature_type: maker.signature_type,
        timestamp_ms: u64::try_from(now_ns / 1_000_000).unwrap_or(0),
    })
}

/// Signs `request` and writes the body `POST /order` takes, as a good-till-cancelled order owned
/// by the API key. The builder field is zero (decision 0002, rule 8).
pub fn sign(
    request: &OrderRequest,
    maker: &Maker,
    signer: &Signer,
    api_key: &str,
    now_ns: i128,
) -> Result<Signed, OrderError> {
    let order = unsigned(request, maker, now_ns)?;
    let digest = order_digest(&order, maker.exchange);
    let signature = match maker.signature_type {
        EOA => signer.sign(&digest),
        DEPOSIT_WALLET => deposit_wallet_signature(signer, &order, maker.exchange),
        other => return Err(OrderError::WalletKind(other)),
    }
    .map_err(OrderError::Signing)?;
    let address = format!("0x{}", hex(&maker.address));
    let body = serde_json::json!({
        "order": {
            "maker": address,
            "signer": address,
            "tokenId": request.instrument.as_str(),
            "makerAmount": order.maker_amount.to_string(),
            "takerAmount": order.taker_amount.to_string(),
            "side": if order.side == 0 { "BUY" } else { "SELL" },
            "expiration": "0",
            "timestamp": order.timestamp_ms.to_string(),
            "metadata": ZERO_WORD,
            "builder": ZERO_WORD,
            "signature": signature,
            "salt": order.salt,
            "signatureType": order.signature_type,
        },
        "owner": api_key,
        "orderType": "GTC",
        "deferExec": false,
        "postOnly": false,
    })
    .to_string();
    Ok(Signed { order_id: format!("0x{}", hex(&digest)), body })
}

/// The hash `request` signs to on each exchange, in `Exchange::ALL` order, for an order a previous
/// process placed: its record fixes every signed field but the exchange, which the outcome's book
/// chose and the record does not name. Only the maker's address and wallet kind are read from
/// `maker`.
pub fn candidate_ids(
    request: &OrderRequest,
    maker: &Maker,
    intent_ns: i128,
) -> Result<[String; 3], OrderError> {
    let order = unsigned(request, maker, intent_ns)?;
    Ok(Exchange::ALL.map(|exchange| format!("0x{}", hex(&order_digest(&order, exchange)))))
}

/// The answer to `POST /order`. Success is `success: true`. Anything the venue refused carries
/// its own text, never mapped to the nearest class. Authentication precedes matching, so a 401
/// or 403 did not place the order. A 5xx or a dead transport may have.
#[must_use]
pub fn read_submit(http_status: u16, body: &str) -> VenueOutcome {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let text = |field: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(field))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    match http_status {
        200 => match parsed
            .as_ref()
            .and_then(|v| v.get("success"))
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => VenueOutcome::Accepted,
            Some(false) => VenueOutcome::Rejected(Rejection::Unclassified {
                venue_code: text("errorMsg").unwrap_or_default(),
            }),
            None => VenueOutcome::Unknown(Unknown::Unreadable),
        },
        401 | 403 => VenueOutcome::Rejected(Rejection::CredentialRefused),
        429 => VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }),
        400..=499 => VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: text("error").unwrap_or_else(|| format!("http {http_status}")),
        }),
        _ => VenueOutcome::Unknown(Unknown::ConnectionLost),
    }
}

/// The exchange a book's orders are signed for (Place orders): a book stating `version: "v2"` is
/// protocol V2. A CTF book omits the version and its `neg_risk` decides. Any other version, or no
/// `neg_risk`, names none.
#[must_use]
pub fn exchange_of_book(body: &str) -> Option<Exchange> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    match parsed.get("version").and_then(serde_json::Value::as_str) {
        Some("v2") => Some(Exchange::ProtocolV2),
        Some(_) => None,
        None => match parsed.get("neg_risk").and_then(serde_json::Value::as_bool)? {
            true => Some(Exchange::CtfNegativeRisk),
            false => Some(Exchange::Ctf),
        },
    }
}

/// The answer to `DELETE /order`: canceled when the order hash is listed under `canceled`. Hashes
/// are compared without case, since hex carries no meaning in it.
#[must_use]
pub fn read_cancel(http_status: u16, body: &str, order_id: &str) -> VenueOutcome {
    if http_status != 200 {
        return read_submit(http_status, body);
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return VenueOutcome::Unknown(Unknown::Unreadable);
    };
    let canceled =
        parsed.get("canceled").and_then(serde_json::Value::as_array).is_some_and(|ids| {
            ids.iter().any(|id| id.as_str().is_some_and(|id| id.eq_ignore_ascii_case(order_id)))
        });
    if canceled {
        return VenueOutcome::Accepted;
    }
    let reason = parsed
        .get("not_canceled")
        .and_then(serde_json::Value::as_object)
        .and_then(|m| m.iter().find(|(id, _)| id.eq_ignore_ascii_case(order_id)))
        .and_then(|(_, reason)| reason.as_str())
        .unwrap_or("not canceled")
        .to_string();
    VenueOutcome::Rejected(Rejection::Unclassified { venue_code: reason })
}

#[cfg(test)]
#[path = "../tests/unit/order_tests.rs"]
mod tests;

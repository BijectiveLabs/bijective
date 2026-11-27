// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A platform order request turned into the body `POST /v2/orders` takes, and the venue's answers
//! to a submit and a cancel read into outcomes (Orders at Alpaca, Place an order, Cancel an order,
//! read 2026-11-17, `docs/integrations/alpaca.md`).

use bijective_model::{OrderRequest, Side};
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome, render_scaled};

/// The answer to a client identifier the venue already holds: HTTP 422, code 40010001,
/// "client_order_id must be unique" (observed 2026-11-17, not in the documents read).
pub const CODE_DUPLICATE_CLIENT_ID: i64 = 40_010_001;

/// The venue's bound on a client identifier (Place an order: `client_order_id`, at most 128).
pub const MAX_CLIENT_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    EmptyClientId,
    ClientIdTooLong,
    NonPositiveQuantity,
    /// A market order that is not reduce-only: queued outside the session, it would execute at
    /// the next open on a decision made before it (ADR 0004).
    MarketEntry,
    /// The venue's price grid: two decimals at or above one dollar, four below (Orders at Alpaca,
    /// sub-penny rule). Refused here rather than by the venue.
    SubPenny {
        limit_price: String,
    },
    /// Post-only: the venue offers none for equities (decision 0009).
    TermsNotTaken(bijective_model::OrderTerms),
}

impl core::fmt::Display for OrderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TermsNotTaken(_) => write!(f, "this venue takes no post-only order"),
            Self::EmptyClientId => write!(f, "an order must carry a client identifier"),
            Self::ClientIdTooLong => {
                write!(f, "a client identifier is at most {MAX_CLIENT_ID_LEN} characters here")
            }
            Self::NonPositiveQuantity => write!(f, "an order must have a positive quantity"),
            Self::MarketEntry => {
                write!(f, "a market order is sent only to reduce a position; enter with a limit")
            }
            Self::SubPenny { limit_price } => {
                write!(f, "{limit_price} is finer than the venue's price grid")
            }
        }
    }
}

impl std::error::Error for OrderError {}

/// The limit price as the venue reads it: no trailing zeros, and refused when finer than its
/// grid. `scale` is the platform's price scale.
pub fn limit_price_text(price_scaled: i128, scale: Scale) -> Result<String, OrderError> {
    let rendered = render_scaled(price_scaled, scale);
    let text = match rendered.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() { whole.to_string() } else { format!("{whole}.{fraction}") }
        }
        None => rendered,
    };
    let decimals = text.split_once('.').map_or(0, |(_, fraction)| fraction.len());
    let one_dollar = 10_i128.pow(u32::from(scale.digits()));
    let allowed = if price_scaled >= one_dollar { 2 } else { 4 };
    if decimals > allowed {
        return Err(OrderError::SubPenny { limit_price: text });
    }
    Ok(text)
}

/// The venue's `time_in_force` for whole shares (Orders at Alpaca, read 2026-11-19: `day`, `gtc`,
/// `opg`, `cls`, `ioc`, `fok`). The default terms keep what each order type used before terms.
fn time_in_force(term: bijective_model::TimeInForce, default: &'static str) -> &'static str {
    use bijective_model::TimeInForce as T;
    match term {
        T::GoodTillCancelled => default,
        T::ImmediateOrCancel => "ioc",
        T::FillOrKill => "fok",
        T::Day => "day",
        T::AtTheOpen => "opg",
        T::AtTheClose => "cls",
    }
}

/// The body for whole shares. A limit order is good till cancelled. A market order is reduce-only
/// and lives for the day. No partner or attribution field is written (section 2.12).
pub fn order_body(
    order: &OrderRequest,
    price: Scale,
    quantity: Scale,
) -> Result<String, OrderError> {
    if order.client_id.trim().is_empty() {
        return Err(OrderError::EmptyClientId);
    }
    if order.client_id.len() > MAX_CLIENT_ID_LEN {
        return Err(OrderError::ClientIdTooLong);
    }
    if order.quantity_scaled <= 0 {
        return Err(OrderError::NonPositiveQuantity);
    }
    let side = match order.side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let mut body = serde_json::json!({
        "symbol": order.instrument.as_str(),
        "side": side,
        "qty": render_scaled(order.quantity_scaled, quantity),
        "client_order_id": order.client_id,
    });
    if order.terms.post_only {
        return Err(OrderError::TermsNotTaken(order.terms));
    }
    if let Some(stop) = order.terms.stop_price_scaled {
        return stop_body(body, order, stop, price);
    }
    match order.price_scaled {
        Some(limit) => {
            body["type"] = "limit".into();
            body["time_in_force"] = time_in_force(order.terms.time_in_force, "gtc").into();
            body["limit_price"] = limit_price_text(limit, price)?.into();
        }
        None if order.reduce_only => {
            body["type"] = "market".into();
            body["time_in_force"] = time_in_force(order.terms.time_in_force, "day").into();
        }
        None => return Err(OrderError::MarketEntry),
    }
    Ok(body.to_string())
}

/// A stop (Orders at Alpaca, read 2026-11-20): `stop_limit` with a limit price, `stop` without one,
/// good till cancelled or for the day only. A stop-market is sent only to reduce a position, as a
/// market order is: it executes at whatever the market gives once elected.
fn stop_body(
    mut body: serde_json::Value,
    order: &OrderRequest,
    stop: i128,
    price: Scale,
) -> Result<String, OrderError> {
    use bijective_model::TimeInForce as T;
    if !matches!(order.terms.time_in_force, T::GoodTillCancelled | T::Day) {
        return Err(OrderError::TermsNotTaken(order.terms));
    }
    body["time_in_force"] = time_in_force(order.terms.time_in_force, "gtc").into();
    body["stop_price"] = limit_price_text(stop, price)?.into();
    match order.price_scaled {
        Some(limit) => {
            body["type"] = "stop_limit".into();
            body["limit_price"] = limit_price_text(limit, price)?.into();
        }
        None if order.reduce_only => body["type"] = "stop".into(),
        None => return Err(OrderError::MarketEntry),
    }
    Ok(body.to_string())
}

/// The venue's own words, `code` and `message` (the shape of its documented error bodies).
fn venue_words(status: u16, body: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let field = |name: &str| {
        let value = parsed.as_ref().and_then(|v| v.get(name))?;
        Some(value.as_str().map_or_else(|| value.to_string(), str::to_string))
    };
    match (field("code"), field("message")) {
        (Some(code), Some(message)) => format!("http {status} {code}: {message}"),
        (None, Some(message)) => format!("http {status}: {message}"),
        _ => format!("http {status}"),
    }
}

/// An answer that refused the request before the venue held an order. A 403 is both the
/// documented "buying power or shares is not sufficient" and the answer to a request without a
/// key. Neither places an order, and the venue's own words travel with it.
fn refused(status: u16, body: &str) -> Option<VenueOutcome> {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(serde_json::Value::as_i64));
    match status {
        422 if code == Some(CODE_DUPLICATE_CLIENT_ID) => {
            Some(VenueOutcome::Rejected(Rejection::Duplicate))
        }
        401 => Some(VenueOutcome::Rejected(Rejection::CredentialRefused)),
        429 => Some(VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None })),
        400..=499 => Some(VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: venue_words(status, body),
        })),
        _ => None,
    }
}

/// The answer to a submit. Accepted only when the order returned carries this client
/// identifier. A server error or a dead transport may have placed it.
#[must_use]
pub fn read_submit(status: u16, body: &str, client_id: &str) -> VenueOutcome {
    if status == 200 {
        let named = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("client_order_id").and_then(|c| c.as_str()).map(str::to_string));
        return match named {
            Some(named) if named == client_id => VenueOutcome::Accepted,
            _ => VenueOutcome::Unknown(Unknown::Unreadable),
        };
    }
    refused(status, body).unwrap_or(VenueOutcome::Unknown(Unknown::ConnectionLost))
}

/// The answer to a cancel: 204 asks the venue to cancel. 422 says the order is no longer
/// cancelable, which only the venue's evidence explains (filled, cancelled or expired).
#[must_use]
pub fn read_cancel(status: u16, body: &str) -> VenueOutcome {
    match status {
        200 | 204 => VenueOutcome::Accepted,
        _ => refused(status, body).unwrap_or(VenueOutcome::Unknown(Unknown::ConnectionLost)),
    }
}

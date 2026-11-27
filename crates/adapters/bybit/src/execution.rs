// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Order request building and venue answer classification. Pure, no transport.

use bijective_venue::contract::{OrderRequest, Side};
use bijective_venue::normalise::{Rejection, Scale, Unknown, VenueOutcome, render_scaled};

/// Bybit's success code. Everything else is a refusal or something we could not read.
const RET_CODE_OK: i64 = 0;

/// Below the venue minimum order value.
const RET_CODE_BELOW_MINIMUM: i64 = 110_094;

/// "OrderLinkedID is duplicate": the client identifier is already at the venue. Error-code page,
/// read 2026-11-05.
const RET_CODE_DUPLICATE_LINK_ID: i64 = 110_072;

/// The account cannot pay for the order: "ab not enough for new order", answered to a sub-account
/// holding nothing (observed 2026-11-19). Refused before matching.
const RET_CODE_INSUFFICIENT_BALANCE: i64 = 110_007;

/// "Too many visits" (the API limit) and "Exceeded the IP Rate Limit". Same page.
pub const RET_CODES_RATE_LIMITED: [i64; 2] = [10_006, 10_018];

/// Every other code travels as itself, so a venue that changed is visible.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    EmptyClientId,
    /// An amend with neither price nor quantity would re-queue the order for nothing.
    AmendChangesNothing,
    EmptyInstrument,
    /// Refused here: a venue refusal costs a round trip and reads like a strategy rejection.
    NonPositiveQuantity,
    /// The profile lists no such instrument, so no scale to write it at (decision 0007).
    UnlistedInstrument,
    /// Terms this venue does not take (decision 0009).
    TermsNotTaken(bijective_model::OrderTerms),
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyClientId => write!(f, "an order must carry a client identifier"),
            Self::TermsNotTaken(terms) => write!(
                f,
                "this venue does not take time_in_force {}{}",
                terms.time_in_force.name(),
                if terms.post_only { " with post-only" } else { "" }
            ),
            Self::UnlistedInstrument => {
                write!(f, "the instrument is not in the venue's profile, so it has no scale")
            }
            Self::AmendChangesNothing => {
                write!(f, "an amend must change the price, the quantity, or both")
            }
            Self::EmptyInstrument => write!(f, "an order must name an instrument"),
            Self::NonPositiveQuantity => write!(f, "an order must have a positive quantity"),
        }
    }
}

impl std::error::Error for RequestError {}

/// The venue's `timeInForce` (V5 Place Order, read 2026-11-19: `GTC`, `IOC`, `FOK`, `PostOnly`,
/// "Market order will always use `IOC`"). `None` leaves the venue's default, `GTC` for a limit.
fn time_in_force(order: &OrderRequest) -> Result<Option<&'static str>, RequestError> {
    use bijective_model::TimeInForce as T;
    let terms = order.terms;
    let refused = || Err(RequestError::TermsNotTaken(terms));
    match (order.price_scaled.is_some(), terms.time_in_force, terms.post_only) {
        (true, T::GoodTillCancelled, true) => Ok(Some("PostOnly")),
        (_, T::GoodTillCancelled, false) => Ok(None),
        (_, T::ImmediateOrCancel, false) => Ok(Some("IOC")),
        (true, T::FillOrKill, false) => Ok(Some("FOK")),
        _ => refused(),
    }
}

/// Renders scaled integers exactly, no float or rounding: a rounded order is a different order.
pub fn order_body(
    order: &OrderRequest,
    price: Scale,
    quantity: Scale,
) -> Result<String, RequestError> {
    if order.client_id.trim().is_empty() {
        return Err(RequestError::EmptyClientId);
    }
    if order.instrument.as_str().trim().is_empty() {
        return Err(RequestError::EmptyInstrument);
    }
    if order.quantity_scaled <= 0 {
        return Err(RequestError::NonPositiveQuantity);
    }

    let qty = render_scaled(order.quantity_scaled, quantity);

    let mut body = serde_json::json!({
        "category": "linear",
        "symbol": order.instrument.as_str(),
        "side": match order.side { Side::Buy => "Buy", Side::Sell => "Sell" },
        "orderType": if order.price_scaled.is_some() { "Limit" } else { "Market" },
        "qty": qty,
        "orderLinkId": order.client_id,
        "reduceOnly": order.reduce_only,
    });

    if let Some(scaled) = order.price_scaled {
        body["price"] = serde_json::Value::String(render_scaled(scaled, price));
    }
    if let Some(time_in_force) = time_in_force(order)? {
        body["timeInForce"] = time_in_force.into();
    }
    // A conditional order (Place Order, read 2026-11-20): `triggerDirection` 1 triggers when the
    // price rises to `triggerPrice`, 2 when it falls to it, judged on the last traded price.
    if let Some(stop) = order.terms.stop_price_scaled {
        body["triggerPrice"] = serde_json::Value::String(render_scaled(stop, price));
        body["triggerDirection"] = match order.side {
            Side::Buy => 1,
            Side::Sell => 2,
        }
        .into();
        body["triggerBy"] = "LastPrice".into();
    }

    Ok(body.to_string())
}

/// An unrecognised code travels as itself so an operator and venue assurance can see a venue that changed.
pub fn classify(ret_code: i64, ret_msg: &str) -> VenueOutcome {
    match ret_code {
        RET_CODE_OK => VenueOutcome::Accepted,
        RET_CODE_BELOW_MINIMUM => VenueOutcome::Rejected(Rejection::BelowMinimumSize),
        RET_CODE_DUPLICATE_LINK_ID => VenueOutcome::Rejected(Rejection::Duplicate),
        RET_CODE_INSUFFICIENT_BALANCE => VenueOutcome::Rejected(Rejection::InsufficientMargin),
        code if RET_CODES_RATE_LIMITED.contains(&code) => {
            VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None })
        }
        other => VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: if ret_msg.trim().is_empty() {
                other.to_string()
            } else {
                format!("{other}: {}", ret_msg.trim())
            },
        }),
    }
}

/// A 401 has an empty body (observed) and auth is checked before matching, so the order did not happen.
pub fn read_http_response(status: u16, body: &str) -> VenueOutcome {
    match status {
        200..=299 => read_response(body),

        // Refused before the order could reach matching. Known not to have happened.
        401 => VenueOutcome::Rejected(Rejection::CredentialRefused),
        // "403, access too frequent": the IP is blocked and the venue says to stop for ten minutes
        // (rate-limit page, read 2026-11-04). Refused before matching, so known not to have happened.
        403 => {
            VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(600_000_000_000) })
        }
        429 => VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }),
        400 => match read_response(body) {
            VenueOutcome::Unknown(_) => VenueOutcome::Rejected(Rejection::Unclassified {
                venue_code: format!("http {status}"),
            }),
            classified => classified,
        },

        // Gateway, proxy or unknown status: the venue may have accepted the order.
        _ => VenueOutcome::Unknown(Unknown::Unreadable),
    }
}

/// An unparseable body is Unknown, never a rejection: the order may have filled.
pub fn read_response(body: &str) -> VenueOutcome {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return VenueOutcome::Unknown(Unknown::Unreadable);
    };
    let Some(ret_code) = parsed.get("retCode").and_then(serde_json::Value::as_i64) else {
        return VenueOutcome::Unknown(Unknown::Unreadable);
    };
    let ret_msg = parsed.get("retMsg").and_then(serde_json::Value::as_str).unwrap_or_default();
    classify(ret_code, ret_msg)
}

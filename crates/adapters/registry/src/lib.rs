// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Which venues this build can reach and how one is opened.

use std::sync::Arc;
use std::time::Duration;

use bijective_common::clock::Clock;
use bijective_secrets_broker::Credential;
use bijective_venue::contract::{
    CapabilityDeclaration, ConnectError, MarketEvent, MarketFeed, StreamRequest, VenueExecution,
};
use bijective_venue::profile::{InstrumentScales, Scales};

/// Named here and nowhere else above the adapters. The list belongs to the binary, not to
/// editable configuration.
pub const REGISTERED: &[&str] = &["bybit", "hyperliquid", "polymarket", "alpaca"];

/// Refuses an unregistered name: a default venue leaves a node connected to a venue no configuration selected.
/// Bybit reads each instrument at its own scales. The other feeds take one pair and open only when
/// every instrument shares it (decision 0007).
pub fn open_market_feed(
    venue: &str,
    scales: &InstrumentScales,
) -> Result<Box<dyn MarketFeed>, ConnectError> {
    if venue == "bybit" {
        return Ok(Box::new(bijective_venue_bybit::Feed::new(scales.clone())));
    }
    let Scales { price, quantity } = one_pair(scales)?;
    match venue {
        "hyperliquid" => Ok(Box::new(bijective_venue_hyperliquid::Feed::new(price, quantity))),
        "polymarket" => Ok(Box::new(bijective_venue_polymarket::Feed::new(price, quantity))),
        "alpaca" => Ok(Box::new(bijective_venue_alpaca::Feed::new(price, quantity))),
        other => Err(unregistered(other)),
    }
}

/// The one pair an adapter that reads a single scale is given. Mixed precision is refused.
fn one_pair(scales: &InstrumentScales) -> Result<Scales, ConnectError> {
    scales.uniform().map_err(|e| ConnectError::NotPermitted { detail: e.to_string() })
}

/// Pure over the answer, so venue assurance's runner can conclude from bytes it also digests. An unregistered
/// name is refused: a wrong-venue observation verifies the wrong declaration.
pub fn observe_instrument(
    venue: &str,
    answer: &str,
    instrument: &str,
) -> Result<bijective_venue::profile::ObservedBehaviour, ConnectError> {
    match venue {
        "bybit" => bijective_venue_bybit::observe::read_instrument(answer, instrument)
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() }),
        "hyperliquid" => bijective_venue_hyperliquid::observe::read_instrument(answer, instrument)
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() }),
        // Carried but not yet observable. Distinct from unregistered.
        "polymarket" | "alpaca" => Err(not_observable(venue)),
        other => Err(unregistered(other)),
    }
}

fn not_observable(venue: &str) -> ConnectError {
    ConnectError::NotPermitted {
        detail: format!("{venue} publishes no instrument rules this build can read"),
    }
}

/// Returns bytes, not a reading: venue assurance records the digest of what it concluded from.
pub async fn fetch_instrument_answer(
    venue: &str,
    endpoint: &str,
    instrument: &str,
) -> Result<String, ConnectError> {
    match venue {
        "bybit" => bijective_venue_bybit::observe::fetch_instrument(endpoint, instrument)
            .await
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() }),
        // One answer lists every perpetual. The instrument is picked out when it is read.
        "hyperliquid" => bijective_venue_hyperliquid::observe::fetch_instrument(endpoint)
            .await
            .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() }),
        "polymarket" | "alpaca" => Err(not_observable(venue)),
        other => Err(unregistered(other)),
    }
}

/// Exact match, no case folding: until 2026-08-30 one venue under two spellings became two
/// identities, and venue limits keyed on a string no exposure was written under.
fn unregistered(venue: &str) -> ConnectError {
    ConnectError::NotPermitted {
        detail: format!(
            "{venue} is not a registered venue in this build; it carries {}",
            REGISTERED.join(", ")
        ),
    }
}

/// A sum type because venues authenticate differently. A shared key-and-secret struct leaks one
/// venue's shape through a field.
pub enum VenueCredential {
    KeyAndSecret {
        api_key: String,
        secret: Credential,
    },
    /// The account address serves the request path and subscriptions. An API wallet's query returns
    /// empty rather than an error and fills would drop silently. Not derived: for an
    /// agent key the two addresses differ.
    WalletKey {
        key: Credential,
        account_address: String,
    },
    /// Two layers (Wallets and authentication): the wallet key signs each order and the API key,
    /// secret and passphrase sign each request. With a Deposit Wallet the key signs for it.
    /// Without one the key holds its own funds.
    WalletKeyAndApiKey {
        key: Credential,
        deposit_wallet: Option<String>,
        api_key: String,
        secret: Credential,
        passphrase: Credential,
    },
}

/// Which credential a venue signs with, so a node names its parts without naming the venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialShape {
    KeyAndSecret,
    WalletKey,
    WalletKeyAndApiKey,
}

/// Refuses an unregistered name, like every other lookup here.
pub fn credential_shape(venue: &str) -> Result<CredentialShape, ConnectError> {
    match venue {
        "alpaca" | "bybit" => Ok(CredentialShape::KeyAndSecret),
        "hyperliquid" => Ok(CredentialShape::WalletKey),
        "polymarket" => Ok(CredentialShape::WalletKeyAndApiKey),
        other => Err(unregistered(other)),
    }
}

/// The broker hands the credential over and it is passed through unread.
pub struct ExecutionSetup<'a> {
    pub endpoint: &'a str,
    pub credential: VenueCredential,
    /// From venue assurance's profile store. An adapter describing itself would be a second source of truth.
    pub declaration: CapabilityDeclaration,
    pub timeout: Duration,
    pub clock: Arc<dyn Clock>,
    pub settle_coin: &'a str,
}

/// Refuses an unregistered name, like the data side. The only place above the adapters where a
/// venue name selects code.
pub fn open_execution(
    venue: &str,
    setup: ExecutionSetup<'_>,
) -> Result<Box<dyn VenueExecution>, ConnectError> {
    // Checked before the name is matched or a credential touched: a venue curated for data has
    // no execution.
    if !setup.declaration.permits_execution() {
        return Err(ConnectError::NotPermitted {
            detail: format!("{venue} is curated for market data only"),
        });
    }
    let scales = InstrumentScales::of_catalogue(setup.declaration.instruments())
        .map_err(|e| ConnectError::NotPermitted { detail: e.to_string() })?;

    match venue {
        "polymarket" => open_polymarket(setup, one_pair(&scales)?),
        "hyperliquid" => open_hyperliquid(setup, one_pair(&scales)?),
        "bybit" => open_bybit(setup, scales),
        "alpaca" => open_alpaca(setup, one_pair(&scales)?),
        other => Err(unregistered(other)),
    }
}

/// A wallet key signs. The account stream opens where the endpoint has one.
fn open_hyperliquid(
    setup: ExecutionSetup<'_>,
    scales: Scales,
) -> Result<Box<dyn VenueExecution>, ConnectError> {
    let VenueCredential::WalletKey { key, account_address } = setup.credential else {
        // Refused, not adapted: this venue signs with a wallet key.
        return Err(ConnectError::NotPermitted {
            detail: "this venue signs with a wallet key".to_string(),
        });
    };
    // Refused, not guessed: the environments sign over different bytes.
    let Some(mainnet) = bijective_venue_hyperliquid::rest::environment_of(setup.endpoint) else {
        return Err(ConnectError::NotPermitted {
            detail: format!("{} names no environment this build can place", setup.endpoint),
        });
    };
    let client = bijective_venue_hyperliquid::rest::RestClient::new(
        bijective_venue_hyperliquid::rest::ClientSetup {
            endpoint: setup.endpoint,
            wallet_key: &key,
            account_address: &account_address,
            price: scales.price,
            quantity: scales.quantity,
            timeout: setup.timeout,
            clock: Arc::clone(&setup.clock),
            mainnet,
        },
    )?;
    use bijective_venue_hyperliquid::user_stream::{LinkSetup, UserLink, endpoint_for};

    let user = client.account_address().to_string();
    let execution = bijective_venue_hyperliquid::rest::Execution::new(client, setup.declaration);
    let Some(url) = endpoint_for(setup.endpoint) else {
        return Ok(Box::new(execution));
    };
    let link =
        UserLink::start(LinkSetup { url: url.to_string(), user, clock: Arc::clone(&setup.clock) })
            .map_err(|e| ConnectError::Transport {
                detail: format!("the account stream's thread was refused: {e}"),
            })?;
    Ok(Box::new(execution.with_account_stream(link)))
}

/// A key and secret sign. The private stream opens where the endpoint has one.
fn open_bybit(
    setup: ExecutionSetup<'_>,
    scales: InstrumentScales,
) -> Result<Box<dyn VenueExecution>, ConnectError> {
    let VenueCredential::KeyAndSecret { api_key, secret } = setup.credential else {
        // Refused, not adapted: an empty key would authenticate as no account and report the
        // venue's refusal as its own.
        return Err(ConnectError::NotPermitted {
            detail: "this venue signs with a key and a secret".to_string(),
        });
    };
    use bijective_venue_bybit::private_stream::{LinkSetup, PrivateLink, endpoint_for};

    let link = match endpoint_for(setup.endpoint) {
        Some(url) => Some(
            PrivateLink::start(LinkSetup {
                url: url.to_string(),
                api_key: api_key.clone(),
                secret: Credential::new(secret.expose().to_vec()),
                scales: scales.clone(),
                clock: Arc::clone(&setup.clock),
            })
            .map_err(|e| ConnectError::Transport {
                detail: format!("the private stream's thread was refused: {e}"),
            })?,
        ),
        None => None,
    };
    let client = bijective_venue_bybit::rest::RestClient::new(
        setup.endpoint,
        api_key,
        secret,
        scales,
        setup.timeout,
        setup.clock,
    )?;
    let execution =
        bijective_venue_bybit::rest::Execution::new(client, setup.declaration, setup.settle_coin);
    Ok(Box::new(match link {
        Some(link) => execution.with_private_stream(link),
        None => execution,
    }))
}

/// A key id and a secret key travel as headers on every request.
fn open_alpaca(
    setup: ExecutionSetup<'_>,
    scales: Scales,
) -> Result<Box<dyn VenueExecution>, ConnectError> {
    use bijective_venue_alpaca::rest::{ClientSetup, Execution, RestClient};

    let VenueCredential::KeyAndSecret { api_key, secret } = setup.credential else {
        return Err(ConnectError::NotPermitted {
            detail: "this venue authenticates with a key id and a secret key".to_string(),
        });
    };
    use bijective_venue_alpaca::trade_stream::{LinkSetup, TradeLink, endpoint_for};

    let client = RestClient::new(ClientSetup {
        endpoint: setup.endpoint,
        key_id: &api_key,
        secret: &secret,
        price: scales.price,
        quantity: scales.quantity,
        timeout: setup.timeout,
        clock: Arc::clone(&setup.clock),
    })?;
    let execution = Execution::new(client, setup.declaration);
    let Some(url) = endpoint_for(setup.endpoint) else {
        return Ok(Box::new(execution));
    };
    let secret = String::from_utf8(secret.expose().to_vec()).map_err(|_| {
        ConnectError::NotPermitted { detail: "the secret key is not text".to_string() }
    })?;
    let link = TradeLink::start(LinkSetup {
        url: url.to_string(),
        key_id: api_key,
        secret,
        clock: setup.clock,
    })
    .map_err(|e| ConnectError::Transport {
        detail: format!("the trade stream's thread was refused: {e}"),
    })?;
    Ok(Box::new(execution.with_trade_stream(link)))
}

fn open_polymarket(
    setup: ExecutionSetup<'_>,
    scales: Scales,
) -> Result<Box<dyn VenueExecution>, ConnectError> {
    use bijective_venue_polymarket::rest::{
        ApiKey, ClientSetup, Execution, RestClient, data_endpoint_for,
    };

    let VenueCredential::WalletKeyAndApiKey { key, deposit_wallet, api_key, secret, passphrase } =
        setup.credential
    else {
        return Err(ConnectError::NotPermitted {
            detail: "this venue signs with a wallet key and an API key".to_string(),
        });
    };
    // Refused, not guessed: positions are read from a second host paired with this one.
    let Some(data_endpoint) = data_endpoint_for(setup.endpoint) else {
        return Err(ConnectError::NotPermitted {
            detail: format!("{} is not a host this build can place orders on", setup.endpoint),
        });
    };
    let client = RestClient::new(ClientSetup {
        endpoint: setup.endpoint,
        data_endpoint,
        wallet_key: &key,
        deposit_wallet: deposit_wallet.as_deref(),
        api: ApiKey { key: &api_key, secret: &secret, passphrase: &passphrase },
        price: scales.price,
        quantity: scales.quantity,
        timeout: setup.timeout,
        clock: setup.clock,
    })?;
    Ok(Box::new(Execution::new(client, setup.declaration)))
}

/// Whether a venue's market stream needs an account's credential: most venues publish market
/// data to anyone, and a credential is never read for one that does not need it.
pub fn market_stream_credential(venue: &str) -> Result<Option<CredentialShape>, ConnectError> {
    match venue {
        "bybit" | "hyperliquid" | "polymarket" => Ok(None),
        "alpaca" => Ok(Some(CredentialShape::KeyAndSecret)),
        other => Err(unregistered(other)),
    }
}

/// The venue's transport, parser and book maintenance run behind this call. `MarketEvent` carries
/// no trace of the venue. `credential` is the account's, for a venue whose stream needs one.
pub async fn run_market_stream(
    venue: &str,
    request: &StreamRequest,
    credential: Option<VenueCredential>,
    out: tokio::sync::mpsc::Sender<MarketEvent>,
) -> Result<(), ConnectError> {
    let ended = match venue {
        "bybit" => bijective_venue_bybit::run_market_stream(request, out).await,
        "hyperliquid" => bijective_venue_hyperliquid::run_market_stream(request, out).await,
        "polymarket" => bijective_venue_polymarket::run_market_stream(request, out).await,
        "alpaca" => {
            let Some(VenueCredential::KeyAndSecret { api_key, secret }) = credential else {
                return Err(ConnectError::NotPermitted {
                    detail: "this venue's market stream needs a key id and a secret key".into(),
                });
            };
            bijective_venue_alpaca::run_market_stream(request, &api_key, &secret, out).await
        }
        other => return Err(unregistered(other)),
    };
    ended.map_err(|detail| ConnectError::Transport { detail })
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

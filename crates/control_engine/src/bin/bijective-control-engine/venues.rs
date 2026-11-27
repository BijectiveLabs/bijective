// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Opening the execution side of every configured venue through the secrets broker.

use std::time::Duration;

use bijective_common::clock::WallClock;
use bijective_control_engine::config::Assets;
use bijective_control_engine::{Accepted, RunPlan};
use bijective_secrets_broker::keystore::Keystore;
use bijective_secrets_broker::{Credential, DeclaredSource, InitCredentialSource, KeySource};
use bijective_venue::contract::{CapabilityDeclaration, VenueExecution};
use bijective_venue::profile::VenueProfile;
use bijective_venue_registry::{CredentialShape, VenueCredential};

use crate::paper::Paper;

/// The venues this node can reach, owned for the life of the process.
pub(crate) type OpenVenues = Vec<(String, Box<dyn VenueExecution>)>;

/// A configured venue that did not open, and why. One venue failing does not stop the others.
pub(crate) struct Withdrawn {
    pub(crate) venue: String,
    pub(crate) reason: String,
}

/// Opens the venues this run plan allows. A backtest and a paper run open a simulated venue per
/// configured venue, from its profile, and read no credential (decision 0015). Live opens them as
/// configured. Fees are read from `market_store`. A node-level failure is an `Err`.
pub(crate) fn open_for(
    plan: RunPlan,
    market_store: &std::path::Path,
    accepted: &Accepted,
    keystore: Option<&std::path::Path>,
    credential: Option<&str>,
) -> Result<(OpenVenues, Vec<Withdrawn>, Paper), String> {
    if !plan.reaches_venues() {
        let (paper, opened, withdrawn) = Paper::open(accepted, market_store);
        return Ok((opened, withdrawn, paper));
    }
    let (opened, withdrawn) = open_venues(accepted, keystore, credential)?;
    Ok((opened, withdrawn, Paper::default()))
}

/// The profile at `path`, each base named by its asset (`docs/decisions/0006-asset-names.md`).
/// Every place this node reads a profile reads it here, so no consumer sees another name.
pub(crate) fn read_profile(path: &str, assets: &Assets) -> Result<VenueProfile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| format!("its profile at {path} could not be read"))?;
    let profile =
        VenueProfile::from_json(&text).map_err(|e| format!("its profile was refused: {e}"))?;
    Ok(assets.applied(profile))
}

/// Opens the execution side for every venue the configuration declares.
fn open_venues(
    accepted: &Accepted,
    keystore: Option<&std::path::Path>,
    credential: Option<&str>,
) -> Result<(OpenVenues, Vec<Withdrawn>), String> {
    if accepted.access().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let store = open_keystore(keystore, credential)?;
    let mut opened: OpenVenues = Vec::new();
    let mut withdrawn: Vec<Withdrawn> = Vec::new();
    for access in accepted.access() {
        match open_venue(access, &store, accepted.assets()) {
            Ok(adapter) => opened.push((access.name.clone(), adapter)),
            Err(reason) => withdrawn.push(Withdrawn { venue: access.name.clone(), reason }),
        }
    }
    Ok((opened, withdrawn))
}

/// The keystore is opened with the init credential. A credential never comes from configuration.
fn open_keystore(
    keystore: Option<&std::path::Path>,
    credential: Option<&str>,
) -> Result<Keystore, String> {
    let (Some(keystore), Some(credential)) = (keystore, credential) else {
        return Err("--keystore and --credential are required once a venue is configured; a \
                    credential cannot come from the configuration file"
            .to_string());
    };
    let key = InitCredentialSource::declared(credential)
        .unwrap_key()
        .map_err(|e| format!("the keystore key could not be resolved: {e}"))?;
    let sealed = std::fs::read(keystore)
        .map_err(|e| format!("the keystore at {} could not be read: {e}", keystore.display()))?;
    Keystore::open(&sealed, &key, DeclaredSource::InitCredential)
        .map_err(|e| format!("the keystore could not be opened: {e:?}"))
}

/// Opens one venue. The `Err` is the reason it is withdrawn.
fn open_venue(
    access: &bijective_control_engine::config::VenueAccess,
    store: &Keystore,
    assets: &Assets,
) -> Result<Box<dyn VenueExecution>, String> {
    let profile = read_profile(&access.profile, assets)?;
    if profile.venue != access.dialect() {
        return Err(format!(
            "its profile is for {}, and the account is at {} (decision 0008)",
            profile.venue,
            access.dialect()
        ));
    }
    let credential = credential(access, store)?;
    let timeout = request_timeout(&access.request_timeout_ns)
        .map_err(|e| format!("its request timeout is unusable: {e}"))?;

    bijective_venue_registry::open_execution(
        access.dialect(),
        bijective_venue_registry::ExecutionSetup {
            endpoint: &access.endpoint,
            credential,
            declaration: CapabilityDeclaration::from_profile(&profile),
            timeout,
            clock: std::sync::Arc::new(WallClock::new()),
            settle_coin: &access.settle_coin,
        },
    )
    .map_err(|e| format!("it could not be opened: {e:?}"))
}

/// The credential in the shape the venue signs with, read from the keystore by the names the
/// configuration gives. Configuration assurance has checked that the shape's names are there.
fn credential(
    access: &bijective_control_engine::config::VenueAccess,
    store: &Keystore,
) -> Result<VenueCredential, String> {
    let secret = |name: &str| {
        store
            .get(name)
            .map(|c| Credential::new(c.expose().to_vec()))
            .ok_or_else(|| format!("the keystore holds no {name}"))
    };
    let text = |name: &str| {
        String::from_utf8(secret(name)?.expose().to_vec())
            .map_err(|_| format!("{name} is not text"))
    };
    let shape = bijective_venue_registry::credential_shape(access.dialect())
        .map_err(|e| format!("it names no credential shape: {e}"))?;
    Ok(match shape {
        CredentialShape::KeyAndSecret => VenueCredential::KeyAndSecret {
            api_key: text(&access.key_name)?,
            secret: secret(&access.secret_name)?,
        },
        CredentialShape::WalletKey => VenueCredential::WalletKey {
            key: wallet_key(secret(&access.wallet_key_name)?),
            account_address: access.account_address.clone(),
        },
        CredentialShape::WalletKeyAndApiKey => VenueCredential::WalletKeyAndApiKey {
            key: wallet_key(secret(&access.wallet_key_name)?),
            deposit_wallet: Some(access.account_address.trim().to_string())
                .filter(|address| !address.is_empty()),
            api_key: text(&access.key_name)?,
            secret: secret(&access.secret_name)?,
            passphrase: secret(&access.passphrase_name)?,
        },
    })
}

/// A wallet key is kept as its 32 bytes or as the hex wallets export, with or without `0x`.
/// Anything else is passed on unchanged for the adapter's signer to refuse.
fn wallet_key(stored: Credential) -> Credential {
    let text = String::from_utf8_lossy(stored.expose()).trim().to_string();
    let digits = text.strip_prefix("0x").unwrap_or(&text);
    if digits.len() != 64 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return stored;
    }
    let bytes = (0..32).filter_map(|i| u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).ok());
    Credential::new(bytes.collect())
}

/// Nanoseconds written as text in the configuration, converted to a request timeout.
fn request_timeout(nanoseconds: &str) -> Result<Duration, String> {
    let ns = nanoseconds.parse::<i128>().map_err(|e| e.to_string())?;
    let ns = u64::try_from(ns).map_err(|_| "it does not fit".to_string())?;
    Ok(Duration::from_nanos(ns))
}

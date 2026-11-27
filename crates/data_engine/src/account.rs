// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The account credential a venue's market stream needs, read from the broker's keystore. A venue
//! that publishes market data to anyone is never given one, and one that needs it is refused
//! without it: there is no other place a credential could come from.

use bijective_secrets_broker::keystore::Keystore;
use bijective_secrets_broker::{Credential, DeclaredSource, InitCredentialSource, KeySource};
use bijective_venue_registry::{CredentialShape, VenueCredential, market_stream_credential};

use crate::args::Account;

/// `Err` is the reason the stream cannot start, for the operator.
pub(crate) fn credential_for(
    venue: &str,
    account: Option<&Account>,
) -> Result<Option<VenueCredential>, String> {
    let shape = market_stream_credential(venue).map_err(|e| e.to_string())?;
    match (shape, account) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(format!(
            "{venue} publishes market data without an account; its stream is given no credential"
        )),
        (Some(_), None) => Err(format!(
            "{venue}'s market stream needs an account: --keystore, --credential, --key-name and \
             --secret-name name where it is read"
        )),
        (Some(CredentialShape::KeyAndSecret), Some(account)) => {
            let store = open(account)?;
            let part = |name: &str| {
                store
                    .get(name)
                    .map(|c| Credential::new(c.expose().to_vec()))
                    .ok_or_else(|| format!("the keystore holds no {name}"))
            };
            let api_key = String::from_utf8(part(&account.key_name)?.expose().to_vec())
                .map_err(|_| format!("{} is not text", account.key_name))?;
            Ok(Some(VenueCredential::KeyAndSecret { api_key, secret: part(&account.secret_name)? }))
        }
        (Some(other), Some(_)) => {
            Err(format!("{venue}'s market stream needs a {other:?} credential, not built here"))
        }
    }
}

/// Opened with the init credential, as the control engine opens it.
fn open(account: &Account) -> Result<Keystore, String> {
    let key = InitCredentialSource::declared(account.credential.as_str())
        .unwrap_key()
        .map_err(|e| format!("the keystore key could not be resolved: {e}"))?;
    let sealed = std::fs::read(&account.keystore).map_err(|e| {
        format!("the keystore at {} could not be read: {e}", account.keystore.display())
    })?;
    Keystore::open(&sealed, &key, DeclaredSource::InitCredential)
        .map_err(|e| format!("the keystore could not be opened: {e:?}"))
}

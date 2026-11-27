// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! HTTPS for the console, with a client certificate required of every connection.
//!
//! A console reachable beyond loopback answers only browsers holding a certificate the operator's
//! own authority issued. The certificates and the authority are the operator's files. The console
//! ships none and generates none.

use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::crypto::ring::default_provider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;

/// The three files HTTPS needs, all PEM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    pub certificate: std::path::PathBuf,
    pub key: std::path::PathBuf,
    /// The authority whose certificates may connect.
    pub client_authority: std::path::PathBuf,
}

fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let found = CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if found.is_empty() {
        return Err(format!("{} holds no certificate", path.display()));
    }
    Ok(found)
}

/// Every refusal names the file it is about, so a misconfiguration is fixed without a debugger.
pub fn server_config(files: &TlsFiles) -> Result<Arc<ServerConfig>, String> {
    let provider = Arc::new(default_provider());
    let mut authorities = rustls::RootCertStore::empty();
    for certificate in certificates(&files.client_authority)? {
        authorities
            .add(certificate)
            .map_err(|e| format!("{}: {e}", files.client_authority.display()))?;
    }
    let verifier =
        WebPkiClientVerifier::builder_with_provider(Arc::new(authorities), provider.clone())
            .build()
            .map_err(|e| format!("{}: {e}", files.client_authority.display()))?;
    let key = PrivateKeyDer::from_pem_file(&files.key)
        .map_err(|e| format!("{}: {e}", files.key.display()))?;
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates(&files.certificate)?, key)
        .map_err(|e| format!("{}: {e}", files.certificate.display()))?;
    Ok(Arc::new(config))
}

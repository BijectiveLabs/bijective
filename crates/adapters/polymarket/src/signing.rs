// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The venue's order signature (EIP-712) and request signature (HMAC), from `docs/integrations/polymarket.md`
//! (Place orders, API authentication, read 2026-11-15). A signature that recovers locally proves
//! the encoding is self-consistent, not that the venue accepts it: only an accepted order does.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use hmac::{Hmac, KeyInit, Mac};
use k256::ecdsa::{SigningKey, VerifyingKey};
use sha2::Sha256;
use sha3::{Digest, Keccak256};

const DOMAIN_NAME: &str = "Polymarket CTF Exchange";
const CHAIN_ID: u64 = 137;
const ORDER_TYPE: &str = "Order(uint256 salt,address maker,address signer,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,uint256 timestamp,bytes32 metadata,bytes32 builder)";
const DOMAIN_TYPE: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

/// Which exchange signs the order, chosen by the book (Place orders): a `v2` book, or a CTF book
/// with or without negative risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exchange {
    ProtocolV2,
    Ctf,
    CtfNegativeRisk,
}

const PROTOCOL_V2: [u8; 20] = address("0xe3333700cA9d93003F00f0F71f8515005F6c00Aa");
const CTF: [u8; 20] = address("0xE111180000d2663C0091e4f400237545B87B996B");
const CTF_NEGATIVE_RISK: [u8; 20] = address("0xe2222d279d744050d28e00520010520000310F59");

impl Exchange {
    pub const ALL: [Exchange; 3] = [Exchange::ProtocolV2, Exchange::Ctf, Exchange::CtfNegativeRisk];

    const fn domain(self) -> (&'static str, [u8; 20]) {
        match self {
            Exchange::ProtocolV2 => ("3", PROTOCOL_V2),
            Exchange::Ctf => ("2", CTF),
            Exchange::CtfNegativeRisk => ("2", CTF_NEGATIVE_RISK),
        }
    }
}

/// Evaluated when the crate is built: a mistyped contract address fails the build.
const fn address(text: &str) -> [u8; 20] {
    let bytes = text.as_bytes();
    assert!(bytes.len() == 42 && bytes[0] == b'0' && bytes[1] == b'x');
    let mut out = [0u8; 20];
    let mut i = 0;
    while i < 20 {
        out[i] = nibble(bytes[2 + 2 * i]) * 16 + nibble(bytes[3 + 2 * i]);
        i += 1;
    }
    out
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("an address is hexadecimal"),
    }
}

/// The fields the venue signs, in its order. Amounts are fixed-point with six decimals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedOrder {
    pub salt: u64,
    pub maker: [u8; 20],
    pub signer: [u8; 20],
    /// The outcome's asset id, a decimal integer of up to 256 bits.
    pub token_id: [u8; 32],
    pub maker_amount: u128,
    pub taker_amount: u128,
    /// `0` buys, `1` sells.
    pub side: u8,
    /// `0` for a key that holds its own funds (EOA).
    pub signature_type: u8,
    pub timestamp_ms: u64,
}

pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn word(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let take = bytes.len().min(32);
    out[32 - take..].copy_from_slice(&bytes[bytes.len() - take..]);
    out
}

/// A decimal integer of up to 256 bits, big-endian. `None` for anything else.
#[must_use]
pub fn uint256(decimal: &str) -> Option<[u8; 32]> {
    if decimal.is_empty() || !decimal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for digit in decimal.bytes().map(|b| u16::from(b - b'0')) {
        let mut carry = digit;
        for byte in out.iter_mut().rev() {
            let next = u16::from(*byte) * 10 + carry;
            *byte = (next & 0xff) as u8;
            carry = next >> 8;
        }
        if carry != 0 {
            return None;
        }
    }
    Some(out)
}

fn domain_separator(exchange: Exchange) -> [u8; 32] {
    let (version, contract) = exchange.domain();
    let mut buffer = Vec::with_capacity(160);
    buffer.extend_from_slice(&keccak256(DOMAIN_TYPE.as_bytes()));
    buffer.extend_from_slice(&keccak256(DOMAIN_NAME.as_bytes()));
    buffer.extend_from_slice(&keccak256(version.as_bytes()));
    buffer.extend_from_slice(&word(&CHAIN_ID.to_be_bytes()));
    buffer.extend_from_slice(&word(&contract));
    keccak256(&buffer)
}

/// Metadata and builder are signed as zero words: the builder field is attribution, which the
/// platform never sets (decision 0002, rule 8).
fn struct_hash(order: &UnsignedOrder) -> [u8; 32] {
    let mut buffer = Vec::with_capacity(32 * 12);
    buffer.extend_from_slice(&keccak256(ORDER_TYPE.as_bytes()));
    buffer.extend_from_slice(&word(&order.salt.to_be_bytes()));
    buffer.extend_from_slice(&word(&order.maker));
    buffer.extend_from_slice(&word(&order.signer));
    buffer.extend_from_slice(&order.token_id);
    buffer.extend_from_slice(&word(&order.maker_amount.to_be_bytes()));
    buffer.extend_from_slice(&word(&order.taker_amount.to_be_bytes()));
    buffer.extend_from_slice(&word(&[order.side]));
    buffer.extend_from_slice(&word(&[order.signature_type]));
    buffer.extend_from_slice(&word(&order.timestamp_ms.to_be_bytes()));
    buffer.extend_from_slice(&[0u8; 32]);
    buffer.extend_from_slice(&[0u8; 32]);
    keccak256(&buffer)
}

/// The digest signed, which is also the venue's order identity (`orderID`, the order hash).
#[must_use]
pub fn order_digest(order: &UnsignedOrder, exchange: Exchange) -> [u8; 32] {
    let mut buffer = Vec::with_capacity(66);
    buffer.extend_from_slice(&[0x19, 0x01]);
    buffer.extend_from_slice(&domain_separator(exchange));
    buffer.extend_from_slice(&struct_hash(order));
    keccak256(&buffer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningError {
    KeyRefused,
    /// The API secret is not the base64 the venue issues.
    SecretRefused,
}

impl core::fmt::Display for SigningError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::KeyRefused => write!(f, "the wallet key is not valid for this curve"),
            Self::SecretRefused => write!(f, "the API secret is not base64"),
        }
    }
}

impl std::error::Error for SigningError {}

/// No `Debug`, and the key is never returned: a printable signer ends up in a log line.
pub struct Signer {
    key: SigningKey,
}

impl Signer {
    pub fn from_secret(bytes: &[u8]) -> Result<Self, SigningError> {
        SigningKey::from_slice(bytes).map(|key| Self { key }).map_err(|_| SigningError::KeyRefused)
    }

    /// The address the key controls: the last twenty bytes of the public key's hash.
    #[must_use]
    pub fn address(&self) -> [u8; 20] {
        let point = VerifyingKey::from(&self.key).to_sec1_point(false);
        let hash = keccak256(&point.as_bytes()[1..]);
        let mut out = [0u8; 20];
        out.copy_from_slice(&hash[12..]);
        out
    }

    /// `r || s || v`, with `v` as 27 or 28, as `0x`-prefixed hex. The nonce is deterministic from
    /// key and digest, so one order signs the same way every time.
    pub fn sign(&self, digest: &[u8; 32]) -> Result<String, SigningError> {
        Ok(format!("0x{}", hex(&self.signature_bytes(digest)?)))
    }

    fn signature_bytes(&self, digest: &[u8; 32]) -> Result<Vec<u8>, SigningError> {
        let (signature, recovery) = self.key.sign_prehash_recoverable(digest);
        let mut bytes = signature.to_bytes().to_vec();
        bytes.push(27 + recovery.to_byte());
        Ok(bytes)
    }
}

const TYPED_DATA_SIGN_TYPE: &str = "TypedDataSign(Order contents,string name,string version,uint256 chainId,address verifyingContract,bytes32 salt)";
const DEPOSIT_WALLET_NAME: &str = "DepositWallet";
const DEPOSIT_WALLET_VERSION: &str = "1";

/// A Deposit Wallet's order signature (Place orders, read 2026-11-16): the key signs the order
/// nested in `TypedDataSign` under the exchange's domain, with the wallet as verifying contract,
/// and the signature is wrapped for ERC-7739 validation as `inner || exchange domain separator ||
/// order struct hash || order type || the type's length as two bytes`. The order's identity is
/// unchanged: the exchange's digest of the order itself.
pub fn deposit_wallet_signature(
    signer: &Signer,
    order: &UnsignedOrder,
    exchange: Exchange,
) -> Result<String, SigningError> {
    let separator = domain_separator(exchange);
    let contents = struct_hash(order);
    let mut buffer = Vec::with_capacity(32 * 7);
    buffer.extend_from_slice(&keccak256(format!("{TYPED_DATA_SIGN_TYPE}{ORDER_TYPE}").as_bytes()));
    buffer.extend_from_slice(&contents);
    buffer.extend_from_slice(&keccak256(DEPOSIT_WALLET_NAME.as_bytes()));
    buffer.extend_from_slice(&keccak256(DEPOSIT_WALLET_VERSION.as_bytes()));
    buffer.extend_from_slice(&word(&CHAIN_ID.to_be_bytes()));
    buffer.extend_from_slice(&word(&order.maker));
    buffer.extend_from_slice(&[0u8; 32]);
    let mut envelope = Vec::with_capacity(66);
    envelope.extend_from_slice(&[0x19, 0x01]);
    envelope.extend_from_slice(&separator);
    envelope.extend_from_slice(&keccak256(&buffer));

    let mut wrapped = signer.signature_bytes(&keccak256(&envelope))?;
    wrapped.extend_from_slice(&separator);
    wrapped.extend_from_slice(&contents);
    wrapped.extend_from_slice(ORDER_TYPE.as_bytes());
    wrapped.extend_from_slice(&(ORDER_TYPE.len() as u16).to_be_bytes());
    Ok(format!("0x{}", hex(&wrapped)))
}

/// `urlsafeBase64WithPadding(HMAC-SHA256(base64Decode(secret), timestamp + METHOD + path + body))`
/// (API authentication).
pub fn request_signature(
    secret: &str,
    timestamp_s: u64,
    method: &str,
    path: &str,
    body: &str,
) -> Result<String, SigningError> {
    let key = URL_SAFE.decode(secret).map_err(|_| SigningError::SecretRefused)?;
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(&key).map_err(|_| SigningError::SecretRefused)?;
    mac.update(format!("{timestamp_s}{method}{path}{body}").as_bytes());
    Ok(URL_SAFE.encode(mac.finalize().into_bytes()))
}

/// The address in the mixed-case checksum form of EIP-55.
#[must_use]
pub fn checksummed(address: &[u8; 20]) -> String {
    let lower = hex(address);
    let hash = keccak256(lower.as_bytes());
    let mut out = String::from("0x");
    for (i, c) in lower.chars().enumerate() {
        let nibble = if i % 2 == 0 { hash[i / 2] >> 4 } else { hash[i / 2] & 0x0f };
        out.push(if nibble >= 8 { c.to_ascii_uppercase() } else { c });
    }
    out
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "../tests/unit/signing_tests.rs"]
mod tests;

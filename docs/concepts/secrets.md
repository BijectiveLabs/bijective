# Secrets

The secrets broker is the only path from a component to a credential. No component resolves a
credential from configuration, the environment or source; configuration names a credential entry
and the broker resolves it. The `Credential resolution` gate enforces this.

## Properties

- **A credential cannot be printed.** `Credential` and `KeyMaterial` render as `redacted`. The
  `Debug` implementation exists and redacts, so a struct holding a credential can still derive
  `Debug` safely.
- **The accessor is named `expose`.** Every use is one search away.
- **Credentials are zeroed on drop.**
- **There is no fallback between key sources.** A keystore declares how its key is obtained, and
  asking for a source the host cannot serve is an error, never a substitution.

Key sources:

| Source | Description |
|---|---|
| `init-credential` | The init system's encrypted credentials (systemd 250 or newer). Used by the core programs |
| `tpm-seal` | A key sealed to the host's TPM |
| `passphrase` | A passphrase entered by an operator |

## The keystore

A keystore is one file: a header (magic, format version, declared source, salt, nonce) and a body
encrypted with ChaCha20-Poly1305. The passphrase source derives its key with Argon2id, with
parameters that configuration cannot lower. The TPM source is declared but not implemented on this
build; asking for it is refused.

- A fresh nonce is generated for every seal.
- A wrong key and a tampered file produce the same error, so an attacker cannot tell which they hit.
- Opening a keystore under a different source than it was sealed with is refused.
- Rewrites are atomic: write, sync, rename. The safe failure is "unchanged".
- The file mode is `0600`, asserted by a test.

The broker is the only crate built on third-party cryptography. Every other core crate runs on the
standard library alone; every third-party package is attested before it enters a manifest.

## Operating the keystore

`bijective-secrets` is the operator's tool for the keystore. A secret is read from standard input,
never from an argument, which would be visible in the process list and kept in shell history. It
seals under a passphrase, or with `--init-credential <file>` under the init credential the core
programs load, which is the only keystore they open. See
[Load a venue credential](../how_to/load_a_venue_credential.md).

# Security

## Reporting a vulnerability

Report vulnerabilities privately through the repository's GitHub Security Advisories
("Report a vulnerability" on the Security tab). Do not open a public issue.

Include what you found, how to reproduce it and which version or commit it affects. Do not include
credentials, account data or live order details.

## Scope

In scope: the source in this repository, including the order path, the risk gate, the secrets
broker, the strategy sandbox, the adapters and the deployment kit.

Out of scope: vulnerabilities in a third-party dependency that are not reachable through this
platform, and findings that require an operator to have already disclosed their own credentials.

## Security model

- Credentials are resolved only through the secrets broker and stored in an encrypted keystore with
  mode `0600`. See [Secrets](docs/concepts/secrets.md).
- Strategies run sandboxed with no network, no credentials and bounded memory and time. See
  [Strategies](docs/concepts/strategies.md).
- The console is the only program that listens on the network. It binds loopback unless it is
  given a server certificate, its key and a client authority (`--tls-cert`, `--tls-key`,
  `--client-ca`). An address other than loopback is refused without all three, and over HTTPS a
  browser without a client certificate that authority issued is refused in the handshake. See
  [Reach the console from another machine](docs/how_to/reach_the_console.md).
- Operator commands are signed with the operator's own key and verified against the operator
  registry and the role on arrival.
- No outbound telemetry and no external assets.
- No component above the adapter contract opens an outbound network connection. The
  `Security checks` gate enforces this, together with a scan for secret literals.

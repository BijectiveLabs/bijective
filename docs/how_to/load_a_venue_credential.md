# Load a venue credential

Platform-Version: 0.1.1
Drill: scripts/verify/keystore_drill.sh
Applies-To: before the platform trades on a venue for the first time, and on every key rotation

## Why there is no file to edit

**No component resolves a credential from configuration or source.** There is no
line in a config file, no environment variable, and no field in the deployment manifest where a key
belongs. A credential goes into the broker's sealed keystore, and the platform asks the broker.

The gate named `Credential resolution` fails the build if any component reads one from anywhere
else, so this is the only path there is.

## Before step 1: where the tool is

On an operator's host the release artefact installs it at `/opt/bijective/bin/bijective-secrets` and it
is on `PATH`. **On a build host it is not installed** - it is at
`crates/secrets_broker/target/debug/bijective-secrets` under the platform tree, and
`command not found` is what that looks like.

```
cargo build --bin bijective-secrets          # from crates/secrets_broker
install -m 0755 target/debug/bijective-secrets ~/.local/bin/
```

The keystore path below is the deployed one. On a build host `/var/lib` needs root, so use a
directory you own - `mkdir -p -m 0700 ~/.bijective` - and substitute it throughout. **The keystore
never belongs inside the repository**, whichever host it is on.

## Steps

The tool reads secrets from **standard input, never from an argument** - arguments are visible in
`ps` to every user on the host and land in shell history.

**1. Create the init credential and the keystore, once per host.** The node's units load the
init credential `bijective-init` from `/etc/bijective/credentials/bijective-init`, and the engines
open only a keystore sealed under it. It is 32 random bytes, readable by root alone.

```
sudo install -d -m 0700 /etc/bijective/credentials
sudo sh -c 'umask 077; head -c 32 /dev/urandom > /etc/bijective/credentials/bijective-init'
INIT="--init-credential /etc/bijective/credentials/bijective-init"
sudo bijective-secrets $INIT init /var/lib/bijective/keystore.bjks </dev/null
```

**2. Load each credential.** The venue needs two: the key identifier and the secret. Under the init
credential the tool reads the secret alone.

```
read -rs -p "API key: " K; echo
printf '%s\n' "$K" | sudo bijective-secrets $INIT put /var/lib/bijective/keystore.bjks venue-api-key
unset K

read -rs -p "API secret: " S; echo
printf '%s\n' "$S" | sudo bijective-secrets $INIT put /var/lib/bijective/keystore.bjks venue-api-secret
unset S
```

`read -rs` does not echo, and the value never becomes part of a command, so it does not reach shell
history either.

**3. Confirm what is stored.**

```
sudo bijective-secrets $INIT list /var/lib/bijective/keystore.bjks </dev/null
```

It prints **names only**. A tool that echoed what it stored would put a credential in a scrollback
buffer.

Without `--init-credential` the tool seals under a passphrase it reads first on standard input. Such
a keystore is for an operator's own use: the engines refuse it, because a passphrase would have to
be typed at every start of an unattended node.

## What the tool refuses, and why each one matters

| Refusal | Why |
|---|---|
| A wrong passphrase, or another init credential | The seal is the point. If it opened, the file would be a file with extra steps |
| A passphrase keystore under `--init-credential` | Refused here, not at the node's first start, where the engines would refuse it |
| `init` over an existing keystore | It would discard every credential in it, and the operator would find out at the next start |
| An empty passphrase | Almost always a pipeline that produced nothing, and it would look like it worked |
| A secret as an argument | Not a path the tool has |

All of them are demonstrated by the drill on every run.

## Rotation

Run step 2 again with the same name. The tool reports `replaced` rather than `stored`. **Do not
delete and re-add** - a keystore that is briefly missing a credential is a platform that briefly
cannot start.

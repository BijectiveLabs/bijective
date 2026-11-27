# Reach the console from another machine

Platform-Version: 0.1.0
Drill: scripts/verify/console_tls.sh
Applies-To: an operator who reads or controls a node from a machine other than the node's host

## Why

The console is the only program on a node that listens on the network. It listens on loopback
unless it is given HTTPS with client certificates, so the two ways to reach it from elsewhere are
an SSH tunnel to loopback, or HTTPS that answers only browsers holding a certificate your own
authority issued.

## Through an SSH tunnel (no certificates)

On the node, start the console as usual; it binds `127.0.0.1`:

```
bijective-console --store /var/lib/bijective/store --archive /var/lib/bijective/archive
```

From your machine:

```
ssh -N -L 7333:127.0.0.1:7333 operator@node-host
```

Open `http://127.0.0.1:7333/`. The console reads the live node's record unless it is given
`--mode paper` or `--mode backtest` ([decision 0017](../decisions/0017-one-console-for-every-run-mode.md)):
the same pages then show that run, its mode leads the strip, and the control page signs for that
run's node only. A backtest's record directory is read by pointing `--store` at it.

Whoever can open the tunnel can read the console, so the SSH
account is the access control.

## Over HTTPS with client certificates

You need three PEM files from an authority you control: the server certificate and its key (for
the address browsers will use), and the authority certificate that issues your operators' client
certificates. The console ships none and generates none.

```
bijective-console --store /var/lib/bijective/store --listen 0.0.0.0 --port 7333 \
    --tls-cert server.pem --tls-key server.key --client-ca operators-ca.pem
```

An address other than loopback is refused without all three files. A browser without a
certificate the authority issued is refused in the TLS handshake, before any request is read.
Import each operator's client certificate into their browser, and issue certificates only to the
people who may read this node.

## Controlling the node from the console

The control page (`/control`) signs commands with an operator's own key, exactly as
`bijective issue` does. Start the console with that operator:

```
bijective-console --store ... --operator ana --role operator --key /home/ana/ana.key
```

The page offers the actions the role may take (halt, reduce, resume, cancel orders, flatten, withdraw or
admit a venue); position adjustments stay on the command line. Every form carries a token that
changes each time the console starts, so a page on another site cannot submit one. The control engine verifies the
signature against `operators.json` and the role on arrival, and what came of each command appears
under Audit.

Run one console per operator who controls from a browser, and with HTTPS give that console's
client certificates only to that operator: the page signs as the operator it was started with.

## Verify

`bash scripts/verify/console_tls.sh` makes a throwaway authority, starts the console over HTTPS
and checks each refusal and the signed command.

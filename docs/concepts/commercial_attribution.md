# Commercial attribution

Some exchanges pay software vendors a share of the fees on orders the software sends, if the order
carries a broker id, a channel code, a referral tag or a builder fee. Done without the client's
knowledge, that is a hidden commission on the client's trading. This platform does not do it.

## What the platform sends today

Nothing that identifies it for revenue. Every request an adapter sends carries authentication and
a content type, and an order carries its trading fields and the client order identifier assigned
before the call, with no vendor prefix. There is no broker id, no referral, no channel code and no
builder fee in any adapter, and no third-party trading library sits between the platform and a
venue: each adapter speaks the venue's own protocol.

## The rule

An attribution may be sent with an order only when all of the following hold:

1. **Opt-in.** It is off unless the operator turns it on in configuration. No build, edition or
   release turns it on by default.
2. **Disclosed.** The venue, what is sent, who earns, and where the agreement is described are
   written in the documentation before the release that sends it, and the declaration list below
   names it.
3. **Visible.** The running node reports it in its status, so an operator can see that it is on
   without reading the source.
4. **Free to the client.** It costs the client nothing beyond the venue's own fee. A programme that
   charges the client extra for the vendor's share is not used.
5. **Removable in one line.** Turning it off is one configuration change, and nothing else
   depends on it.

Commercial agreements with a venue concern the integration and the support of it. They never
change an order.

## How the rule is enforced

The `Order attribution` gate fails the build when an adapter sends a request header that is not on
the allow-list (`scripts/ci/request_headers.txt`: authentication and content type), or when an
adapter source file contains a broker id, referral, affiliate, channel code, partner id or builder
fee, unless the file is named in `scripts/ci/attribution_declared.txt` with its disclosure.
Adding a line to either file is a reviewed decision that carries the documentation with it.

## For clients

You can verify this yourself: read the adapter source for the venue you use, or run the node
against a local server and look at the requests. A request that carries more than authentication
and the order is a defect, and it is reported as one.

## Fields a venue requires

Polymarket's signed order carries a `builder` field for integrator attribution. The adapter always
sends it as the zero word, so no order earns anyone a share. It is listed in
`scripts/ci/attribution_zeroed.txt`, and the `Order attribution` gate accepts it only on a line
that writes the zero value.


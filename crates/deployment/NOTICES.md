# Third-party notices, and why election is a decision

`Release_Gate.md` L1 requires an *Open Source Notices* surface and a `THIRD_PARTY_LICENSES` file,
enforced by a build check. **Both were claimed and neither existed** - the same shape OSS Policy P8
had before the attestation gate was built.

It is not bookkeeping. **MIT and Apache-2.0 both require their notice to accompany
redistribution**, and they cover most of what the platform links, so a binary shipped without them
is in breach of nearly every dependency it has.

## Election

When a package offers several licences we notice exactly one: the first permissive option in the
fixed order listed in the generator (`ELECTION_ORDER` in `src/notices.rs`). Noticing an option we
did not take would state that we accepted terms we refused.

A package that offers only a copyleft licence, or none we elect, fails the build. It is one we may
not link, and the failure names the package.

## Conjunctions of choices

`(MIT OR Apache-2.0) AND Unicode-3.0` means elect one of the first two **and comply with the third
as well**. Treating the whole expression as one flat choice reads it as *any of the three will do*,
which is how a mandatory term gets silently dropped. `unicode-ident` is exactly that shape and is
what caught it.

## Why the file is generated rather than written

The generator has no timestamp, no host name and no varying iteration order, so the build check is
a **diff rather than an opinion** - the same property the unit generator needs, for the same
reason. A hand-edited notices file is detectable the way a hand-edited unit is.

One copy of each distinct licence text rather than one per package: two hundred packages under
Apache-2.0 owe one Apache-2.0 notice between them, and a file repeating it two hundred times would
be read by no one.

## What it refuses

- **A package whose every option we decline.** It is one we may not link, and leaving it silently
  out of the notices would hide that.
- **A licence named with no text found.** A notices file missing the text of a licence it names
  does not discharge that obligation, and looks exactly like one that does.

# Venue profiles - the document a check verifies against

The venue profile format: a file an operator edits and a Client can read without us. One per venue,
named for it: the scheduled check reads `/etc/bijective/profiles/<venue>.json`.

`example.json` is a complete, valid document for a venue that does not exist. It is here because the
format has four rules a hand-written first attempt gets wrong, and finding them one refusal at a
time is time a Client should not spend:

- **Wide integers travel as text.** `"price_tick": "10"`, not `10` - the same rule the rest of the
  platform runs on, and for the same reason: a JSON number is a double once it has been through
  most parsers.
- **Enumerations are tagged and hyphenated.** `{"rule": "increment-only"}`, `{"model": "published",
  "interval_ns": "28800000000000"}`.
- **The instrument catalogue is a plain list**, not an object wrapping one. The symbol is a field of
  each entry rather than a key above it.
- **The support entry names the same venue as the profile.** A profile curated for one venue
  carrying another's support entry would let a data-only curation be dodged by pasting one in.

A profile carries no verification date. Whether it has been verified is a marker held beside it -
the assurance stream - and a document that could assert its own verification would be a declaration
certifying itself.

#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Fixtures the drills run against, built from the current types rather than kept as files.
# A checked-in profile or configuration drifts from its type. Building them here breaks the drills loudly.
# Sourced by every drill that needs a node.

# Exactly 32 bytes: the broker takes the credential verbatim.
# verbatim: a credential of the wrong length is one this deployment did not produce.
drill_credentials_dir() {
    local dir="$1" name="$2"
    mkdir -p "$dir"
    head -c 32 /dev/urandom > "${dir}/${name}"
}

drill_profile() {
    local core="$1" venue="$2" symbols="$3" out="$4"
    "${core}/target/debug/examples/drill_profile" "$venue" "$symbols" > "$out"
}

drill_keystore() {
    local core="$1" creds="$2" name="$3" out="$4"
    rm -f "$out"
    CREDENTIALS_DIRECTORY="$creds" \
        "${core}/target/debug/examples/seal_under_init_credential" \
        "$out" "$name" drill-key=drill-key drill-secret=drill-secret >/dev/null
}

drill_config() {
    local venue="$1" profile="$2" instrument="$3" out="$4"
    cat > "$out" <<JSON
{
  "version": 1,
  "venues": ["${venue}"],
  "venue_access": [
    {
      "name": "${venue}",
      "endpoint": "https://drill.invalid",
      "profile": "${profile}",
      "key_name": "drill-key",
      "secret_name": "drill-secret",
      "settle_coin": "usdt",
      "request_timeout_ns": "10000000000",
      "order_rate": { "orders": 10, "per_ns": "1000000000" }
    }
  ],
  "desks": [
    {
      "name": "alpha",
      "instruments": ["${instrument}"],
      "venues": ["${venue}"],
      "maximum_per_order_scaled": "100000000"
    }
  ],
  "limits": [
    { "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "100000000000" }
  ],
  "sandbox": { "${venue}": { "usdt": "100000000000" } },
  "freshness_bound_ns": "5000000000"
}
JSON
}

drill_node_fixtures() {
    local core="$1" work="$2" venue="$3" symbols="$4"
    drill_credentials_dir "${work}/creds" drill-cred
    drill_profile "$core" "$venue" "$symbols" "${work}/profile.json"
    drill_keystore "$core" "${work}/creds" drill-cred "${work}/keystore.bin"
    drill_config "$venue" "${work}/profile.json" "${venue}.${symbols%%,*}" "${work}/config.json"
    drill_operators "${work}/operators.json"
}

# Live mode refuses to start without a registry of who may sign commands. The key is RFC 8032's
# published test key: public by design, it signs nothing a drill depends on.
drill_operators() {
    printf '%s\n' '{ "version": 1, "operators": [ { "name": "drill-operator", "role": "operator", "public_key": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a" } ] }' >"$1"
}

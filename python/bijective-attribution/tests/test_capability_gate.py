# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance gates for C5, the capability gate.
Capability is read from the declaration, never inferred; the gate affects the figure, not trading.
"""

from bijective.attribution.calculator import Availability
from bijective.attribution.capability import UnknownCapability, gate, resolve


def test_an_undeclared_field_resolves_to_unavailable():
    """No declaration from an adapter is not a grant."""
    declaration = resolve({"fee": "observed"})

    assert declaration.fee is Availability.OBSERVED
    assert declaration.liquidity is Availability.UNAVAILABLE
    assert declaration.acknowledgement is Availability.UNAVAILABLE


def test_an_empty_declaration_resolves_to_nothing_available():
    """An adapter that declares nothing supports no attributed component."""
    declaration = resolve({})

    for field_name in ("liquidity", "acknowledgement", "fee"):
        assert getattr(declaration, field_name) is Availability.UNAVAILABLE


def test_an_unrecognised_availability_token_is_refused():
    """A token we do not understand must not be guessed at."""
    try:
        resolve({"fee": "probably"})
    except ValueError as refusal:
        assert "probably" in str(refusal)
    else:
        raise AssertionError("an unrecognised availability token was interpreted")


def test_an_unknown_component_name_fails_loudly():
    """A typo in a component name must not become a permanent silent unavailability."""
    declaration = resolve({"fee": "observed", "liquidity": "observed"})

    try:
        gate("fee_attribution", declaration)
    except UnknownCapability as refusal:
        assert "fee_attribution" in str(refusal)
    else:
        raise AssertionError("an unknown component name was silently gated")


def test_fee_attribution_requires_both_fee_and_liquidity():
    """Knowing the total charged is not knowing how it arose."""
    both = resolve({"fee": "observed", "liquidity": "observed"})
    assert gate("fee", both) is Availability.OBSERVED

    fee_only = resolve({"fee": "observed"})
    assert gate("fee", fee_only) is Availability.UNAVAILABLE

    liquidity_only = resolve({"liquidity": "observed"})
    assert gate("fee", liquidity_only) is Availability.UNAVAILABLE


def test_resolution_is_deterministic_for_identical_input():
    """The gate is a pure function; two resolutions of one declaration must agree."""
    raw = {"fee": "observed", "liquidity": "derived", "acknowledgement": "unavailable"}

    assert resolve(raw) == resolve(raw)

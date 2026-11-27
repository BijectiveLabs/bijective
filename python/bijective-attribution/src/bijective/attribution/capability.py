# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""C5 - the capability gate: what a substrate declares becomes what the platform will report.
Capability is read, never inferred. The gate affects the figure and never the trade.
"""

from __future__ import annotations

from collections.abc import Mapping

from bijective.attribution.calculator import Availability, CapabilityDeclaration

# Unknown fields are ignored so a newer adapter cannot break an older platform. A typo then
# resolves to unavailable, the safe side.
_DECLARABLE_FIELDS = ("liquidity", "acknowledgement", "fee")

_TOKENS = {
    "observed": Availability.OBSERVED,
    "derived": Availability.DERIVED,
    "unavailable": Availability.UNAVAILABLE,
}


class UnknownCapability(Exception):
    """Raised for an unknown component name; "unavailable" would pass for a substrate limitation."""


def resolve(raw: Mapping[str, str]) -> CapabilityDeclaration:
    """Turn an adapter's raw declaration into the typed form. Missing fields resolve to unavailable; unknown tokens are refused."""
    resolved: dict[str, Availability] = {}
    for field in _DECLARABLE_FIELDS:
        token = raw.get(field)
        if token is None:
            resolved[field] = Availability.UNAVAILABLE
            continue
        key = str(token).strip().lower()
        if key not in _TOKENS:
            raise ValueError(
                f"unrecognised availability token {token!r} for field {field!r}; "
                f"expected one of {sorted(_TOKENS)}"
            )
        resolved[field] = _TOKENS[key]
    return CapabilityDeclaration(**resolved)


def _combine(*availabilities: Availability) -> Availability:
    """Availability of a result: never better than its weakest input."""
    if any(a is Availability.UNAVAILABLE for a in availabilities):
        return Availability.UNAVAILABLE
    if any(a is Availability.DERIVED for a in availabilities):
        return Availability.DERIVED
    return Availability.OBSERVED


def gate(component: str, declaration: CapabilityDeclaration) -> Availability:
    """State whether a named cost component may be computed on this substrate.
    ``fee`` needs the fee and the liquidity side; ``ack_residual`` needs an acknowledgement state.
    """
    if component == "crossing":
        return Availability.OBSERVED
    if component == "fee":
        return _combine(declaration.fee, declaration.liquidity)
    if component == "ack_residual":
        return declaration.acknowledgement
    raise UnknownCapability(
        f"no capability rule for component {component!r}; "
        f"known components are 'crossing', 'fee', 'ack_residual'"
    )

# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""S1 - the expectation registry: what each integration must produce, how often, and why.
An undeclared integration is reported unmonitored, never healthy. Pure data and validation.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class RegistryError(ValueError):
    """A declaration was refused, loudly and at construction, so a malformed rule never reaches the register."""


class Severity(Enum):
    """How urgently a breach must reach a person. Declared, never inferred: the interval says nothing about urgency."""

    INFO = "info"
    WARNING = "warning"
    CRITICAL = "critical"


class Coverage(Enum):
    """Whether an integration is watched. Not a boolean: UNMONITORED must render as a finding, never as health."""

    DECLARED = "declared"
    UNMONITORED = "unmonitored"


@dataclass(frozen=True)
class Expectation:
    """What one integration must produce, how often, and why. Frozen so a threshold cannot move without a trace."""

    #: The integration covered. Named so an operator reading an alarm recognises it.
    integration: str

    #: The longest interval with no event that is still normal. Beyond this, the integration is not
    #: producing and the condition is raised.
    must_produce_within_ns: int

    #: Whether a subscription must be confirmed attached. Substrate-conditional: where unavailable,
    #: report unavailable and never treat the absence of an error as confirmation.
    acknowledgement_required: bool

    #: How urgently a breach must reach a person. See `Severity`.
    severity: Severity

    #: The incident or observation behind this expectation. Required, so a threshold can be audited and relaxed knowingly.
    origin: str

    def __post_init__(self) -> None:
        """Refuses a declaration that cannot be acted on or accounted for."""
        if not self.integration.strip():
            raise RegistryError(
                "an expectation must name the integration it covers, so an operator reading "
                "an alarm knows what stopped"
            )
        if self.must_produce_within_ns <= 0:
            raise RegistryError(
                f"the interval for {self.integration!r} must be positive: zero would raise "
                "permanently and a negative value would never raise"
            )
        if not self.origin.strip():
            raise RegistryError(
                f"the expectation for {self.integration!r} carries no origin. An interval "
                "with no stated reason cannot be judged later, and is the unexplained "
                "constant the coding standard refuses"
            )


class ExpectationRegistry:
    """The declared expectations, indexed, duplicates refused. Construction is the validation, as for limits."""

    def __init__(self, expectations: list[Expectation]) -> None:
        """Indexes `expectations`, refusing two declarations for one integration (two answers, one silently ignored)."""
        indexed: dict[str, Expectation] = {}
        for expectation in expectations:
            if expectation.integration in indexed:
                raise RegistryError(
                    f"{expectation.integration!r} is declared more than once; one integration "
                    "has one expectation, or the register shows a rule that is not enforced"
                )
            indexed[expectation.integration] = expectation
        self._by_integration = indexed

    def coverage_of(self, integration: str) -> Coverage:
        """Whether `integration` is watched. Anything not declared is UNMONITORED, never healthy."""
        if integration in self._by_integration:
            return Coverage.DECLARED
        return Coverage.UNMONITORED

    def unmonitored_among(self, integrations: list[str]) -> list[str]:
        """Which of `integrations` have no declared expectation, in the order given; the operator surface reports gaps."""
        return [
            integration
            for integration in integrations
            if self.coverage_of(integration) is Coverage.UNMONITORED
        ]

    def expectation_for(self, integration: str) -> Expectation | None:
        """The declared expectation for `integration`, or `None` if it is unmonitored."""
        return self._by_integration.get(integration)

    def declared(self) -> list[Expectation]:
        """Every declared expectation, in declaration order."""
        return list(self._by_integration.values())


#: Registry file layouts this loader understands. An unrecognised version is refused, not guessed at.
SUPPORTED_SCHEMA_VERSIONS = frozenset({1})

#: Fields every entry must carry, with no defaults: a default is a rule no one reviewed.
_REQUIRED_FIELDS = (
    "integration",
    "must_produce_within_ns",
    "acknowledgement_required",
    "severity",
    "origin",
)


def load_registry(path) -> ExpectationRegistry:
    """Reads an expectation registry from a JSON document.
    Refuses rather than defaulting on an unknown schema version, missing field or unknown severity, naming what and where.
    """
    import json
    from pathlib import Path

    document = json.loads(Path(path).read_text(encoding="utf-8"))

    version = document.get("schema_version")
    if version not in SUPPORTED_SCHEMA_VERSIONS:
        raise RegistryError(
            f"registry {path} declares schema version {version!r}, which this reader does not "
            f"understand (supported: {sorted(SUPPORTED_SCHEMA_VERSIONS)})"
        )

    entries = document.get("expectations")
    if not isinstance(entries, list):
        raise RegistryError(f"registry {path} carries no 'expectations' list")

    expectations: list[Expectation] = []
    for position, entry in enumerate(entries):
        missing = [field for field in _REQUIRED_FIELDS if field not in entry]
        if missing:
            raise RegistryError(
                f"expectation at position {position} in {path} is missing {missing}; "
                "no field here has a default, because an unreviewed default rule is worse than none"
            )
        try:
            severity = Severity(entry["severity"])
        except ValueError as unknown_severity:
            raise RegistryError(
                f"expectation at position {position} in {path} declares severity "
                f"{entry['severity']!r}, which is not one of "
                f"{[level.value for level in Severity]}"
            ) from unknown_severity

        expectations.append(
            Expectation(
                integration=entry["integration"],
                must_produce_within_ns=entry["must_produce_within_ns"],
                acknowledgement_required=entry["acknowledgement_required"],
                severity=severity,
                origin=entry["origin"],
            )
        )

    return ExpectationRegistry(expectations)

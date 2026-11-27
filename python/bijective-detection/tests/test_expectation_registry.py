# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance tests for the expectation registry.
Every expectation carries the incident that taught it, so thresholds are auditable and never unexplained constants.
"""

import pytest

from bijective.detection.expectations import (
    Coverage,
    Expectation,
    ExpectationRegistry,
    RegistryError,
    Severity,
)

NS_PER_S = 1_000_000_000


def _expectation(integration: str = "venue-a.public.orderbook") -> Expectation:
    """A declared expectation with every field populated, used as the baseline."""
    return Expectation(
        integration=integration,
        must_produce_within_ns=30 * NS_PER_S,
        acknowledgement_required=True,
        severity=Severity.CRITICAL,
        origin=(
            "incident 2026-07-16: a subscribe response was swallowed, and the dead topic "
            "went unrecorded for more than nine days"
        ),
    )


def test_an_integration_with_no_declared_expectation_is_unmonitored_not_healthy():
    registry = ExpectationRegistry([_expectation("venue-a.public.orderbook")])

    coverage = registry.coverage_of("venue-a.private.execution")

    assert coverage is Coverage.UNMONITORED


def test_a_declared_integration_reports_as_declared():
    registry = ExpectationRegistry([_expectation("venue-a.public.orderbook")])

    assert registry.coverage_of("venue-a.public.orderbook") is Coverage.DECLARED


def test_the_registry_can_list_what_it_does_not_cover():
    registry = ExpectationRegistry([_expectation("venue-a.public.orderbook")])

    unmonitored = registry.unmonitored_among(
        ["venue-a.public.orderbook", "venue-a.private.execution", "venue-a.public.liquidation"]
    )

    assert unmonitored == ["venue-a.private.execution", "venue-a.public.liquidation"]


def test_an_expectation_without_an_origin_is_refused():
    with pytest.raises(RegistryError):
        Expectation(
            integration="venue-a.public.orderbook",
            must_produce_within_ns=30 * NS_PER_S,
            acknowledgement_required=True,
            severity=Severity.CRITICAL,
            origin="",
        )


def test_a_non_positive_interval_is_refused():
    # Zero fires permanently. A negative value never fires.
    with pytest.raises(RegistryError):
        Expectation(
            integration="venue-a.public.orderbook",
            must_produce_within_ns=0,
            acknowledgement_required=False,
            severity=Severity.WARNING,
            origin="a reason",
        )


def test_an_expectation_without_an_integration_name_is_refused():
    with pytest.raises(RegistryError):
        Expectation(
            integration="   ",
            must_produce_within_ns=30 * NS_PER_S,
            acknowledgement_required=False,
            severity=Severity.WARNING,
            origin="a reason",
        )


def test_severity_comes_from_the_declaration_not_from_the_interval():
    # A short interval does not imply an urgent alarm, nor a long one a quiet one.
    urgent_but_slow = Expectation(
        integration="venue.funding.settlement",
        must_produce_within_ns=8 * 3600 * NS_PER_S,
        acknowledgement_required=False,
        severity=Severity.CRITICAL,
        origin="funding settles every eight hours; missing one is a real cost, not a delay",
    )
    routine_but_fast = Expectation(
        integration="venue.public.trades",
        must_produce_within_ns=5 * NS_PER_S,
        acknowledgement_required=False,
        severity=Severity.WARNING,
        origin="trade flow is continuous in liquid hours; a five-second gap is worth noting",
    )

    assert urgent_but_slow.severity is Severity.CRITICAL
    assert routine_but_fast.severity is Severity.WARNING


def test_two_expectations_for_one_integration_are_refused():
    with pytest.raises(RegistryError):
        ExpectationRegistry(
            [
                _expectation("venue-a.public.orderbook"),
                _expectation("venue-a.public.orderbook"),
            ]
        )


def test_the_shipped_reference_registry_loads_and_every_entry_states_its_origin():
    from pathlib import Path

    from bijective.detection.expectations import load_registry

    reference = Path(__file__).resolve().parents[1] / "reference" / "crypto_venue.expectations.json"

    registry = load_registry(reference)

    declared = registry.declared()
    assert declared, "the reference registry is empty"
    for expectation in declared:
        assert expectation.origin.strip(), f"{expectation.integration} carries no origin"
        assert expectation.must_produce_within_ns > 0


def test_loading_a_registry_with_a_missing_field_is_refused():
    # A truncated file must fail loudly, not yield a default value.
    import json
    import tempfile
    from pathlib import Path

    from bijective.detection.expectations import load_registry

    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "broken.json"
        path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "expectations": [
                        {"integration": "venue.public.trades", "severity": "warning"}
                    ],
                }
            )
        )
        with pytest.raises(RegistryError):
            load_registry(path)


def test_loading_a_registry_with_an_unknown_severity_is_refused():
    import json
    import tempfile
    from pathlib import Path

    from bijective.detection.expectations import load_registry

    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "bad_severity.json"
        path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "expectations": [
                        {
                            "integration": "venue.public.trades",
                            "must_produce_within_ns": 5_000_000_000,
                            "acknowledgement_required": False,
                            "severity": "urgent-ish",
                            "origin": "a reason",
                        }
                    ],
                }
            )
        )
        with pytest.raises(RegistryError):
            load_registry(path)


def test_loading_a_registry_of_an_unknown_schema_version_is_refused():
    import json
    import tempfile
    from pathlib import Path

    from bijective.detection.expectations import load_registry

    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "future.json"
        path.write_text(json.dumps({"schema_version": 99, "expectations": []}))
        with pytest.raises(RegistryError):
            load_registry(path)

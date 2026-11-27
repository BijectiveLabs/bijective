# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""What a strategy written in Python reads and writes.

Assertions are on the wire bytes, since a round trip through this module could be wrong on both halves.
"""

import io
import json

from bijective_strategy import Book, EndOfWindow, Resumed, Unrecognised, answered, events, propose, read_event

BOOK = (
    '{"event":"book","position":"7","instrument":"btc-perp","venue_sequence":"3",'
    '"venue_ts_ms":"1788000000000","bid_price":"2654375","bid_quantity":"1500",'
    '"ask_price":"2654500","ask_quantity":"900","bid_depth":5,"ask_depth":5}'
)


def test_a_book_arrives_with_every_number_still_text():
    event = read_event(BOOK)
    assert isinstance(event, Book)
    assert event.position == "7"
    assert event.bid_price == "2654375"
    # Scaled integers cross as decimal strings. A float here would be the platform's one prohibited conversion.
    assert isinstance(event.bid_price, str)
    assert event.bid_depth == 5


def test_an_event_this_version_does_not_know_is_returned_not_raised():
    event = read_event('{"event":"something_added_later","detail":"x"}')
    assert isinstance(event, Unrecognised)


def test_a_book_missing_a_field_is_not_half_a_book():
    partial = '{"event":"book","position":"7","instrument":"btc-perp"}'
    assert isinstance(read_event(partial), Unrecognised)


def test_the_two_events_that_are_not_books():
    assert isinstance(read_event('{"event":"end_of_window"}'), EndOfWindow)
    resumed = read_event('{"event":"resumed","from_position":"12","missed":{"missed":"not_recorded"}}')
    assert isinstance(resumed, Resumed)
    assert resumed.from_position == "12"


def test_a_line_that_is_not_json_is_not_an_exception():
    assert isinstance(read_event("a strategy logged where it should not"), Unrecognised)


def test_events_skips_blank_lines_and_keeps_order():
    stream = io.StringIO(BOOK + "\n\n" + '{"event":"end_of_window"}' + "\n")
    read = list(events(stream))
    assert [type(e) for e in read] == [Book, EndOfWindow]


def test_a_proposal_carries_what_was_given_and_nothing_else():
    out = io.StringIO()
    propose(
        desk="alpha",
        instrument="btc-perp",
        side="buy",
        quantity_scaled="1",
        price_scaled="3000000",
        client_id="c1",
        decided_at_ns="1",
        out=out,
    )
    written = json.loads(out.getvalue())
    assert written == {
        "emit": "proposal",
        "desk": "alpha",
        "instrument": "btc-perp",
        "side": "buy",
        "quantity_scaled": "1",
        "price_scaled": "3000000",
        "client_id": "c1",
        "decided_at_ns": "1",
    }
    # Absent stays absent: null is not the same as a field that was not sent.
    assert "cap_scaled" not in written
    assert "decided_at_position" not in written


def test_a_decision_can_name_the_event_it_was_made_on():
    out = io.StringIO()
    propose(
        desk="alpha",
        instrument="btc-perp",
        side="buy",
        quantity_scaled="1",
        price_scaled="3000000",
        client_id="c1",
        decided_at_ns="1",
        decided_at_position="7",
        out=out,
    )
    assert json.loads(out.getvalue())["decided_at_position"] == "7"


def test_the_turn_marker_is_exactly_what_the_supervisor_waits_for():
    out = io.StringIO()
    answered(out=out)
    assert out.getvalue() == '{"emit":"answered"}\n'


def test_every_line_ends_with_a_newline_because_a_message_is_a_whole_line():
    out = io.StringIO()
    propose(
        desk="alpha",
        instrument="btc-perp",
        side="buy",
        quantity_scaled="1",
        price_scaled="1",
        client_id="c1",
        decided_at_ns="1",
        out=out,
    )
    answered(out=out)
    assert out.getvalue().count("\n") == 2
    for line in out.getvalue().splitlines():
        json.loads(line)


def test_a_top_carries_its_venue_and_a_proposal_may_name_one() -> None:
    import io

    from bijective_strategy import Book, propose, read_event

    line = (
        '{"event":"book","position":"1","instrument":"btc-perp","venue":"venue-a",'
        '"venue_ts_ms":"1","bid_price":"9","bid_quantity":"1","ask_price":"11",'
        '"ask_quantity":"1","bid_depth":1,"ask_depth":1}'
    )
    top = read_event(line)
    assert isinstance(top, Book) and top.venue == "venue-a"
    out = io.StringIO()
    propose(desk="d", instrument="btc-perp", side="buy", quantity_scaled="1", price_scaled="9",
            client_id="c", decided_at_ns="1", venue="venue-b", out=out)
    assert '"venue": "venue-b"' in out.getvalue() or '"venue":"venue-b"' in out.getvalue()


def test_the_venue_line_carries_fees_and_unsaid_fees_stay_absent() -> None:
    from bijective_strategy import Venue, read_event

    line = (
        '{"event":"venue","venue":"venue-a","taker_fee_bps_scaled":"550000000",'
        '"maker_fee_bps_scaled":"-100000000","fee_source":"venue fee page, read 2026-11-04"}'
    )
    opened = read_event(line)
    assert isinstance(opened, Venue)
    assert (opened.venue, opened.taker_fee_bps_scaled) == ("venue-a", "550000000")
    unsaid = read_event('{"event":"venue","venue":"venue-a"}')
    assert isinstance(unsaid, Venue) and unsaid.taker_fee_bps_scaled is None


def test_an_order_update_carries_a_fill_and_a_refusal_names_no_order() -> None:
    from bijective_strategy import OrderUpdate, read_event

    filled = read_event(
        '{"event":"order","proposal_position":"12","client_id":"c-1","status":"filled",'
        '"fill_id":"f-9","fill_quantity_scaled":"1500","fill_price_scaled":"2654375"}'
    )
    assert isinstance(filled, OrderUpdate)
    assert (filled.status, filled.fill_quantity_scaled) == ("filled", "1500")
    refused = read_event('{"event":"order","proposal_position":"13","status":"refused","reason":"x"}')
    assert isinstance(refused, OrderUpdate) and refused.client_id is None and refused.reason == "x"


def test_the_venue_line_carries_each_instruments_increments() -> None:
    from bijective_strategy import Venue, read_event

    opened = read_event(
        '{"event":"venue","venue":"venue-a","instruments":[{"instrument":"btc-perp",'
        '"price_scale":1,"quantity_scale":3,"price_tick":"5","quantity_lot":"1",'
        '"minimum_size":"1","minimum_notional":"50","trading":true}]}'
    )
    assert isinstance(opened, Venue)
    assert opened.instruments[0].price_tick == "5" and opened.instruments[0].trading


def test_a_top_carries_when_it_arrived_and_an_old_top_says_nothing() -> None:
    from bijective_strategy import Book, read_event

    base = (
        "{\"event\":\"book\",\"position\":\"1\",\"instrument\":\"btc-perp\",\"venue\":\"venue-a\","
        "\"venue_ts_ms\":\"1\",\"bid_price\":\"9\",\"bid_quantity\":\"1\",\"ask_price\":\"11\","
        "\"ask_quantity\":\"1\",\"bid_depth\":1,\"ask_depth\":1"
    )
    stamped = read_event(base + ",\"received_at_ns\":\"1788000000001000000\"}")
    assert isinstance(stamped, Book) and stamped.received_at_ns == "1788000000001000000"
    old = read_event(base + "}")
    assert isinstance(old, Book) and old.received_at_ns is None


def test_an_instrument_says_what_it_is_and_a_session_of_several_venues_resumes_per_venue() -> None:
    from bijective_strategy import Resumed, Venue, read_event

    opened = read_event(
        "{\"event\":\"venue\",\"venue\":\"venue-a\",\"instruments\":[{\"instrument\":\"btc-perp\","
        "\"price_scale\":1,\"quantity_scale\":3,\"price_tick\":\"5\",\"quantity_lot\":\"1\","
        "\"minimum_size\":\"1\",\"minimum_notional\":\"50\",\"trading\":true,"
        "\"kind\":\"perpetual\",\"quote_currency\":\"usdt\",\"settlement_currency\":\"usdt\",\"base_currency\":\"btc\"}]}"
    )
    assert isinstance(opened, Venue)
    rules = opened.instruments[0]
    assert (rules.kind, rules.quote_currency, rules.settlement_currency) == (
        "perpetual",
        "usdt",
        "usdt",
    )
    resumed = read_event(
        "{\"event\":\"resumed\",\"from_position\":\"3\",\"missed\":{\"missed\":\"not_recorded\"},"
        "\"venue\":\"venue-b\"}"
    )
    assert isinstance(resumed, Resumed) and resumed.venue == "venue-b"


def test_a_fact_of_any_kind_reads_as_one_event_with_its_values() -> None:
    from bijective_strategy import Fact, read_event

    trade = read_event(
        '{"event":"fact","kind":"trade","fact_position":"4","instrument":"btc-perp",'
        '"venue":"venue-a","received_at_ns":"9","trade_id":"t-1","aggressor":"seller",'
        '"price_scaled":"2654375","quantity_scaled":"1500"}'
    )
    assert isinstance(trade, Fact)
    assert (trade.kind, trade.fact_position, trade.venue) == ("trade", "4", "venue-a")
    assert trade.values == {
        "trade_id": "t-1",
        "aggressor": "seller",
        "price_scaled": "2654375",
        "quantity_scaled": "1500",
    }
    later = read_event('{"event":"fact","kind":"basis","instrument":"btc-perp","basis":"3"}')
    assert isinstance(later, Fact) and later.kind == "basis" and later.values == {"basis": "3"}


def test_a_proposal_states_its_terms_only_when_asked() -> None:
    import io
    import json

    from bijective_strategy import propose

    plain, stated = io.StringIO(), io.StringIO()
    common = dict(desk="alpha", instrument="i", side="buy", quantity_scaled="1",
                  price_scaled="100", client_id="c", decided_at_ns="1")
    propose(**common, out=plain)
    propose(**common, time_in_force="ioc", post_only=True, out=stated)
    assert "time_in_force" not in json.loads(plain.getvalue())
    assert "post_only" not in json.loads(plain.getvalue())
    line = json.loads(stated.getvalue())
    assert (line["time_in_force"], line["post_only"]) == ("ioc", True)
    stop = io.StringIO()
    propose(**common, stop_price_scaled="90", reduce_only=True, out=stop)
    line = json.loads(stop.getvalue())
    assert (line["stop_price_scaled"], line["reduce_only"]) == ("90", True)
    assert "reduce_only" not in json.loads(plain.getvalue())
    child = io.StringIO()
    propose(**common, reduce_only=True, parent="entry-1", oco="bracket", out=child)
    line = json.loads(child.getvalue())
    assert (line["parent"], line["oco"]) == ("entry-1", "bracket")
    assert "parent" not in json.loads(plain.getvalue())
    assert "oco" not in json.loads(plain.getvalue())
    dated = io.StringIO()
    propose(**common, expire_at_ns="1800000000000000000", out=dated)
    assert json.loads(dated.getvalue())["expire_at_ns"] == "1800000000000000000"
    assert "expire_at_ns" not in json.loads(plain.getvalue())

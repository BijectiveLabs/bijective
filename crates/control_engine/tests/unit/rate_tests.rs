// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::{OrderRate, RateWindow};

fn window() -> RateWindow {
    RateWindow::new(OrderRate { orders: 2, per_ns: 100 })
}

#[test]
fn no_window_of_the_rate_holds_more_than_its_orders() {
    let mut w = window();
    assert_eq!(w.wait_ns(0), None);
    w.sent(0);
    assert_eq!(w.wait_ns(10), None);
    w.sent(10);
    assert_eq!(w.wait_ns(50), Some(50), "the submit at 0 leaves the window at 100");
    assert_eq!(w.wait_ns(100), None);
    w.sent(100);
    assert_eq!(w.wait_ns(105), Some(5), "the submit at 10 leaves at 110");
}

#[test]
fn a_venue_rate_refusal_closes_the_window_for_what_it_stated_or_one_window() {
    let mut w = window();
    w.refused(0, Some(30));
    assert_eq!(w.wait_ns(10), Some(20));
    assert_eq!(w.wait_ns(30), None);
    w.refused(40, None);
    assert_eq!(w.wait_ns(40), Some(100));
}

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! When a portfolio summary is recorded, and when it is not.

use super::*;

const MINUTE: i128 = AT_MOST_EVERY_NS;

fn valued(gross: Option<i128>, loss: Option<i128>) -> Valued {
    Valued {
        gross_scaled: gross,
        net_scaled: gross,
        daily_loss_scaled: loss,
        observed_at_ns: 1,
        leverage_scaled: None,
        margin_use_scaled: None,
        equity_scaled: None,
    }
}

#[test]
fn the_first_summary_is_recorded() {
    assert!(due(None, &valued(Some(1_000), None), 0));
}

#[test]
fn a_figure_that_appears_or_is_lost_is_recorded_at_once() {
    let held = valued(Some(1_000), Some(10));
    assert!(due(Some((&held, 0)), &valued(None, Some(10)), 1), "the book could not be valued");
    assert!(due(Some((&held, 0)), &valued(Some(1_000), None), 1), "the loss became unmeasured");
}

#[test]
fn a_move_of_a_hundredth_is_recorded_at_once_and_a_smaller_one_waits_a_minute() {
    let held = valued(Some(100_000), None);
    assert!(due(Some((&held, 0)), &valued(Some(101_000), None), 1));
    assert!(!due(Some((&held, 0)), &valued(Some(100_500), None), 1));
    assert!(due(Some((&held, 0)), &valued(Some(100_500), None), MINUTE));
}

#[test]
fn nothing_changed_is_never_recorded_however_long_it_has_been() {
    let held = valued(Some(100_000), Some(0));
    assert!(!due(Some((&held, 0)), &held, 10 * MINUTE));
}

#[test]
fn a_figure_moving_away_from_zero_is_material() {
    let held = valued(Some(0), None);
    assert!(due(Some((&held, 0)), &valued(Some(1), None), 1));
}

#[test]
fn the_portfolio_figures_are_read_from_the_snapshot_and_nothing_else_is() {
    use bijective_limits::Amount;
    let money = |v: i128| Threshold::Money(Amount::from_scaled(v));
    let snapshot = ExposureSnapshot {
        observed_at_ns: 7,
        figures: vec![
            (Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(9)),
            (Scope::Portfolio, LimitKind::GrossExposure, money(50)),
            (Scope::Portfolio, LimitKind::NetExposure, money(-20)),
            (Scope::Portfolio, LimitKind::Leverage, ratio(25_000)),
        ],
    };
    let figures = portfolio(&snapshot, Some(900));
    assert_eq!(figures.equity_scaled, Some(900));
    assert_eq!(figures.gross_scaled, Some(50));
    assert_eq!(figures.net_scaled, Some(-20));
    assert_eq!(figures.daily_loss_scaled, None);
    assert_eq!(figures.observed_at_ns, 7);
    assert_eq!(figures.leverage_scaled, Some(25_000));
    assert_eq!(figures.margin_use_scaled, None, "absent, never zero");
}

fn ratio(v: i128) -> Threshold {
    Threshold::Ratio(bijective_limits::Ratio::from_scaled(v))
}

#[test]
fn a_leverage_move_of_a_hundredth_is_recorded_at_once() {
    let with = |l: i128| Valued { leverage_scaled: Some(l), ..valued(Some(100_000), None) };
    assert!(due(Some((&with(20_000), 0)), &with(20_200), 1));
    assert!(!due(Some((&with(20_000), 0)), &with(20_100), 1));
    assert!(due(Some((&valued(Some(100_000), None), 0)), &with(20_000), 1), "it appeared");
}

/// The equity follows the same rule: lost at once, a hundredth at once, a smaller move in a minute.
#[test]
fn an_equity_move_is_recorded_on_the_same_rule() {
    let with = |e: Option<i128>| Valued { equity_scaled: e, ..valued(Some(100_000), None) };
    assert!(due(Some((&with(Some(10_000)), 0)), &with(None), 1), "it was lost");
    assert!(due(Some((&with(Some(10_000)), 0)), &with(Some(9_900)), 1));
    assert!(!due(Some((&with(Some(10_000)), 0)), &with(Some(9_950)), 1));
    assert!(due(Some((&with(Some(10_000)), 0)), &with(Some(9_950)), MINUTE));
}

fn at(scope: &str, value: i128) -> crate::wire::ScopeFigure {
    crate::wire::ScopeFigure {
        scope: bijective_limits::Scope::Desk(scope.into()),
        kind: LimitKind::GrossExposure,
        value_scaled: value,
    }
}

/// The scoped figures follow the portfolio summary's rule: a scope appearing or going, or a move
/// of a hundredth, is recorded at once. A smaller change waits a minute. No change never is.
#[test]
fn scoped_figures_are_recorded_on_the_portfolio_summary_s_rule() {
    let last = vec![at("alpha", 10_000)];
    assert!(scoped_due(None, &last, 0));
    assert!(!scoped_due(None, &[], 0), "nothing to show is not a record");
    assert!(scoped_due(Some((&last, 0)), &[at("alpha", 10_000), at("beta", 1)], 1));
    assert!(scoped_due(Some((&last, 0)), &[], 1), "a scope that went is recorded at once");
    assert!(scoped_due(Some((&last, 0)), &[at("alpha", 10_100)], 1));
    assert!(!scoped_due(Some((&last, 0)), &[at("alpha", 10_050)], 1));
    assert!(scoped_due(Some((&last, 0)), &[at("alpha", 10_050)], MINUTE));
    assert!(!scoped_due(Some((&last, 0)), &[at("alpha", 10_000)], MINUTE));
}

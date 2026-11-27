// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the console must not do, and how its routes answer.

use std::path::{Path, PathBuf};

use bijective_control_engine::wire::{ControlEvent, RefusalReason, encode};
use bijective_event_store::store::StreamWriter;
use bijective_model::{OrderRequest, Side, VenueSymbol};

use super::*;

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-console-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn write(dir: &Path, events: &[ControlEvent]) {
    let mut w = StreamWriter::open(dir, "control").expect("open");
    for e in events {
        w.append(&encode(e).expect("encode")).expect("append");
    }
}

fn intent(instrument: &str) -> ControlEvent {
    ControlEvent::Intent {
        decision_id: "d-1".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: "bl-1".into(),
            instrument: VenueSymbol::new(instrument),
            side: Side::Buy,
            quantity_scaled: 1_000,
            price_scaled: Some(200),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 10,
    }
}

/// One refresh of a new console: what a single fold over the sources shows.
fn folded(dir: &Path, archive: Option<&Path>, standing: Standing) -> Console {
    let mut console = Console::default();
    console.refresh(dir, &Streams::default(), archive, standing);
    console
}

fn fold(dir: &Path) -> Console {
    folded(dir, None, Standing::NotConfigured)
}

fn found(console: &Console, route: &str) -> String {
    match console.page(route) {
        Page::Found(body) => body,
        Page::NotFound(_) => panic!("{route} is not a route"),
    }
}

/// Venue text must not reach the page as markup, on any route.
#[test]
fn text_from_a_venue_cannot_write_the_page() {
    let dir = root("escape");
    write(&dir, &[intent("<script>alert(1)</script>")]);
    let rendered = found(&fold(&dir), "/orders");
    assert!(!rendered.contains("<script>"), "a venue symbol reached the page as markup");
    assert!(rendered.contains("&lt;script&gt;"), "and it was not shown at all either");
}

#[test]
fn an_unreadable_entry_is_listed_and_raised_on_the_strip() {
    let dir = root("unreadable");
    let mut w = StreamWriter::open(&dir, "control").expect("open");
    w.append(b"not an event this build can read").expect("append");
    let console = fold(&dir);
    assert!(found(&console, "/data").contains("position 0"));
    let strip = console.signals();
    assert!(strip.iter().any(|s| s.label == "unreadable entries" && s.value == "1"), "{strip:?}");
    assert!(found(&console, "/").contains("unreadable entries: 1"), "not on every page");
}

/// Every group has a route. Anything else is answered as not found, with the navigation, and
/// reaches nothing.
#[test]
fn each_group_has_a_route_and_nothing_else_is_one() {
    let console = fold(&root("routes"));
    for route in ["/", "/market", "/data", "/venues", "/orders", "/risk", "/audit", "/orders?x=1"] {
        assert!(matches!(console.page(route), Page::Found(_)), "{route}");
    }
    for route in ["/../etc/passwd", "/orders/1", "/admin"] {
        match console.page(route) {
            Page::NotFound(body) => assert!(body.contains("href=\"/orders\"")),
            Page::Found(_) => panic!("{route} answered a page"),
        }
    }
}

#[test]
fn the_page_loads_nothing_and_runs_nothing() {
    let dir = root("closed");
    write(&dir, &[intent("aaa")]);
    let console = fold(&dir);
    for route in ["/", "/orders", "/risk"] {
        let page = found(&console, route);
        assert!(!page.contains("<script"), "{route}");
        assert!(!page.contains("http://") && !page.contains("https://"), "{route}");
    }
}

#[test]
fn an_unconfigured_standing_says_it_cannot_tell() {
    let console = fold(&root("standing"));
    assert!(found(&console, "/").contains("cannot say whether the node is up"));
    let strip = console.signals();
    assert!(strip.iter().any(|s| s.label == "node" && s.value == "not configured"), "{strip:?}");
}

#[test]
fn an_unreadable_manifest_is_not_a_node_that_is_down() {
    let dir = root("manifest");
    let standing = Standing::read(&dir.join("no-such-manifest.json"), &dir, 1_000);
    assert!(matches!(standing, Standing::ManifestUnreadable { .. }), "{standing:?}");
    let page = found(&folded(&dir, None, standing), "/");
    assert!(page.contains("manifest could not be read"));
    assert!(page.contains("not the same as the node being down"), "one read as the other");
}

#[test]
fn a_refusal_carries_its_reason() {
    let dir = root("refusal");
    write(
        &dir,
        &[ControlEvent::Refused {
            decision_id: "d-9".into(),
            reason: RefusalReason::Unreadable,
            from_position: 0,
            at_ns: 30,
        }],
    );
    let page = found(&fold(&dir), "/orders");
    assert!(page.contains("1 refused") && page.contains("Unreadable"));
}

/// A market stream is read from its tail: the oldest of a long stream never reaches the panel.
#[test]
fn a_market_stream_is_read_from_its_newest_entries_only() {
    use bijective_serialization::signal::market::{BookUpdate, encode as book};
    let dir = root("market-tail");
    let mut w = StreamWriter::open(&dir, "market-venue-a").expect("open");
    for n in 0..(MARKET_TAIL + 50) {
        let instrument = if n < 50 { "old-only" } else { "btc-perp" };
        let update = BookUpdate {
            instrument: bijective_model::InstrumentId::new(instrument),
            seq: bijective_model::Sequence::NotNumbered,
            venue_ts_ms: n as u64,
            received_at_ns: None,
            bid_price: 100,
            bid_quantity: 1,
            ask_price: 101,
            ask_quantity: 1,
            bid_depth: 1,
            ask_depth: 1,
        };
        w.append(&book(&update).expect("encode")).expect("append");
    }
    drop(w);
    let page = found(&fold(&dir), "/market");
    assert!(page.contains("btc-perp"));
    assert!(!page.contains("old-only"), "the history before the tail was read");
}

fn archived(root: &Path, venue: &str, facts: &[bijective_model::market::MarketFact]) {
    use bijective_archive::{ArchiveConfig, ArchiveWriter};
    use bijective_model::market::{ExchangeTime, MarketRecord};
    const AT: i128 = 1_793_664_000_000_000_000;
    let config = ArchiveConfig {
        root: root.to_path_buf(),
        venue: venue.into(),
        price_scale: 2,
        quantity_scale: 3,
    };
    let (mut w, _) = ArchiveWriter::open(config, AT).expect("open");
    for (n, fact) in facts.iter().enumerate() {
        let record = MarketRecord {
            instrument: VenueSymbol::new("BTCUSDT"),
            exchange_time: ExchangeTime::NotPublished,
            received_at_ns: AT + n as i128,
            fact: fact.clone(),
        };
        w.append_record(&record, n as u64 + 1, 0).expect("append");
    }
    w.close(AT + 60_000_000_000).expect("close");
}

/// The market page reads the archive at the scales its files declare, and the newest trade wins.
#[test]
fn the_market_page_shows_the_archived_facts_at_their_declared_scales() {
    use bijective_model::market::{
        Aggressor, ExchangeTime, Funding, Liquidation, MarketFact, ScaledDecimal, Trade,
    };
    let (dir, archive) = (root("archive-store"), root("archive"));
    let trade = |id: &str, price_scaled| {
        MarketFact::Trade(Trade {
            trade_id: id.into(),
            aggressor: Aggressor::Buyer,
            price_scaled,
            quantity_scaled: 1_500,
        })
    };
    archived(
        &archive,
        "venue-a",
        &[
            trade("t-1", 6_600_000),
            trade("t-2", 6_612_345),
            MarketFact::Funding(Funding {
                rate: ScaledDecimal { value_scaled: -5_671, scale: 8 },
                next_funding_time: ExchangeTime::NotPublished,
            }),
            MarketFact::Liquidation(Liquidation {
                forced_order_side: Side::Sell,
                price_scaled: 6_500_000,
                quantity_scaled: 2_000,
            }),
        ],
    );
    let console = folded(&dir, Some(&archive), Standing::NotConfigured);
    let page = found(&console, "/market");
    assert!(page.contains("<td>66123.45<td>1.500<td>buyer<td>2<td>"), "{page}");
    assert!(page.contains("-0.00005671"), "the funding rate as the venue wrote it");
    assert!(page.contains("<td>sell<td>65000.00<td>2.000</tr>"), "the liquidation is listed");
}

#[test]
fn an_archive_that_cannot_be_read_is_listed_not_left_out() {
    let (dir, archive) = (root("bad-archive-store"), root("bad-archive"));
    std::fs::create_dir_all(archive.join("venue-a")).expect("venue");
    std::fs::write(archive.join("venue-a").join("manifest.jsonl"), "not a manifest line\n")
        .expect("manifest");
    let console = folded(&dir, Some(&archive), Standing::NotConfigured);
    assert!(found(&console, "/data").contains("archive"));
    assert!(console.signals().iter().any(|s| s.label == "unreadable entries"));
}

fn every_page(console: &Console) -> Vec<String> {
    ["/", "/market", "/data", "/venues", "/orders", "/risk", "/audit"]
        .iter()
        .map(|route| found(console, route))
        .collect()
}

/// The contract of a refresh: what one fold over the same streams would show, on every page.
#[test]
fn a_refresh_shows_what_a_fresh_fold_shows() {
    let dir = root("refresh");
    let mut w = StreamWriter::open(&dir, "control").expect("open");
    w.append(&encode(&intent("btc-perp")).expect("encode")).expect("append");
    let mut console = fold(&dir);
    for instrument in ["eth-perp", "sol-perp"] {
        w.append(&encode(&intent(instrument)).expect("encode")).expect("append");
    }
    w.append(b"not an event this build can read").expect("append");
    console.refresh(&dir, &Streams::default(), None, Standing::NotConfigured);
    assert_eq!(every_page(&console), every_page(&fold(&dir)));
    assert!(found(&console, "/orders").contains("sol-perp"));

    console.refresh(&dir, &Streams::default(), None, Standing::NotConfigured);
    assert_eq!(every_page(&console), every_page(&fold(&dir)), "a quiet refresh changes nothing");
}

/// A store replaced under a running console must not leave the old history on the page.
#[test]
fn a_stream_rewritten_under_the_console_starts_the_fold_over() {
    let dir = root("rewritten");
    write(&dir, &[intent("old-a"), intent("old-b")]);
    let mut console = fold(&dir);
    std::fs::remove_dir_all(dir.join("control")).expect("replace");
    write(&dir, &[intent("new-a")]);
    let stream = dir.join("control").join("000000000000.seg");
    let held = std::fs::read(&stream).expect("segment");
    assert!(held.len() < 400, "the replacement is shorter than the cursor into the original");
    console.refresh(&dir, &Streams::default(), None, Standing::NotConfigured);
    let page = found(&console, "/orders");
    assert!(page.contains("new-a") && !page.contains("old-a"), "{page}");
}

#[test]
fn a_source_that_keeps_failing_is_listed_once() {
    let (dir, archive) = (root("failing-store"), root("failing-archive"));
    std::fs::create_dir_all(archive.join("venue-a")).expect("venue");
    std::fs::write(archive.join("venue-a").join("manifest.jsonl"), "not a manifest line\n")
        .expect("manifest");
    let mut console = Console::default();
    for _ in 0..3 {
        console.refresh(&dir, &Streams::default(), Some(&archive), Standing::NotConfigured);
    }
    let strip = console.signals();
    assert!(strip.iter().any(|s| s.label == "unreadable entries" && s.value == "1"), "{strip:?}");
}

/// The safe state is shown under Risk and beside the controls. Its figure is on the strip once.
#[test]
fn a_panel_in_two_groups_is_on_the_strip_once() {
    let strip = fold(&root("strip-once")).signals();
    assert_eq!(strip.iter().filter(|s| s.label == "trading").count(), 1, "{strip:?}");
}

fn settled(at_ns: i128) -> ControlEvent {
    let outcome = bijective_venue::normalise::VenueOutcome::Accepted;
    ControlEvent::Settled { client_id: "bl-1".into(), outcome, at_ns }
}

fn table(console: &Console, panel: &str) -> String {
    console.api(&format!("{API_PANELS}/{panel}")).unwrap_or_else(|| panic!("{panel} has no table"))
}

/// The page and the table come from one fold: what one shows, the other carries.
#[test]
fn the_page_and_the_table_show_one_fold() {
    let dir = root("table-page");
    let scales = ControlEvent::InstrumentScales {
        venue: "venue-a".into(),
        instrument: VenueSymbol::new("aaa"),
        price_scale: 2,
        quantity_scale: 3,
        at_ns: 9,
        identity: None,
        quote_currency: None,
    };
    write(&dir, &[scales, intent("aaa"), settled(30)]);
    let console = fold(&dir);
    let page = found(&console, "/orders");
    assert!(page.contains("bl-1") && page.contains("<td>1000<td>0<td>200<td>10<td>Accepted"));
    let orders = table(&console, "orders");
    let row = "[\"bl-1\",\"aaa\",\"Buy\",\"venue-a\",\"1.000\",\"0.000\",\"2.00\",\"10\",\"Accepted\",\"30\",false,null,null,null]";
    assert!(orders.contains(row), "the figures at the scales the record declared: {orders}");
}

/// A record written before scales were declared shows no figure rather than a guessed decimal.
#[test]
fn an_order_without_declared_scales_carries_no_figure() {
    let dir = root("table-unscaled");
    write(&dir, &[intent("aaa")]);
    let orders = table(&fold(&dir), "orders");
    assert!(orders.contains("\"venue-a\",null,null,null,\"10\""), "{orders}");
}

/// Replaying a store gives the same bytes, whether folded at once or a refresh at a time.
#[test]
fn a_replayed_store_gives_identical_tables() {
    let dir = root("table-replay");
    write(&dir, &[intent("aaa")]);
    let mut stepped = fold(&dir);
    write(&dir, &[settled(30)]);
    stepped.refresh(&dir, &Streams::default(), None, Standing::NotConfigured);
    let at_once = fold(&dir);
    assert_eq!(stepped.documents(), at_once.documents());
    assert_eq!(fold(&dir).documents(), at_once.documents());
}

#[test]
fn every_row_has_one_cell_per_column() {
    let dir = root("table-width");
    write(&dir, &[intent("aaa"), settled(30)]);
    let console = fold(&dir);
    for panel in console.groups.iter().flat_map(|g| g.panels.iter()) {
        let Some(rows) = panel.rows() else { continue };
        for row in &rows.rows {
            assert_eq!(row.len(), rows.columns.len(), "{}", panel.title());
        }
    }
}

#[test]
fn the_index_names_every_table_and_nothing_else_is_one() {
    let console = fold(&root("table-index"));
    let index = console.api(API_PANELS).expect("index");
    for name in ["strip", "orders", "held", "portfolio", "exposure-by-scope"] {
        assert!(index.contains(&format!("{{\"panel\":\"{name}\"")), "{name}: {index}");
        assert!(console.api(&format!("{API_PANELS}/{name}?x=1")).is_some(), "{name}");
    }
    for route in ["/api/panels/", "/api/panels/nope", "/api/panels/orders/x", "/api/orders"] {
        assert!(console.api(route).is_none(), "{route}");
    }
}

/// An event stream sends every table first, then only a table that changed.
#[test]
fn only_a_changed_table_is_sent_again() {
    let dir = root("table-changed");
    write(&dir, &[intent("aaa")]);
    let mut console = fold(&dir);
    let mut sent = BTreeMap::new();
    let first: Vec<String> = console.changed(&mut sent).into_iter().map(|(n, _)| n).collect();
    assert_eq!(first.len(), console.documents().len());
    assert!(console.changed(&mut sent).is_empty(), "nothing changed");
    write(&dir, &[settled(30)]);
    console.refresh(&dir, &Streams::default(), None, Standing::NotConfigured);
    let again: Vec<String> = console.changed(&mut sent).into_iter().map(|(n, _)| n).collect();
    // The answer completes the order and measures the venue's latency. Nothing else moved.
    assert_eq!(again, ["venue-latency", "orders"]);
}

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Configuration assurance tests, on what an operator declares to this engine.

use super::*;

/// Venue name comes from the build's registry: no component above the adapter contract names a venue.
fn venue() -> &'static str {
    bijective_venue_registry::REGISTERED[0]
}

fn good() -> String {
    let v = venue();
    format!(
        r#"{{
      "version": 1,
      "venues": ["{v}"],
      "freshness_bound_ns": "5000000000",
      "venue_access": [
        {{
          "name": "{v}",
          "endpoint": "https://api.example.invalid",
          "profile": "/etc/bijective/venues/venue.json",
          "key_name": "venue-api-key",
          "secret_name": "venue-api-secret",
          "wallet_key_name": "venue-wallet-key",
          "passphrase_name": "venue-api-passphrase",
          "account_address": "0x0000000000000000000000000000000000000001",
          "settle_coin": "USDT",
          "request_timeout_ns": "5000000000",
          "order_rate": {{ "orders": 10, "per_ns": "1000000000" }}
        }}
      ],
      "desks": [
        {{
          "name": "alpha",
          "instruments": ["btc-perp"],
          "venues": ["{v}"],
          "maximum_per_order_scaled": "1000000000"
        }}
      ],
      "limits": [
        {{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" }},
        {{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }}
      ]
    }}"#
    )
}

fn accept(raw: &str) -> Result<Accepted, Invalid> {
    Configuration::parse(raw.as_bytes())?.accept()
}

/// Replace a piece of the fixture, refusing a no-op substitution that would make a negative test pass vacuously.
fn instead_of(from: &str, to: &str) -> String {
    let changed = good().replace(from, to);
    assert_ne!(changed, good(), "the fixture does not contain {from}");
    changed
}

#[test]
fn a_declared_configuration_becomes_mandates_and_limits() {
    let accepted = accept(&good()).expect("valid");

    assert_eq!(accepted.venues(), [venue().to_string()]);
    assert_eq!(accepted.access().len(), 1);
    assert_eq!(accepted.freshness_bound_ns(), 5_000_000_000);
    assert_eq!(accepted.limits().len(), 2);
    let mandate = accepted.mandates().get("alpha").expect("alpha is declared");
    assert_eq!(mandate.venue_preference, [venue().to_string()]);
    assert_eq!(mandate.maximum_per_order, Amount::from_scaled(1_000_000_000));
}

/// An unset safety setting is refused, never chosen for the operator.
#[test]
fn an_absent_freshness_bound_refuses_the_configuration() {
    let without = instead_of("\"freshness_bound_ns\": \"5000000000\",", "");

    let refusal = accept(&without).expect_err("unset");
    assert_eq!(refusal.reason, Reason::Absent);
    assert!(refusal.to_string().contains("freshness_bound_ns"));
}

/// A desk preferring an unreachable venue is refused, with the reachable ones listed.
#[test]
fn a_desk_preferring_an_unreachable_venue_is_refused_with_the_list() {
    let wrong = instead_of(
        &format!("\"venues\": [\"{}\"],\n          \"maximum", venue()),
        "\"venues\": [\"elsewhere\"],\n          \"maximum",
    );

    let said = accept(&wrong).expect_err("unreachable").to_string();
    assert!(said.contains("elsewhere"), "{said}");
    assert!(said.contains(venue()), "the refusal names what this node reaches: {said}");
}

/// A ratio kind given a money maximum is refused, and the refusal names the field it takes.
#[test]
fn a_ratio_limit_cannot_be_declared_in_money() {
    let ratio = instead_of(
        "{ \"scope\": \"portfolio\", \"kind\": \"gross_exposure\"",
        "{ \"scope\": \"portfolio\", \"kind\": \"leverage\"",
    );

    let said = accept(&ratio).expect_err("not drawable").to_string();
    assert!(said.contains("leverage"), "{said}");
    assert!(said.contains("maximum_ratio_scaled"), "the refusal names the ratio field: {said}");
}

/// A loss has a figure for the whole book only. Anywhere else the limit could never be judged.
#[test]
fn a_daily_loss_limit_is_declared_for_the_portfolio_and_nowhere_else() {
    let portfolio = instead_of(
        r#"{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        r#"{ "scope": "portfolio", "kind": "daily_loss", "maximum_money_scaled": "300000000" }"#,
    );
    let accepted = accept(&portfolio).expect("a portfolio daily loss is declarable");
    assert!(accepted.limits().iter().any(|(_, kind, _)| *kind == LimitKind::DailyLoss));

    let narrower = instead_of(
        r#"{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        r#"{ "scope": "instrument:btc-perp", "kind": "daily_loss", "maximum_money_scaled": "300000000" }"#,
    );
    let said = accept(&narrower).expect_err("no loss figure below the portfolio").to_string();
    assert!(said.contains("portfolio"), "{said}");
}

#[test]
fn a_non_positive_ceiling_is_refused() {
    let zero = instead_of(
        "\"maximum_per_order_scaled\": \"1000000000\"",
        "\"maximum_per_order_scaled\": \"0\"",
    );

    assert!(matches!(accept(&zero).expect_err("zero").reason, Reason::NotPositive { .. }));
}

/// A wide amount that lost precision on the way in would be a limit the operator did not write.
#[test]
fn a_scaled_amount_that_is_not_an_integer_is_refused_where_it_arrives() {
    let fractional = instead_of("\"5000000000\"", "\"5e9\"");

    let said = accept(&fractional).expect_err("not an integer").to_string();
    assert!(said.contains("could not be read"), "{said}");
}

#[test]
fn a_desk_declared_twice_is_refused() {
    let twice = instead_of(
        "\"desks\": [",
        &format!(
            "\"desks\": [ {{ \"name\": \"alpha\", \"instruments\": [\"eth-perp\"], \
             \"venues\": [\"{}\"], \"maximum_per_order_scaled\": \"1\" }},",
            venue()
        ),
    );

    assert!(matches!(accept(&twice).expect_err("twice").reason, Reason::Duplicate { .. }));
}

#[test]
fn one_limit_declared_twice_for_one_scope_is_refused() {
    let twice = instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"portfolio\"");

    assert!(matches!(accept(&twice).expect_err("twice").reason, Reason::Duplicate { .. }));
}

#[test]
fn a_desk_with_no_instruments_is_refused() {
    let empty = instead_of("\"instruments\": [\"btc-perp\"]", "\"instruments\": []");

    assert!(matches!(accept(&empty).expect_err("empty").reason, Reason::Empty));
}

#[test]
fn a_desk_with_no_venues_is_refused() {
    let empty = instead_of(
        &format!("\"venues\": [\"{}\"],\n          \"maximum", venue()),
        "\"venues\": [],\n          \"maximum",
    );

    assert!(matches!(accept(&empty).expect_err("empty").reason, Reason::Empty));
}

/// A file from another build is refused whole, never partly applied.
#[test]
fn a_version_this_build_does_not_know_is_refused() {
    let newer = instead_of("\"version\": 1", "\"version\": 2");

    let said = accept(&newer).expect_err("newer").to_string();
    assert!(said.contains("version"), "{said}");
}

/// Malformed text carries what the parser said, not a bare refusal.
#[test]
fn unreadable_text_carries_the_parsers_own_message() {
    let said = accept("{ \"version\": ").expect_err("truncated").to_string();

    assert!(said.contains("could not be read"), "{said}");
    assert!(
        said.len() > "configuration: could not be read: ".len() + 4,
        "a refusal that says only *no* is what this component exists to remove: {said}"
    );
}

/// A venue this build does not carry is refused, and the refusal names what it does.
#[test]
fn a_venue_this_build_does_not_carry_is_refused() {
    let unknown = instead_of(
        &format!("\"venues\": [\"{}\"],\n      \"freshness", venue()),
        "\"venues\": [\"elsewhere\"],\n      \"freshness",
    );

    let said = accept(&unknown).expect_err("not in this build").to_string();
    assert!(said.contains("elsewhere"), "{said}");
    assert!(said.contains(venue()), "{said}");
}

/// A venue with no way to reach it is refused here, not at the first proposal.
#[test]
fn a_venue_with_no_access_block_is_refused() {
    let without = instead_of("\"venue_access\": [", "\"venue_access\": [ ] , \"unused\": [");

    let refusal = accept(&without).expect_err("nothing can reach it");
    assert_eq!(refusal.reason, Reason::Absent);
    assert!(refusal.to_string().contains(venue()), "{refusal}");
}

/// An access block for a venue the node may not trade is refused.
#[test]
fn an_access_block_for_an_undeclared_venue_is_refused() {
    let broken = instead_of(
        &format!("\"name\": \"{}\",\n          \"endpoint\"", venue()),
        "\"name\": \"elsewhere\",\n          \"endpoint\"",
    );

    assert!(accept(&broken).is_err());
}

#[test]
fn a_scale_named_in_a_venue_access_is_refused_with_the_fix() {
    for field in ["price_scale", "quantity_scale"] {
        let named = instead_of(
            "\"settle_coin\": \"USDT\",",
            &format!("\"settle_coin\": \"USDT\", \"{field}\": 2,"),
        );
        let said = accept(&named).expect_err("a retired field").to_string();
        assert!(said.contains(field) && said.contains("decision 0007"), "{said}");
    }
}

#[test]
fn a_blank_endpoint_or_credential_name_is_refused() {
    let values = [
        ("endpoint", "https://api.example.invalid"),
        ("profile", "/etc/bijective/venues/venue.json"),
        ("key_name", "venue-api-key"),
        ("secret_name", "venue-api-secret"),
        ("settle_coin", "USDT"),
    ];

    for (field, value) in values {
        let blanked =
            instead_of(&format!("\"{field}\": \"{value}\""), &format!("\"{field}\": \"   \""));

        let refusal = accept(&blanked).expect_err("blank");
        assert_eq!(refusal.reason, Reason::Absent, "{field}");
        assert!(refusal.to_string().contains(field), "{field}: {refusal}");
    }
}

/// A request timeout of zero is refused: a call with no bound is one that can hang forever.
#[test]
fn a_zero_request_timeout_is_refused() {
    let zero =
        instead_of("\"request_timeout_ns\": \"5000000000\"", "\"request_timeout_ns\": \"0\"");

    assert!(matches!(accept(&zero).expect_err("zero").reason, Reason::NotPositive { .. }));
}

/// Coverage in the one form answerable without an entity inventory: a portfolio limit covers everything.
#[test]
fn a_limit_set_that_cannot_be_shown_to_cover_everything_is_refused() {
    let without = good().replace(
        r#"{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" },"#,
        "",
    );
    let parsed: Configuration = serde_json::from_str(&without).expect("still valid json");
    match parsed.accept() {
        Err(invalid) => {
            assert_eq!(invalid.field, "limits[portfolio]");
            assert!(matches!(invalid.reason, Reason::Absent));
        }
        Ok(_) => panic!("a node started with entities nothing can be shown to constrain"),
    }
}

/// The refusal says what to declare.
#[test]
fn the_refusal_names_the_declaration_that_would_answer_it() {
    let without = good().replace(
        r#"{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" },"#,
        "",
    );
    let parsed: Configuration = serde_json::from_str(&without).expect("still valid json");
    let said = parsed.accept().expect_err("accepted").to_string();
    assert!(said.contains("portfolio"), "the refusal does not name it: {said}");
}

/// A set with only a portfolio limit is complete, which is the point of the scope.
#[test]
fn a_portfolio_limit_alone_covers_everything() {
    let only = good().replace(
        r#",
        { "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        "",
    );
    assert_ne!(only, good(), "the fixture does not hold the instrument limit");
    let parsed: Configuration = serde_json::from_str(&only).expect("still valid json");
    assert!(parsed.accept().is_ok(), "a portfolio limit was not enough");
}

// desk and strategy limits

/// Measured from the platform's placement record, so a desk limit is a limit in force.
#[test]
fn a_desk_scoped_limit_is_accepted() {
    let desked = instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"desk:alpha\"");

    let accepted = accept(&desked).expect("a desk limit");
    assert!(accepted.limits().iter().any(|(s, _, _)| *s == Scope::Desk("alpha".into())));
}

/// A misspelt desk would read as a limit in force and bind nothing.
#[test]
fn a_limit_on_a_desk_nobody_declared_is_refused() {
    let misspelt = instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"desk:alpah\"");

    let invalid = accept(&misspelt).expect_err("a limit on no desk");
    assert!(
        invalid.to_string().contains("alpha"),
        "the refusal does not offer the desk: {invalid}"
    );
}

/// A strategy is named by the strategy engine, not declared here, so its name is taken as written.
#[test]
fn a_strategy_scoped_limit_is_accepted() {
    let by_strategy =
        instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"strategy:carry\"");

    let accepted = accept(&by_strategy).expect("a strategy limit");
    assert!(accepted.limits().iter().any(|(s, _, _)| *s == Scope::Strategy("carry".into())));
}

/// A desk limit of a kind covers what that desk trades, as an instrument limit would.
#[test]
fn a_desk_limit_covers_its_desk_for_its_kind() {
    let net = good().replace(
        r#"{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        r#"{ "scope": "desk:alpha", "kind": "net_exposure", "maximum_money_scaled": "2000000000" }"#,
    );
    assert_ne!(net, good(), "the fixture does not hold the instrument limit");
    let parsed: Configuration = serde_json::from_str(&net).expect("still valid json");
    assert!(parsed.accept().is_ok(), "a desk limit did not cover its desk");
}

/// Every level this build measures is accepted in a scope. None is refused for want of a figure.
#[test]
fn every_measured_level_is_a_scope_an_operator_may_declare() {
    let venue_scope = format!("venue:{}", venue());
    for level in bijective_risk::valuation::MEASURED_LEVELS {
        let spelt = match level {
            // The fixture already declares both.
            ScopeLevel::Portfolio | ScopeLevel::Instrument => continue,
            ScopeLevel::Desk => "desk:alpha",
            ScopeLevel::Venue => venue_scope.as_str(),
            ScopeLevel::Strategy => "strategy:carry",
            ScopeLevel::Underlying => "underlying:BTC",
        };
        let at_level =
            instead_of("\"scope\": \"instrument:btc-perp\"", &format!("\"scope\": \"{spelt}\""));
        assert!(accept(&at_level).is_ok(), "{spelt} was refused");
    }
}

/// Two venues one desk reaches, so a venue-scoped limit binds one of them and not the other.
fn two_venues() -> String {
    let a = bijective_venue_registry::REGISTERED[0];
    let b = bijective_venue_registry::REGISTERED[1];
    let access = |v: &str| {
        format!(
            r#"{{
          "name": "{v}",
          "endpoint": "https://api.example.invalid",
          "profile": "/etc/bijective/venues/venue.json",
          "key_name": "venue-api-key",
          "secret_name": "venue-api-secret",
          "wallet_key_name": "venue-wallet-key",
          "passphrase_name": "venue-api-passphrase",
          "account_address": "0x0000000000000000000000000000000000000001",
          "settle_coin": "USDT",
          "request_timeout_ns": "5000000000",
          "order_rate": {{ "orders": 10, "per_ns": "1000000000" }}
        }}"#
        )
    };
    format!(
        r#"{{
      "version": 1,
      "venues": ["{a}", "{b}"],
      "freshness_bound_ns": "5000000000",
      "venue_access": [{}, {}],
      "desks": [
        {{
          "name": "alpha",
          "instruments": ["btc-perp"],
          "venues": ["{a}", "{b}"],
          "maximum_per_order_scaled": "1000000000"
        }}
      ],
      "limits": [
        {{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" }},
        {{ "scope": "venue:{a}", "kind": "net_exposure", "maximum_money_scaled": "2000000000" }}
      ]
    }}"#,
        access(a),
        access(b)
    )
}

/// A kind declared for one venue only binds nothing on the other, and nothing says so.
#[test]
fn a_kind_that_binds_one_venue_and_not_another_is_refused() {
    let partial = two_venues();

    assert!(
        accept(&partial).is_err(),
        "a declared kind left part of the book unconstrained and the configuration was accepted"
    );
}

/// The scope hierarchy must still allow a stricter limit somewhere.
#[test]
fn a_narrower_limit_beside_a_book_level_one_of_the_same_kind_is_a_tightening() {
    let both = two_venues().replace(
        r#"{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" },"#,
        r#"{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" },
        { "scope": "portfolio", "kind": "net_exposure", "maximum_money_scaled": "9000000000" },"#,
    );
    assert_ne!(both, two_venues(), "the substitution changed nothing");

    assert!(
        accept(&both).is_ok(),
        "a venue limit beside a book-level one of the same kind was refused: {:?}",
        accept(&both).err()
    );
}

/// A refusal an operator cannot act on is a run refused twice.
#[test]
fn the_coverage_refusal_offers_the_declarations_that_would_close_it() {
    let said = accept(&two_venues()).expect_err("accepted").to_string();

    assert!(said.contains("net_exposure"), "the kind is not named: {said}");
    assert!(
        said.contains("portfolio/net_exposure"),
        "the book-level answer is not offered: {said}"
    );
    assert!(said.contains("instrument:btc-perp"), "the instrument answer is not offered: {said}");
}

#[test]
fn a_venue_without_a_positive_order_rate_is_refused() {
    let zero = instead_of("\"orders\": 10", "\"orders\": 0");
    assert!(matches!(accept(&zero).expect_err("zero").reason, Reason::NotPositive { .. }));
    let none = instead_of("\"order_rate\"", "\"order_rate_misspelt\"");
    assert!(accept(&none).is_err(), "an absent rate is refused, never defaulted");
}

/// Measured like the daily loss, from the day's result, so declarable for the whole book only.
#[test]
fn a_drawdown_limit_is_declared_in_money_for_the_portfolio_only() {
    let portfolio = instead_of(
        r#"{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        r#"{ "scope": "portfolio", "kind": "drawdown", "maximum_money_scaled": "150000000" }"#,
    );
    let accepted = accept(&portfolio).expect("a portfolio drawdown is declarable");
    assert!(accepted.limits().iter().any(|(_, kind, _)| *kind == LimitKind::Drawdown));

    let narrower = instead_of(
        r#"{ "scope": "instrument:btc-perp", "kind": "gross_exposure", "maximum_money_scaled": "2000000000" }"#,
        r#"{ "scope": "venue:venue-a", "kind": "drawdown", "maximum_money_scaled": "150000000" }"#,
    );
    let said = accept(&narrower).expect_err("no drawdown figure below the portfolio").to_string();
    assert!(said.contains("portfolio"), "{said}");
}

/// An asset is one asset whatever case it is written in, as its figures are keyed.
#[test]
fn an_underlying_limit_is_keyed_in_lower_case() {
    let asset = instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"underlying:BTC\"");

    let accepted = accept(&asset).expect("an underlying limit");
    assert!(accepted.limits().iter().any(|(s, _, _)| *s == Scope::Underlying("btc".into())));
    assert!(
        accept(&instead_of("\"scope\": \"instrument:btc-perp\"", "\"scope\": \"underlying:\""))
            .is_err()
    );
}

/// The sandbox account is declared per configured venue, every balance positive.
#[test]
fn a_sandbox_balance_is_for_a_configured_venue_and_positive() {
    let v = venue();
    let with = |block: &str| {
        good().replace(
            "\"freshness_bound_ns\": \"5000000000\",",
            &format!("\"freshness_bound_ns\": \"5000000000\", \"sandbox\": {block},"),
        )
    };
    let accepted =
        accept(&with(&format!(r#"{{ "{v}": {{ "usdt": "100000000000" }} }}"#))).expect("sandbox");
    assert_eq!(accepted.sandbox(v).and_then(|b| b.get("usdt")).copied(), Some(100_000_000_000));
    assert!(accept(&with(r#"{ "not-a-venue": { "usdt": "1" } }"#)).is_err());
    assert!(accept(&with(&format!(r#"{{ "{v}": {{ "usdt": "0" }} }}"#))).is_err());
    assert_eq!(accept(&good()).expect("none").sandbox(v), None, "absent is not a default");
}

/// Each registered venue asks for the names of its own credential.
#[test]
fn the_credential_names_required_follow_the_venues_credential_shape() {
    use bijective_venue_registry::{CredentialShape, REGISTERED, credential_shape};

    let values = [
        ("key_name", "venue-api-key"),
        ("secret_name", "venue-api-secret"),
        ("wallet_key_name", "venue-wallet-key"),
        ("passphrase_name", "venue-api-passphrase"),
        ("account_address", "0x0000000000000000000000000000000000000001"),
    ];
    for registered in REGISTERED {
        let config = good().replace(&format!("\"{}\"", venue()), &format!("\"{registered}\""));
        assert!(accept(&config).is_ok(), "{registered}: {:?}", accept(&config).err());
        let needed: &[&str] = match credential_shape(registered).expect("registered") {
            CredentialShape::KeyAndSecret => &["key_name", "secret_name"],
            CredentialShape::WalletKey => &["wallet_key_name", "account_address"],
            CredentialShape::WalletKeyAndApiKey => {
                &["wallet_key_name", "key_name", "secret_name", "passphrase_name"]
            }
        };
        for (name, value) in values {
            let blanked =
                config.replace(&format!("\"{name}\": \"{value}\""), &format!("\"{name}\": \" \""));
            let refused = accept(&blanked).err();
            assert_eq!(
                refused.is_some(),
                needed.contains(&name),
                "{registered} {name}: {refused:?}"
            );
        }
    }
}

fn with_valuation(block: &str) -> String {
    instead_of(r#""version": 1,"#, &format!(r#""version": 1, "valuation": {block},"#))
}

#[test]
fn a_configuration_without_a_valuation_keeps_one_settlement_currency() {
    assert_eq!(accept(&good()).expect("accepted").reporting(), None);
}

#[test]
fn a_reporting_currency_and_its_rates_are_read_in_lower_case() {
    let raw = with_valuation(
        r#"{ "currency": "USD",
             "rates": { "USDT": { "instrument": "venue.usdtusd", "max_age_ns": "300000000000" } },
             "markets": [{ "name": "venue-rates", "profile": "/p.json" }] }"#,
    );

    let accepted = accept(&raw).expect("accepted");

    let reporting = accepted.reporting().expect("declared");
    assert_eq!(reporting.currency, "usd");
    let usdt = reporting.rates.get("usdt").expect("rate");
    assert_eq!((usdt.instrument.as_str(), usdt.max_age_ns), ("venue.usdtusd", 300_000_000_000));
    assert_eq!(reporting.markets.len(), 1);
}

#[test]
fn a_rate_declared_twice_in_two_cases_is_refused() {
    let raw = with_valuation(
        r#"{ "currency": "usd", "rates": {
             "usdt": { "instrument": "venue.a", "max_age_ns": "1" },
             "USDT": { "instrument": "venue.b", "max_age_ns": "1" } } }"#,
    );

    let refused = accept(&raw).expect_err("refused");

    assert_eq!(refused.field, "valuation.rates");
}

#[test]
fn an_unnamed_reporting_currency_or_rate_market_is_refused() {
    for block in [
        r#"{ "currency": " " }"#,
        r#"{ "currency": "usd", "rates": { "usdt": { "instrument": "", "max_age_ns": "1" } } }"#,
        r#"{ "currency": "usd", "rates": { "usdt": { "instrument": "venue.a" } } }"#,
        r#"{ "currency": "usd", "rates": { "usdt": { "instrument": "a", "max_age_ns": "0" } } }"#,
        r#"{ "currency": "usd", "markets": [{ "name": "", "profile": "/p.json" }] }"#,
    ] {
        let refused = accept(&with_valuation(block)).expect_err("refused");
        assert!(refused.field.starts_with("valuation."), "{refused:?}");
    }
}

fn with_assets(block: &str) -> String {
    instead_of(r#""version": 1,"#, &format!(r#""version": 1, "assets": {block},"#))
}

/// One asset under three venue names, as spot, a linear contract of a thousand and a perpetual of a
/// thousand are written on real venues (`docs/decisions/0006-asset-names.md`).
#[test]
fn venue_base_names_are_read_as_the_asset_they_denote() {
    let accepted = accept(&with_assets(r#"{ "PEPE": ["1000PEPE", "kPEPE"] }"#)).expect("accepted");

    let assets = accepted.assets();
    assert_eq!(assets.canonical("kPEPE"), "pepe");
    assert_eq!(assets.canonical("1000pepe"), "pepe");
    assert_eq!(assets.canonical("PEPE"), "pepe");
    assert_eq!(assets.canonical("ETH"), "eth", "an unlisted name is its own asset");
}

#[test]
fn a_name_that_denotes_two_assets_is_refused() {
    for block in [
        r#"{ "pepe": ["kpepe"], "kpepe2": ["KPEPE"] }"#,
        r#"{ "pepe": ["btc"], "btc": ["xbt"] }"#,
        r#"{ "pepe": [""] }"#,
    ] {
        let refused = accept(&with_assets(block)).expect_err("refused");
        assert_eq!(refused.field, "assets", "{block}");
    }
}

/// An account at a registered venue may carry another name. An unregistered venue is refused under
/// the field that names it (decision 0008).
#[test]
fn an_account_is_named_apart_from_its_venue() {
    let good_text = good();
    let venue = accept(&good_text).expect("the fixture").access()[0].name.clone();
    let renamed = good_text
        .replace(
            &format!("\"name\": \"{venue}\""),
            &format!("\"name\": \"main\", \"venue\": \"{venue}\""),
        )
        .replace(&format!("\"{venue}\""), "\"main\"")
        .replace("\"venue\": \"main\"", &format!("\"venue\": \"{venue}\""));
    let accepted = accept(&renamed).expect("an account named apart from its venue");
    assert_eq!(accepted.access()[0].name, "main");
    assert_eq!(accepted.access()[0].dialect(), venue);

    let unregistered =
        renamed.replace(&format!("\"venue\": \"{venue}\""), "\"venue\": \"nowhere\"");
    let said = accept(&unregistered).expect_err("no such venue").to_string();
    assert!(said.contains("nowhere"), "{said}");
}

#[test]
fn a_desk_collar_of_zero_or_beyond_the_widest_is_refused() {
    let from = "\"maximum_per_order_scaled\": \"1000000000\"";
    for bps in ["0", "10001"] {
        let raw = instead_of(from, &format!("{from}, \"price_collar_bps\": {bps}"));
        let refused = accept(&raw).expect_err("refused");
        assert!(refused.field.ends_with("price_collar_bps"), "{refused:?}");
        assert!(matches!(refused.reason, Reason::OutOfRange { .. }));
    }
}

/// A ratio kind is declared in its own unit, for the whole book only.
#[test]
fn leverage_and_margin_use_are_declared_as_ratios_for_the_portfolio() {
    let anchor = r#"{ "scope": "portfolio", "kind": "gross_exposure", "maximum_money_scaled": "5000000000" },"#;
    let ratios = format!(
        r#"{anchor}
        {{ "scope": "portfolio", "kind": "leverage", "maximum_ratio_scaled": "30000" }},
        {{ "scope": "portfolio", "kind": "margin_use", "maximum_ratio_scaled": "5000" }},"#
    );
    let accepted = accept(&instead_of(anchor, &ratios)).expect("accepted");
    let leverage = accepted
        .limits()
        .iter()
        .find(|(_, kind, _)| *kind == LimitKind::Leverage)
        .map(|(_, _, maximum)| *maximum);
    assert_eq!(leverage, Some(Threshold::Ratio(bijective_limits::Ratio::from_scaled(30_000))));

    let in_money = format!(
        r#"{anchor} {{ "scope": "portfolio", "kind": "leverage", "maximum_money_scaled": "3" }},"#
    );
    let refused = accept(&instead_of(anchor, &in_money)).expect_err("money for a ratio");
    assert!(refused.field.ends_with("maximum_money_scaled"), "{refused:?}");

    let on_a_desk = format!(
        r#"{anchor} {{ "scope": "desk:alpha", "kind": "leverage", "maximum_ratio_scaled": "30000" }},"#
    );
    assert!(accept(&instead_of(anchor, &on_a_desk)).is_err(), "measured for the whole book only");
}

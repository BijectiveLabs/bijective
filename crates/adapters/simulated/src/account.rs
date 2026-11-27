// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The sandbox account: cash per currency that every fill moves and every buy must be able to
//! pay for, with the venue's declared fees charged on each fill
//! (`docs/decisions/0003-sandbox.md`).
//!
//! Money is held at eight decimals. A cost is rounded up and a proceed down, so the account never
//! holds more than the fills could have left it.

use std::collections::BTreeMap;

use bijective_lifecycle::{Fee, Liquidity};
use bijective_model::Side;

/// Money is carried at this many decimals, the platform's scale.
pub const MONEY_SCALE: u8 = 8;

/// Basis points at the platform's eight-decimal scale, as the venue's fee declaration states them.
/// Absent is not zero: a venue with no declaration charges nothing and the model says so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeeRates {
    pub taker_bps_scaled: Option<i128>,
    pub maker_bps_scaled: Option<i128>,
}

/// What an order is priced in, taken from the venue's catalogue when it is submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terms {
    pub currency: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    cash: BTreeMap<String, i128>,
    /// What each open buy may still cost, so two buys cannot both spend one balance.
    reserved: BTreeMap<String, (String, i128)>,
    fees: FeeRates,
}

impl Account {
    /// `cash` per currency at [`MONEY_SCALE`].
    #[must_use]
    pub fn new(cash: BTreeMap<String, i128>, fees: FeeRates) -> Self {
        Self { cash, reserved: BTreeMap::new(), fees }
    }

    #[must_use]
    pub fn cash(&self, currency: &str) -> i128 {
        self.cash.get(currency).copied().unwrap_or(0)
    }

    fn available(&self, currency: &str) -> i128 {
        let held: i128 =
            self.reserved.values().filter(|(c, _)| c == currency).map(|(_, a)| *a).sum();
        self.cash(currency).saturating_sub(held)
    }

    /// Reserves what a buy of `quantity` at `price` may cost with the taker fee, or `false` when
    /// the account cannot pay it. A sell reserves nothing: what it sells is checked on the record.
    pub fn reserve(
        &mut self,
        client_id: &str,
        terms: &Terms,
        side: Side,
        (price, quantity): (i128, i128),
    ) -> bool {
        if side == Side::Sell {
            return true;
        }
        let Some(cost) = value(price, quantity, terms, true) else { return false };
        let cost = cost.saturating_add(self.fee(cost, Liquidity::Taker).unwrap_or(0));
        if cost > self.available(&terms.currency) {
            return false;
        }
        self.reserved.insert(client_id.to_string(), (terms.currency.clone(), cost));
        true
    }

    /// The order can spend nothing more: whatever it reserved is released.
    pub fn release(&mut self, client_id: &str) {
        self.reserved.remove(client_id);
    }

    /// Moves cash for one fill and returns the fee charged, if the venue declares one.
    pub fn fill(
        &mut self,
        client_id: &str,
        terms: &Terms,
        side: Side,
        (price, quantity, liquidity): (i128, i128, Liquidity),
    ) -> Option<Fee> {
        let notional = value(price, quantity, terms, side == Side::Buy).unwrap_or(i128::MAX);
        let fee = self.fee(notional, liquidity);
        let charged = fee.unwrap_or(0);
        let cash = self.cash.entry(terms.currency.clone()).or_default();
        *cash = match side {
            Side::Buy => cash.saturating_sub(notional.saturating_add(charged)),
            Side::Sell => cash.saturating_add(notional).saturating_sub(charged),
        };
        if let Some((_, held)) = self.reserved.get_mut(client_id) {
            *held = held.saturating_sub(notional.saturating_add(charged)).max(0);
        }
        fee.map(|amount| Fee {
            asset: terms.currency.clone(),
            amount_scaled: amount,
            scale: MONEY_SCALE,
        })
    }

    /// The fee on `notional` at the rate for `liquidity`, rounded up. `None` when undeclared.
    fn fee(&self, notional: i128, liquidity: Liquidity) -> Option<i128> {
        let rate = match liquidity {
            Liquidity::Maker => self.fees.maker_bps_scaled,
            // Not published is charged as taking, the larger of the two at every venue declared.
            Liquidity::Taker | Liquidity::NotPublished => self.fees.taker_bps_scaled,
        }?;
        // Basis points at eight decimals: one part in 10^12 of the notional per unit of rate.
        let parts = notional.checked_mul(rate)?;
        Some(div_round(parts, 1_000_000_000_000, parts > 0))
    }
}

/// Price times quantity at [`MONEY_SCALE`]. Rounded up for a cost, down for a proceed.
fn value(price: i128, quantity: i128, terms: &Terms, up: bool) -> Option<i128> {
    let raw = price.checked_mul(quantity)?;
    let from = u32::from(terms.price_scale) + u32::from(terms.quantity_scale);
    let to = u32::from(MONEY_SCALE);
    if from <= to {
        return raw.checked_mul(10i128.checked_pow(to - from)?);
    }
    Some(div_round(raw, 10i128.checked_pow(from - to)?, up))
}

fn div_round(value: i128, by: i128, up: bool) -> i128 {
    let whole = value / by;
    if up && value % by != 0 { whole + 1 } else { whole }
}

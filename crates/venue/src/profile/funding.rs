// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The funding model, and why an absent interval cannot be encoded without choosing one.

use serde::{Deserialize, Serialize};

/// Part of the model: the same venue reports differently by wire, so a rate must carry its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Stream,
    Request,
}

/// Venue assurance: an absent interval has five meanings. The variants are those five, so absent is
/// not representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "kebab-case")]
pub enum FundingModel {
    /// No interval exists. The venue funds continuously. An interval here would be a fiction.
    Continuous,
    Published {
        #[serde(with = "crate::profile::wide")]
        interval_ns: i128,
    },
    /// Deliberately carries no interval: inferring one is what this variant prevents.
    UnpublishedOnThisTransport,
    /// Documented as constant. Re-verified on every venue change since the exchange sets its
    /// expiry.
    FixedAndKnown {
        #[serde(with = "crate::profile::wide")]
        interval_ns: i128,
    },
    /// Derived by the adapter from another venue field. Our inference, not the venue's statement.
    AdapterComputed {
        #[serde(with = "crate::profile::wide")]
        interval_ns: i128,
    },
}

impl FundingModel {
    /// `None` means opposite things for `Continuous` and `UnpublishedOnThisTransport`. Match
    /// on the model.
    pub fn interval_ns(self) -> Option<i128> {
        match self {
            FundingModel::Published { interval_ns }
            | FundingModel::FixedAndKnown { interval_ns }
            | FundingModel::AdapterComputed { interval_ns } => Some(interval_ns),
            FundingModel::Continuous | FundingModel::UnpublishedOnThisTransport => None,
        }
    }

    pub fn is_derived(self) -> bool {
        matches!(self, FundingModel::AdapterComputed { .. })
    }
}

/// No constructor without a transport: a cost from request data would silently lack
/// a period the live feed supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingFigure {
    #[serde(with = "crate::profile::wide")]
    rate_scaled: i128,
    transport: Transport,
    model: FundingModel,
}

impl FundingFigure {
    pub fn new(rate_scaled: i128, transport: Transport, model: FundingModel) -> Self {
        Self { rate_scaled, transport, model }
    }

    pub fn rate_scaled(&self) -> i128 {
        self.rate_scaled
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn model(&self) -> FundingModel {
        self.model
    }

    /// The model records the schedule, never that a payment was applied.
    pub fn implies_payment(&self) -> bool {
        false
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The session gate (decision 0004, rule 6): a new entry is sent only while the venue's clock
//! states the core session. Read from `GET /v3/clock?markets=NYSE` (Market clock, read
//! 2026-11-17), whose `phase` is `closed`, `pre`, `core`, `lunch` or `post`, held until its
//! `phase_until`. An exit is never held by it.

use bijective_venue::resilience::CallClass;
use reqwest::Method;
use serde_json::Value;

use super::RestClient;
use crate::evidence::utc_ns;

const PATH_CLOCK: &str = "/v3/clock";

/// The phase is read again at least this often, whatever `phase_until` says, so a clock answer
/// that was wrong is not held for hours.
const MAX_HOLD_NS: i128 = 60_000_000_000;

/// The phase the venue stated and until when it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Phase {
    pub(super) name: String,
    pub(super) holds_until_ns: i128,
}

/// The first clock in a `/v3/clock` answer.
pub(super) fn read_phase(body: &str, now_ns: i128) -> Option<Phase> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let clock = parsed.get("clocks")?.as_array()?.first()?;
    let name = clock.get("phase")?.as_str()?.to_string();
    let holds_until_ns = clock.get("phase_until").and_then(Value::as_str).and_then(utc_ns)?;
    Some(Phase { name, holds_until_ns: holds_until_ns.min(now_ns + MAX_HOLD_NS) })
}

impl RestClient {
    /// `Err` names why an entry is not sent: outside the core session, or the clock could not
    /// be read, which holds entries back as well (fail closed).
    pub(super) async fn core_session(&mut self) -> Result<(), String> {
        let now = self.clock.now_ns();
        if let Some(phase) = self.phase.as_ref().filter(|p| now < p.holds_until_ns) {
            return in_core(phase);
        }
        let reply = self
            .call(CallClass::Read, Method::GET, PATH_CLOCK, "markets=NYSE", "")
            .await
            .map_err(|held| format!("the market clock read was held back: {held}"))?;
        if reply.status != 200 {
            return Err(format!("the market clock answered {}", reply.status));
        }
        let phase = read_phase(&reply.body, now)
            .ok_or_else(|| "the market clock answer could not be read".to_string())?;
        let verdict = in_core(&phase);
        self.phase = Some(phase);
        verdict
    }
}

fn in_core(phase: &Phase) -> Result<(), String> {
    if phase.name == "core" {
        Ok(())
    } else {
        Err(format!("outside the core session: the market is {}", phase.name))
    }
}

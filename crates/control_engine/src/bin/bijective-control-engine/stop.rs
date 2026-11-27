// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stop request. A terminate or interrupt signal ends a paper or live run between passes,
//! never inside one, so a pass that recorded an intent also records what the venue answered.

use std::time::Duration;

/// Registered before the first pass, so a signal sent once the node reads as ready is held until
/// the next wait instead of ending the process by the signal's default action.
pub(crate) struct Stop {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl Stop {
    /// Precondition: called inside the runtime. Error contract: a handler that cannot be
    /// registered is returned, and the caller runs with the signals' default action.
    #[cfg(unix)]
    pub(crate) fn register() -> Result<Self, std::io::Error> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn register() -> Result<Self, std::io::Error> {
        Ok(Self {})
    }

    /// Waits `heartbeat`, and answers whether a stop was requested before or during the wait.
    /// A signal stream that ends is not a request: the wait then runs its full length.
    #[cfg(unix)]
    pub(crate) async fn requested_within(&mut self, heartbeat: Duration) -> bool {
        tokio::select! {
            Some(()) = self.terminate.recv() => true,
            Some(()) = self.interrupt.recv() => true,
            () = tokio::time::sleep(heartbeat) => false,
        }
    }

    #[cfg(not(unix))]
    pub(crate) async fn requested_within(&mut self, heartbeat: Duration) -> bool {
        tokio::select! {
            Ok(()) = tokio::signal::ctrl_c() => true,
            () = tokio::time::sleep(heartbeat) => false,
        }
    }
}

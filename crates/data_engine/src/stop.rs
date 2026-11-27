// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The stop request: one listener per program, installed before the first readiness report and
//! latched once a signal arrives.
//!
//! Installed early, so a stop sent the moment the program reads as ready is caught instead of
//! ending the process by the signal's default action. Latched, so a signal that arrives while no
//! loop is waiting on it is still seen at the next wait.

use std::sync::OnceLock;

use tokio::sync::watch;

static REQUESTED: OnceLock<watch::Receiver<bool>> = OnceLock::new();

/// Precondition: called once, inside the runtime, before anything reports `ready`.
/// Error contract: a handler that cannot be registered is reported and the listener never
/// resolves, so a failed registration cannot read as a stop request.
pub(crate) fn install() -> Result<(), std::io::Error> {
    let (sender, receiver) = watch::channel(false);
    let _ = REQUESTED.set(receiver);
    let signals = Signals::register()?;
    tokio::spawn(async move {
        signals.first().await;
        let _ = sender.send(true);
        // Held open: a closed channel would read as a request to every later waiter.
        sender.closed().await;
    });
    Ok(())
}

/// Resolves once a stop has been requested, immediately when one already was.
pub(crate) async fn requested() {
    let Some(receiver) = REQUESTED.get() else {
        return std::future::pending().await;
    };
    let mut receiver = receiver.clone();
    if receiver.wait_for(|requested| *requested).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(unix)]
struct Signals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    /// Registered here, not on first poll, so the handlers exist when `install` returns.
    fn register() -> Result<Self, std::io::Error> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    async fn first(mut self) {
        // A stream that ends without a signal is not a request.
        tokio::select! {
            Some(()) = self.terminate.recv() => {}
            Some(()) = self.interrupt.recv() => {}
            else => std::future::pending().await,
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn register() -> Result<Self, std::io::Error> {
        Ok(Self)
    }

    async fn first(self) {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/stop_tests.rs"]
mod tests;

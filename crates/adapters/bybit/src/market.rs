// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Market-data side of the adapter contract over the streaming path. Design notes: `docs/integrations/bybit.md`.

use std::time::Duration;

use bijective_venue::contract::{
    Answer, ConnectError, ConnectionState, Continuity, MarketEvent, Reconnected, StreamRequest,
    SubscribeError, SubscriptionAck, VenueData, VenueSymbol,
};
use bijective_venue::profile::{CapabilityDeclaration, DataClass};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Bounded so a silent venue yields an unconfirmed answer rather than a hung program.
const CONFIRM_WITHIN: Duration = Duration::from_secs(15);

pub struct MarketData {
    request: StreamRequest,
    declaration: CapabilityDeclaration,
    state: ConnectionState,
    events: Option<mpsc::Receiver<MarketEvent>>,
    driver: Option<JoinHandle<()>>,
    /// Gap flag read once by `reconnect`. Held because the state moves on before the caller asks.
    continuity_broken: bool,
}

impl MarketData {
    pub fn new(request: StreamRequest, declaration: CapabilityDeclaration) -> Self {
        Self {
            request,
            declaration,
            state: ConnectionState::Disconnected,
            events: None,
            driver: None,
            continuity_broken: false,
        }
    }

    /// Drains observations. The contract itself carries only the connection's state.
    pub fn events(&mut self) -> Option<&mut mpsc::Receiver<MarketEvent>> {
        self.events.as_mut()
    }

    fn spawn(&mut self) -> mpsc::Receiver<MarketEvent> {
        let (tx, rx) = mpsc::channel::<MarketEvent>(4096);
        let request = self.request.clone();

        self.driver = Some(tokio::spawn(async move {
            let _ = crate::stream::run_market_stream(&request, tx).await;
        }));
        rx
    }

    /// Consume events until the link reports usable, the deadline passes, or the stream ends.
    async fn settle(&mut self, mut rx: mpsc::Receiver<MarketEvent>) -> mpsc::Receiver<MarketEvent> {
        let deadline = tokio::time::Instant::now() + CONFIRM_WITHIN;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return rx;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Err(_) | Ok(None) => return rx,
                Ok(Some(MarketEvent::State(state))) => {
                    let usable = state.is_usable();
                    self.state = state;
                    if usable {
                        return rx;
                    }
                }
                Ok(Some(MarketEvent::ContinuityLost)) => self.continuity_broken = true,
                Ok(Some(_)) => {}
            }
        }
    }
}

impl VenueData for MarketData {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn connect(&mut self, _now_ns: i128) -> Answer<'_, Result<ConnectionState, ConnectError>> {
        Box::pin(async move {
            if self.request.instruments.is_empty() {
                return Err(ConnectError::NotPermitted {
                    detail: "a connection with no instruments subscribes to nothing".into(),
                });
            }
            let rx = self.spawn();
            let rx = self.settle(rx).await;
            self.events = Some(rx);
            Ok(self.state.clone())
        })
    }

    fn disconnect(&mut self) -> Answer<'_, ()> {
        Box::pin(async move {
            if let Some(driver) = self.driver.take() {
                driver.abort();
            }
            self.events = None;
            self.state = ConnectionState::Disconnected;
        })
    }

    fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    fn reconnect(&mut self, now_ns: i128) -> Answer<'_, Result<Reconnected, ConnectError>> {
        Box::pin(async move {
            self.disconnect().await;
            let state = self.connect(now_ns).await?;

            // Never `Unbroken`: the sequence is per connection, so nothing says how much was missed.
            self.continuity_broken = false;

            Ok(Reconnected { state, continuity: Continuity::GapOfUnknownExtent })
        })
    }

    fn subscribe<'a>(
        &'a mut self,
        class: DataClass,
        instrument: &'a VenueSymbol,
        now_ns: i128,
    ) -> Answer<'a, Result<SubscriptionAck, SubscribeError>> {
        Box::pin(async move {
            if !self.declaration.serves(class) {
                return Err(SubscribeError::NotServed);
            }
            if !self.request.instruments.iter().any(|i| i.as_str() == instrument.as_str()) {
                return Err(SubscribeError::NotServed);
            }

            // A usable link implies a confirmed subscription (the confirmed-subscription rule is enforced in the session).
            Ok(if self.state.is_usable() {
                SubscriptionAck::Confirmed { at_ns: now_ns }
            } else {
                SubscriptionAck::Unconfirmed { requested_at_ns: now_ns }
            })
        })
    }
}

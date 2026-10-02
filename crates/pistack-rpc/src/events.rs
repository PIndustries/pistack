// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A typed wrapper over `tokio::sync::broadcast<Event>` with filtered
//! subscriptions. `ModuleContext.events` is the raw sender; wrap it with
//! [`EventBus::from_sender`] to get the conveniences here.

use std::sync::Arc;

use pistack_types::{Event, EventAction};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

/// Default channel capacity when none is given.
pub const DEFAULT_CAPACITY: usize = 4096;

type Filter = Arc<dyn Fn(&Event) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("receivers", &self.tx.receiver_count())
            .finish()
    }
}

impl EventBus {
    /// A new bus whose slowest subscriber may fall `capacity` events behind
    /// before it starts seeing `Lagged`.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Wrap an existing sender (for example `ModuleContext::events`).
    pub fn from_sender(tx: broadcast::Sender<Event>) -> Self {
        Self { tx }
    }

    /// The raw sender, for `ModuleContext`.
    pub fn sender(&self) -> broadcast::Sender<Event> {
        self.tx.clone()
    }

    /// Publish; returns how many subscribers received it. Zero is not an
    /// error: nobody listening is normal at boot.
    pub fn publish(&self, ev: Event) -> usize {
        self.tx.send(ev).unwrap_or(0)
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Every event.
    pub fn subscribe(&self) -> EventStream {
        self.subscribe_filter(|_| true)
    }

    /// Events whose `kind` equals `kind` (a resource kind such as `"mach"`).
    pub fn subscribe_kind(&self, kind: impl Into<String>) -> EventStream {
        let kind = kind.into();
        self.subscribe_filter(move |e| e.kind == kind)
    }

    /// Events whose `kind` is any of `kinds`.
    pub fn subscribe_kinds<I, S>(&self, kinds: I) -> EventStream
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let kinds: Vec<String> = kinds.into_iter().map(Into::into).collect();
        self.subscribe_filter(move |e| kinds.contains(&e.kind))
    }

    /// Events of `kind` with the given action.
    pub fn subscribe_kind_action(
        &self,
        kind: impl Into<String>,
        action: EventAction,
    ) -> EventStream {
        let kind = kind.into();
        self.subscribe_filter(move |e| e.kind == kind && e.action == action)
    }

    /// Arbitrary predicate.
    pub fn subscribe_filter<F>(&self, f: F) -> EventStream
    where
        F: Fn(&Event) -> bool + Send + Sync + 'static,
    {
        EventStream {
            rx: self.tx.subscribe(),
            filter: Arc::new(f),
        }
    }
}

/// A filtered subscription. Events that fail the filter are consumed
/// silently; `Lagged` is passed through so the subscriber can resync.
pub struct EventStream {
    rx: broadcast::Receiver<Event>,
    filter: Filter,
}

impl EventStream {
    /// Next matching event. `Err(Lagged(n))` means `n` events (matching or
    /// not) were missed; `Err(Closed)` means every sender is gone.
    pub async fn recv(&mut self) -> Result<Event, RecvError> {
        loop {
            let ev = self.rx.recv().await?;
            if (self.filter)(&ev) {
                return Ok(ev);
            }
        }
    }

    /// Non-blocking variant.
    pub fn try_recv(&mut self) -> Result<Event, TryRecvError> {
        loop {
            let ev = self.rx.try_recv()?;
            if (self.filter)(&ev) {
                return Ok(ev);
            }
        }
    }

    /// Events queued for this subscriber, before filtering.
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pistack_types::{kind, Id};

    fn ev(k: &'static str, action: EventAction) -> Event {
        Event {
            kind: k.into(),
            id: Id::new(k),
            project: None,
            action,
            revision: 1,
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn kind_filter() {
        let bus = EventBus::new(16);
        let mut machines = bus.subscribe_kind(kind::MACHINE);
        let mut all = bus.subscribe();
        assert_eq!(bus.receiver_count(), 2);

        assert_eq!(bus.publish(ev(kind::VOLUME, EventAction::Created)), 2);
        assert_eq!(bus.publish(ev(kind::MACHINE, EventAction::Updated)), 2);
        assert_eq!(bus.publish(ev(kind::MACHINE, EventAction::Deleted)), 2);

        let m = machines.recv().await.unwrap();
        assert_eq!(
            (m.kind.as_str(), m.action),
            (kind::MACHINE, EventAction::Updated)
        );
        let m = machines.recv().await.unwrap();
        assert_eq!(m.action, EventAction::Deleted);
        assert!(matches!(machines.try_recv(), Err(TryRecvError::Empty)));

        assert_eq!(all.recv().await.unwrap().kind, kind::VOLUME);
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn kinds_and_action_filters() {
        let bus = EventBus::default();
        let mut s = bus.subscribe_kinds([kind::MACHINE, kind::POD]);
        let mut deletes = bus.subscribe_kind_action(kind::VOLUME, EventAction::Deleted);
        bus.publish(ev(kind::VOLUME, EventAction::Created));
        bus.publish(ev(kind::POD, EventAction::Created));
        bus.publish(ev(kind::VOLUME, EventAction::Deleted));
        assert_eq!(s.recv().await.unwrap().kind, kind::POD);
        assert_eq!(deletes.recv().await.unwrap().action, EventAction::Deleted);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_zero_not_error() {
        let bus = EventBus::new(4);
        assert_eq!(bus.publish(ev(kind::MACHINE, EventAction::Created)), 0);
    }

    #[tokio::test]
    async fn lag_is_reported() {
        let bus = EventBus::new(2);
        let mut s = bus.subscribe_kind(kind::MACHINE);
        for _ in 0..5 {
            bus.publish(ev(kind::MACHINE, EventAction::Created));
        }
        assert!(matches!(s.recv().await, Err(RecvError::Lagged(3))));
        assert!(s.recv().await.is_ok());
    }

    #[tokio::test]
    async fn from_sender_shares_channel() {
        let bus = EventBus::new(8);
        let other = EventBus::from_sender(bus.sender());
        let mut s = other.subscribe();
        bus.publish(ev(kind::MACHINE, EventAction::Created));
        assert!(s.recv().await.is_ok());
        drop(bus);
        drop(other);
        assert!(matches!(s.recv().await, Err(RecvError::Closed)));
    }
}

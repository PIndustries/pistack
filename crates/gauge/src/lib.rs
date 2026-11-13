// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Metrics and alarms.
//!
//! Samples are kept per series in a short window in memory ([`SampleRing`]):
//! enough for the API, `pi machine stats` and alarms. They are not written
//! to disk and not replicated, so each server has its own window and a
//! restart empties it. History is Prometheus's job: it scrapes `/metrics`.
//!
//! Alarms are store resources. Recording a sample re-evaluates the alarms
//! whose metric selects its series: the alarm is [`AlarmPhase::Firing`]
//! while any selected series crosses the threshold.
//!
//! Every `Gauge` value, including `Arc::new(gauge::Gauge)` in the server
//! and a heartbeat in another crate, shares one process-wide window.

pub mod api;
pub mod catalog;
pub mod client;
pub mod series;
pub mod store;
pub mod types;

pub use client::{Client, Stat};
pub use series::{series, Series};
pub use store::{Reading, SampleRing};
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use client::Client as GaugeClient;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::{Arc, OnceLock};
use store::SampleRing as Ring;

/// The gauge module. Samples live in [`samples`], not on this value.
#[derive(Default)]
pub struct Gauge;

impl Gauge {
    pub const fn new() -> Self {
        Self
    }

    /// The process-wide samples.
    pub fn buffer(&self) -> Arc<SampleRing> {
        samples()
    }

    pub fn client(&self, store: Store) -> GaugeClient {
        GaugeClient::new(store, self.buffer())
    }
}

/// One window for the process. The server, `/metrics` and host heartbeats
/// all use it.
pub fn samples() -> Arc<SampleRing> {
    static SAMPLES: OnceLock<Arc<Ring>> = OnceLock::new();
    SAMPLES.get_or_init(|| Arc::new(Ring::memory())).clone()
}

#[async_trait]
impl Module for Gauge {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "gauge",
            prefix: "gauge",
            summary: "Metrics, events, alarms, metering",
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        api::router(self.client((*store).clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_gauge_values_share_one_window() {
        let key = series(catalog::HOST_VCPUS, &[("host", "gauge-shared")]);
        Gauge
            .client(Store::in_memory().unwrap())
            .record_all(vec![(key, 7.0)]);
        let text = Gauge::new()
            .client(Store::in_memory().unwrap())
            .metrics_text();
        assert!(text.contains("host=\"gauge-shared\""), "{text}");
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Metrics and alarms.
//!
//! Samples are a ring on [`Gauge`] ([`MAX_SAMPLES`], oldest dropped). Alarms
//! are store resources. A posted sample re-evaluates alarms on that metric:
//! the latest value either crosses the threshold ([`AlarmPhase::Firing`]) or
//! it does not ([`AlarmPhase::Ok`]). `GET /metrics` is Prometheus text.
//!
//! Every `Gauge` value, including `Arc::new(gauge::Gauge)` in the server
//! and a heartbeat in another crate, shares one process-wide sample ring.

pub mod api;
pub mod client;
pub mod types;

pub use client::{record_host, Client};
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use client::Client as GaugeClient;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::{Arc, Mutex, OnceLock};

/// The gauge module. Samples live in [`samples`], not on this value.
#[derive(Default)]
pub struct Gauge;

impl Gauge {
    pub const fn new() -> Self {
        Self
    }

    /// The process-wide sample ring.
    pub fn buffer(&self) -> Arc<Mutex<Vec<Sample>>> {
        samples()
    }

    pub fn client(&self, store: Store) -> GaugeClient {
        GaugeClient::new(store, self.buffer())
    }
}

/// One ring for the process. The server, `GET /metrics`, and host
/// heartbeats all write here.
pub fn samples() -> Arc<Mutex<Vec<Sample>>> {
    static SAMPLES: OnceLock<Arc<Mutex<Vec<Sample>>>> = OnceLock::new();
    SAMPLES
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[async_trait]
impl Module for Gauge {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "gauge",
            prefix: "gauge",
            summary: "Metrics, events, alarms, metering",
            version: env!("CARGO_PKG_VERSION"),
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
    use client::record_host;

    #[test]
    fn two_gauge_values_share_one_ring() {
        record_host(Gauge.buffer().as_ref(), "gauge-shared-ring", 7, 1);
        let text = Gauge::new()
            .client(Store::in_memory().unwrap())
            .metrics_text();
        assert!(text.contains("node=\"gauge-shared-ring\""), "{text}");
    }
}

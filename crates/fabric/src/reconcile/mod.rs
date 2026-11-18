// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reconciler: turns `spec` into backend state and writes `status`.
//!
//! Driven by store events (`handle_event`), a full pass at start
//! (`reconcile_all`) and a periodic retry of anything not `Ready`
//! (`reconcile_pending`). Every status write records
//! `observed_revision = revision + 1`, the revision the write itself
//! produces, so the event that write emits is recognised and skipped.
//!
//! Dependencies are pulled, not scheduled: realising a port first makes
//! sure its network is realised, a router makes sure its ports are. That
//! keeps ordering correct whatever order events arrive in.

macro_rules! observed {
    ($t:ty) => {
        impl $crate::reconcile::Observed for $t {
            fn phase(&self) -> Phase {
                self.status.phase
            }
            fn observed_revision(&self) -> u64 {
                self.status.observed_revision
            }
            fn message(&self) -> &str {
                &self.status.message
            }
            fn set_result(&mut self, phase: Phase, message: String, observed: u64) {
                self.status.phase = phase;
                self.status.message = message;
                self.status.observed_revision = observed;
            }
        }
    };
}

mod api;
mod client;
mod finish;
mod floating_ip;
mod network;
mod observed;
mod port;
mod router;
mod security_group;
mod subnet;
#[cfg(test)]
mod tests;
mod vpc;
pub use observed::Observed;

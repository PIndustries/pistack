// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, Phase};

/// Common status surface, so the event loop can ask "is this stale?" for
/// any of our kinds.
pub trait Observed: Kind {
    fn phase(&self) -> Phase;
    fn observed_revision(&self) -> u64;
    fn message(&self) -> &str;
    fn set_result(&mut self, phase: Phase, message: String, observed: u64);
    /// Written since the reconciler last looked. An event acts on this.
    fn unseen(&self) -> bool {
        self.observed_revision() != self.meta().revision
    }
    /// Due on the periodic pass: unseen, or failed last time. A failure is
    /// not retried on events. The status write that records it is itself an
    /// event, so retrying there never stops.
    fn stale(&self) -> bool {
        self.unseen() || self.phase() == Phase::Error
    }
}

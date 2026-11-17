// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Local TCP forwarder. Serves [`Provider::Forward`] (L4). HTTP stays on
//! [`super::super::RelayL7Provider`].

use super::dial::Dial;
use super::slot::Slots;
use crate::types::Algorithm;
use pistack_types::Id;
use std::sync::Arc;

pub struct ForwardProvider {
    pub(super) slots: Slots,
}

impl Default for ForwardProvider {
    fn default() -> Self {
        Self {
            slots: parking_lot::Mutex::new(Default::default()),
        }
    }
}

impl ForwardProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) async fn stop(&self, id: &Id) {
        let slot = self.slots.lock().remove(id);
        if let Some(mut slot) = slot {
            slot.cancel.cancel();
            if let Some(udp) = slot.udp.take() {
                udp.shutdown().await;
            }
            if let Some(task) = slot.task {
                let _ = task.await;
            }
        }
    }
}

impl Drop for ForwardProvider {
    fn drop(&mut self) {
        let slots = std::mem::take(&mut *self.slots.lock());
        for (_, slot) in slots {
            slot.cancel.cancel();
            if let Some(task) = slot.task {
                task.abort();
            }
        }
    }
}

/// The listener's pool and members as the forwarder dials them. Members
/// not fit to take traffic are already out of the view.
pub(super) fn dial_for(view: &super::super::ListenerView) -> Arc<Dial> {
    use crate::types::Member;
    let empty: &[Member] = &[];
    let (algo, members) = match &view.pool {
        Some((pool, members)) if pool.spec.admin_up => (pool.spec.algorithm, members.as_slice()),
        // No pool, or one administratively down: nothing to dial.
        _ => (Algorithm::default(), empty),
    };
    let dial = Arc::new(Dial::new(algo));
    dial.set_members(members);
    dial
}

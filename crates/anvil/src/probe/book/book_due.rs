// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::book_type::Book;
use crate::probe::book::due_check::DueCheck;
use crate::probe::book::probe_target::ProbeTarget;
use chrono::{DateTime, Utc};

impl Book {
    pub(super) fn due(&mut self, targets: &[ProbeTarget], now: DateTime<Utc>) -> Vec<DueCheck> {
        let mut out = Vec::new();
        for target in targets {
            if !target.power_on {
                continue;
            }
            let Some(machine) = self.machines.get_mut(&target.id) else {
                continue;
            };
            let generation = machine.generation;
            for track in &mut machine.tracks {
                if !track.due(now) {
                    continue;
                }
                track.last_attempt = Some(now);
                out.push(DueCheck {
                    machine: target.id.clone(),
                    generation,
                    container: track.container.clone(),
                    kind: track.kind.as_str().to_owned(),
                    action: track.spec.action(),
                    timeout: track.spec.timeout(),
                    address: target.address.clone(),
                });
            }
        }
        out
    }
}

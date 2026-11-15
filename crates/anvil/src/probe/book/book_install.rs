// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::book_type::Book;
use crate::probe::book::machine_probes::MachineProbes;
use crate::probe::book::tracks_from::tracks_from;
use crate::probe::spec::container_probes::ContainerProbes;
use chrono::{DateTime, Utc};
use pistack_types::Id;

impl Book {
    pub fn install(&mut self, id: &Id, specs: Vec<ContainerProbes>, now: DateTime<Utc>) {
        let tracks = tracks_from(specs, now);
        if tracks.is_empty() {
            self.machines.remove(id);
            return;
        }
        let generation = self
            .machines
            .get(id)
            .map(|m| m.generation.saturating_add(1))
            .unwrap_or(1);
        self.machines
            .insert(id.clone(), MachineProbes { generation, tracks });
    }
}

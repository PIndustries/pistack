// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::book_type::Book;
use pistack_rpc::proto::ProbeReport;
use pistack_types::Id;

impl Book {
    pub(super) fn reports(&self, id: &Id) -> Vec<ProbeReport> {
        let Some(machine) = self.machines.get(id) else {
            return Vec::new();
        };
        machine
            .tracks
            .iter()
            .map(|t| ProbeReport {
                container: t.container.clone(),
                kind: t.kind.as_str().to_owned(),
                success: t.success,
                successes: t.successes,
                failures: t.failures,
                message: t.message.clone(),
                unsupported: t.unsupported,
            })
            .collect()
    }
}

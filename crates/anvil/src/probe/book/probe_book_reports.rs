// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use pistack_rpc::proto::ProbeReport;
use pistack_types::Id;

impl ProbeBook {
    pub fn reports(&self, id: &Id) -> Vec<ProbeReport> {
        self.lock().reports(id)
    }
}

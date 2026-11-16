// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_kind::ProbeKind;
use pistack_rpc::proto::{PROBE_LIVENESS, PROBE_READINESS};

impl ProbeKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ProbeKind::Liveness => PROBE_LIVENESS,
            ProbeKind::Readiness => PROBE_READINESS,
        }
    }
}

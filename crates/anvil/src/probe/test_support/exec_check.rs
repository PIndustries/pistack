// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::spec::probe_action::ProbeAction;
use pistack_rpc::proto::PROBE_LIVENESS;
use pistack_types::{kind, Id};
use std::time::Duration;

pub(in crate::probe) fn exec_check(command: Vec<String>, timeout: Duration) -> DueCheck {
    DueCheck {
        machine: Id::new(kind::MACHINE),
        generation: 1,
        container: "web".into(),
        kind: PROBE_LIVENESS.into(),
        action: ProbeAction::Exec { command },
        timeout,
        address: Some("127.0.0.1".into()),
    }
}

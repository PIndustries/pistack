// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

use super::agent_certs::agent_certs;
use super::app_state::AppState;

/// `host` as the API shows it: with its certificate's expiry.
pub(crate) fn shown(s: &AppState, mut host: Host) -> Host {
    host.status.cert_expires = keep::agent_cert_expiry(&agent_certs(s), &host.spec.node_name);
    host
}

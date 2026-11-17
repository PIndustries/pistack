// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The members a UDP listener may send to: its primaries, or every up
//! member when no primary is up.

use super::is_up::is_up;
use crate::provider::ListenerView;
use std::net::SocketAddr;

/// Up members that should take datagrams. Backup members are used only when
/// no primary is up. Administratively down, zero-weight, offline, and
/// draining members are left out.
pub fn targets(view: &ListenerView) -> Vec<SocketAddr> {
    if !view.listener.spec.admin_up {
        return Vec::new();
    }
    let Some((_, members)) = &view.pool else {
        return Vec::new();
    };
    let primary: Vec<SocketAddr> = members
        .iter()
        .filter(|m| is_up(m) && !m.spec.backup)
        .map(|m| m.addr())
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members
        .iter()
        .filter(|m| is_up(m))
        .map(|m| m.addr())
        .collect()
}

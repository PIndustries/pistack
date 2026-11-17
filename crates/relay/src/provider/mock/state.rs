// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The in-memory provider's recorded calls, allocated VIPs and failure mode.

use super::call::Call;
use pistack_types::Id;
use std::collections::BTreeMap;
use std::net::IpAddr;

#[derive(Default)]
pub(super) struct State {
    pub(super) calls: Vec<Call>,
    pub(super) vips: BTreeMap<Id, IpAddr>,
    pub(super) next: u32,
    /// When set, every call fails with this message.
    pub(super) fail: Option<String>,
}

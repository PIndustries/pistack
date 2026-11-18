// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Meta;
use std::net::IpAddr;

pub(crate) struct ListedMachine {
    pub(crate) meta: Meta,
    pub(crate) ipv4: Option<IpAddr>,
    /// Reason and message of its [`FIP_CONDITION`], if it has one.
    pub(crate) noted: Option<(String, String)>,
}

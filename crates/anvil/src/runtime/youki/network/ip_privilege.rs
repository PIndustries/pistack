// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;

pub(in crate::runtime::youki) fn ip_privilege(mode: &RunMode) -> podnet::IpPrivilege {
    match &mode.privilege {
        Privilege::Root => podnet::IpPrivilege::Direct,
        Privilege::Sudo { sudo } => podnet::IpPrivilege::Sudo(sudo.clone()),
        Privilege::Rootless { .. } => podnet::IpPrivilege::Rootless,
    }
}

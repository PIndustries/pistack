// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use std::path::Path;

pub(in crate::runtime::youki) fn command_program(bin: &Path, mode: &RunMode) -> String {
    match &mode.privilege {
        Privilege::Sudo { sudo } => sudo.display().to_string(),
        Privilege::Root | Privilege::Rootless { .. } => bin.display().to_string(),
    }
}

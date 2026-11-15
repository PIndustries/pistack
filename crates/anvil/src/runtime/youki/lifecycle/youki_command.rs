// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::privilege::Privilege;
use std::path::Path;
use tokio::process::Command;

pub(in crate::runtime::youki) fn youki_command(bin: &Path, privilege: &Privilege) -> Command {
    match privilege {
        Privilege::Sudo { sudo } => {
            let mut cmd = Command::new(sudo);
            cmd.arg("-n").arg(bin);
            cmd
        }
        Privilege::Root | Privilege::Rootless { .. } => Command::new(bin),
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;
use tokio::process::Command;

pub(in crate::runtime::youki) fn arm_user_ns_join(cmd: &mut Command, path: Option<&Path>) {
    #[cfg(target_os = "linux")]
    {
        let Some(path) = path else {
            return;
        };
        // A missing path is the test double (the sandbox pid is not real).
        // A path that exists and cannot be joined fails the command.
        if !path.exists() {
            return;
        }
        let path = path.to_path_buf();
        // SAFETY: the closure runs in the forked child before exec. That child
        // is single-threaded, so setns(CLONE_NEWUSER) is valid there.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(cmd.as_std_mut(), move || {
                super::enter_user_ns::enter_user_ns(&path)
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cmd, path);
    }
}

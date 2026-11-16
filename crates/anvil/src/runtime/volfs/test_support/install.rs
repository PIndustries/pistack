// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::shell_quote::shell_quote;
use super::tool_body::tool_body;
use super::tools::Tools;
use super::write_exe::write_exe;
use std::path::Path;

pub(in crate::runtime::volfs) fn install(dir: &Path) -> Tools {
    let log = dir.join("tools.log");
    let empty = dir.join("blkid-empty");
    let mkfs_fail = dir.join("mkfs-fail");
    let mount_fail = dir.join("mount-fail");
    let not_mounted = dir.join("umount-not-mounted");
    let sudo_log = dir.join("sudo.log");
    let losetup = dir.join("losetup");
    let blkid = dir.join("blkid");
    let mkfs = dir.join("mkfs.ext4");
    let mount = dir.join("mount");
    let umount = dir.join("umount");
    let sudo = dir.join("sudo");
    write_exe(
        &losetup,
        &tool_body(
            &log,
            "if [ \"$1\" = \"--find\" ]; then printf '%s\\n' /dev/loop7; exit 0; fi\nexit 0\n",
        ),
    );
    write_exe(
        &blkid,
        &tool_body(
            &log,
            &format!(
                "if [ -f {} ]; then exit 2; fi\nprintf '%s\\n' ext4\nexit 0\n",
                shell_quote(&empty)
            ),
        ),
    );
    write_exe(
        &mkfs,
        &tool_body(
            &log,
            &format!(
                "if [ -f {} ]; then echo 'mkfs failed: device busy' >&2; exit 1; fi\nexit 0\n",
                shell_quote(&mkfs_fail)
            ),
        ),
    );
    write_exe(
        &mount,
        &tool_body(
            &log,
            &format!(
                "if [ -f {} ]; then echo 'mount failed: no such device' >&2; exit 1; fi\nexit 0\n",
                shell_quote(&mount_fail)
            ),
        ),
    );
    write_exe(
        &umount,
        &tool_body(
            &log,
            &format!(
                "if [ -f {} ]; then echo 'umount: not mounted' >&2; exit 32; fi\nexit 0\n",
                shell_quote(&not_mounted)
            ),
        ),
    );
    write_exe(
        &sudo,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$(basename \"$0\")\" \"$@\" >> {}\nprintf '\\n' >> {}\nif [ \"$1\" = \"-n\" ]; then shift; fi\nexec \"$@\"\n",
            shell_quote(&sudo_log),
            shell_quote(&sudo_log)
        ),
    );
    Tools {
        log,
        empty,
        mkfs_fail,
        mount_fail,
        not_mounted,
        losetup,
        blkid,
        mkfs,
        mount,
        umount,
        sudo,
        sudo_log,
    }
}

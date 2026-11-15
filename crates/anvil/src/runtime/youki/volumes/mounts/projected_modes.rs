// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `(file mode, directory mode)`. Sudo mode opens Secret files to the
/// container user. Root and rootless keep them private to the owner.
use crate::runtime::youki::privilege::Privilege;

pub(in crate::runtime::youki) fn projected_modes(
    privilege: &Privilege,
    secret: bool,
) -> (u32, u32) {
    if matches!(privilege, Privilege::Sudo { .. }) {
        // 0644, not 0640. The container user is uid 0 / gid 0, not the
        // agent's group, and may not have CAP_DAC_OVERRIDE. The bundle
        // directory is 0700, so other host users cannot traverse here.
        return (0o644, 0o755);
    }
    if secret {
        (0o400, 0o700)
    } else {
        (0o644, 0o755)
    }
}

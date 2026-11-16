// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// In sudo mode the bundle is `0700` and owned by the agent. Other host
/// users cannot traverse it. Root and rootless leave the mode alone.
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::volumes::set_mode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn privatize_bundle(
    bundle: &Path,
    privilege: &Privilege,
) -> ApiResult<()> {
    if matches!(privilege, Privilege::Sudo { .. }) {
        set_mode(bundle, 0o700)?;
    }
    Ok(())
}

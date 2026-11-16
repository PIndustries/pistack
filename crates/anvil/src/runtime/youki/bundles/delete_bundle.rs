// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Unlink a symlink `rootfs` without following it, then remove the bundle.
use crate::runtime::youki::bundles::delete_path;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn delete_bundle(
    bundle: &Path,
    privilege: &Privilege,
) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    if let Ok(meta) = std::fs::symlink_metadata(&rootfs) {
        if meta.file_type().is_symlink() {
            delete_path(&rootfs, privilege)?;
        }
    }
    delete_path(bundle, privilege)
}

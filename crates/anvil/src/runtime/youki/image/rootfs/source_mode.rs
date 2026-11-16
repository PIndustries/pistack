// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Permission bits for a copied file or directory. Same mask as a tar header:
/// `0o7777`, with setuid and setgid kept only when [`keep_setid`] is set.
use crate::runtime::youki::image::keep_setid;
use crate::runtime::youki::image::mask_mode;

pub(in crate::runtime::youki) fn source_mode(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        mask_mode(meta.permissions().mode(), keep_setid())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0
    }
}

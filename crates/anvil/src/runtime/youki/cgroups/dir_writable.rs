// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(target_os = "linux")]
use std::path::Path;

#[cfg(target_os = "linux")]
pub(in crate::runtime::youki) fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // access follows the caller's credentials. W_OK is the write bit.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(target_os = "linux")]
pub(in crate::runtime::youki) fn enter_user_ns(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "youki runtime: user namespace path",
        )
    })?;
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rc = unsafe { libc::setns(fd, libc::CLONE_NEWUSER) };
    let err = std::io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if rc == 0 {
        Ok(())
    } else {
        Err(err)
    }
}

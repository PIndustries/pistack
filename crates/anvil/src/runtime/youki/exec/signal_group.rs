// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn signal_group(pid: u32) {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        if pid <= 0 {
            return;
        }
        // SAFETY: `pid` is a positive process-group id created with
        // `process_group(0)`. A negative pid asks the kernel to signal that
        // group. A stale id fails the call; the result is ignored.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

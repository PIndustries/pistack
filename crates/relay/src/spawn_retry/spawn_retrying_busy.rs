// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Running `spawn`, trying again while it says the file is still open for
//! writing.

use super::spawn_attempts::SPAWN_ATTEMPTS;
use super::spawn_retry_delay::SPAWN_RETRY_DELAY;
use std::io;

/// Run `spawn`, trying again while it says the file is still open for
/// writing. A helper that was only just written or replaced can still be
/// open for writing for a moment: another thread forking a child at that
/// instant hands it a copy of the write descriptor, and the kernel refuses
/// the exec until the last one closes. The window is microseconds, so a
/// short wait clears it; anything else is returned at once.
pub(crate) async fn spawn_retrying_busy<T, F>(mut spawn: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut attempt = 1;
    loop {
        match spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(SPAWN_RETRY_DELAY).await;
            }
            other => return other,
        }
    }
}

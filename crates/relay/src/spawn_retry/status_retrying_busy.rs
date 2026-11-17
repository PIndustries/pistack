// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Running a helper to completion, trying the start again while it says the
//! file is still open for writing.

use super::spawn_attempts::SPAWN_ATTEMPTS;
use super::spawn_retry_delay::SPAWN_RETRY_DELAY;
use std::io;

/// Run a helper to completion, trying the start again while it says the file
/// is still open for writing.
///
/// This is [`tokio::process::Command::status`] with the retry wrapped around
/// it, and nothing else: the same stdio `status` would give, the same result,
/// the same pipes left alone. When the exec is refused with "text file busy"
/// nothing has run, so calling `status` again on the same command starts it
/// fresh; any other outcome is returned at once.
pub(crate) async fn status_retrying_busy(
    cmd: &mut tokio::process::Command,
) -> io::Result<std::process::ExitStatus> {
    let mut attempt = 1;
    loop {
        match cmd.status().await {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(SPAWN_RETRY_DELAY).await;
            }
            other => return other,
        }
    }
}

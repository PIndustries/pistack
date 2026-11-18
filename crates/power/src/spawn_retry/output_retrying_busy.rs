// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::io;

use super::spawn_attempts::SPAWN_ATTEMPTS;
use super::spawn_retry_delay::SPAWN_RETRY_DELAY;

/// Run a tool to completion and collect what it wrote, trying the start
/// again while it says the file is still open for writing.
///
/// This is [`tokio::process::Command::output`] with the retry wrapped
/// around it, and nothing else: the same stdio `output` would give, the
/// same result, the same care against a tool that fills a pipe. When the
/// exec is refused with "text file busy" nothing has run, so calling
/// `output` again on the same command starts it fresh; any other outcome is
/// returned at once.
///
/// The window is real: a test that writes a small script to a temp dir can
/// have another thread fork at the instant the file is still open for
/// writing. The child inherits a copy of the write descriptor and the
/// kernel refuses the exec until the last one closes. The window is
/// microseconds, so a short wait clears it.
pub(crate) async fn output_retrying_busy(
    cmd: &mut tokio::process::Command,
) -> io::Result<std::process::Output> {
    let mut attempt = 1;
    loop {
        match cmd.output().await {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(SPAWN_RETRY_DELAY).await;
            }
            other => return other,
        }
    }
}

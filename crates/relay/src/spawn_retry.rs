// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Retrying the start of a helper program that may still be being written.

use std::io;
use std::time::Duration;

/// Attempts before giving up on a helper that answers "text file busy".
pub(crate) const SPAWN_ATTEMPTS: u32 = 20;

/// Pause between attempts, short enough to stay invisible on the happy path.
pub(crate) const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn busy() -> io::Error {
        io::Error::new(
            io::ErrorKind::ExecutableFileBusy,
            "Text file busy (os error 26)",
        )
    }

    #[tokio::test]
    async fn a_helper_busy_twice_is_run_a_third_time() {
        let calls = AtomicU32::new(0);
        let out = spawn_retrying_busy(|| {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(busy())
            } else {
                Ok("spawned")
            }
        })
        .await
        .unwrap();
        assert_eq!(out, "spawned");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_helper_always_busy_comes_back_after_the_tries_are_used_up() {
        let calls = AtomicU32::new(0);
        let err = spawn_retrying_busy(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(busy())
        })
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ExecutableFileBusy);
        assert_eq!(calls.load(Ordering::SeqCst), SPAWN_ATTEMPTS);
    }

    #[tokio::test]
    async fn any_other_error_is_not_retried() {
        let calls = AtomicU32::new(0);
        let err = spawn_retrying_busy(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(io::Error::new(
                io::ErrorKind::NotFound,
                "No such file or directory",
            ))
        })
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_started_helper_is_collected_like_status_would() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("printf out-here; printf err-here >&2; exit 3");
        let status = status_retrying_busy(&mut cmd).await.unwrap();
        assert_eq!(status.code(), Some(3));
    }

    #[tokio::test]
    async fn a_helper_that_is_not_there_is_reported_at_once() {
        let mut cmd = tokio::process::Command::new("/no/such/helper-at-all");
        let err = status_retrying_busy(&mut cmd).await.unwrap_err();
        assert!(
            err.kind() != io::ErrorKind::ExecutableFileBusy,
            "a missing helper must not be retried as busy: {err:?}"
        );
    }
}

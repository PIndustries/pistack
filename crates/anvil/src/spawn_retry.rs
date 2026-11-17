// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Retrying the spawn of a helper program that may still be being written.

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

/// Run a helper to completion and collect what it wrote, trying the start
/// again while it says the file is still open for writing.
///
/// This is [`tokio::process::Command::output`] with the retry wrapped around
/// it, and nothing else: the same stdio `output` would give (both streams
/// captured, the child left with no standard input), the same result, the
/// same care against a helper that fills a pipe. When the exec is refused
/// with "text file busy" nothing has run, so calling `output` again on the
/// same command starts it fresh; any other outcome is returned at once.
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

/// Run `spawn`, trying again while it says the file is still open for
/// writing, with a plain busy wait between tries. The one synchronous
/// helper start in the crate, the privileged cleanup helper, uses this: it
/// has no executor to hand a timer to.
pub(crate) fn spawn_sync_retrying_busy<T, F>(mut spawn: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut attempt = 1;
    loop {
        match spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(SPAWN_RETRY_DELAY);
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

    #[test]
    fn the_sync_helper_gets_the_same_tries() {
        let calls = AtomicU32::new(0);
        let out = spawn_sync_retrying_busy(|| {
            if calls.fetch_add(1, Ordering::SeqCst) < 1 {
                Err(busy())
            } else {
                Ok("spawned")
            }
        })
        .unwrap();
        assert_eq!(out, "spawned");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
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

    #[tokio::test]
    async fn a_helper_that_is_not_there_is_reported_at_once() {
        let mut cmd = tokio::process::Command::new("/no/such/helper-at-all");
        let err = output_retrying_busy(&mut cmd).await.unwrap_err();
        assert!(
            err.kind() != io::ErrorKind::ExecutableFileBusy,
            "a missing helper must not be retried as busy: {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_started_helper_is_collected_like_output_would() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("printf out-here; printf err-here >&2; exit 3");
        let out = output_retrying_busy(&mut cmd).await.unwrap();
        assert_eq!(out.stdout, b"out-here");
        assert_eq!(out.stderr, b"err-here");
        assert_eq!(out.status.code(), Some(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_helper_writing_far_past_a_pipe_is_all_collected() {
        // 300 KiB down each stream, several times a pipe buffer (64 KiB). A
        // reader that waits for the child to exit before draining would
        // deadlock here; `output` drains the pipes concurrently with the wait.
        // A /dev/zero read capped by `head -c` gives exactly 307_200 bytes.
        const BYTES: usize = 300 * 1024;
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(format!(
            "head -c {BYTES} </dev/zero; head -c {BYTES} </dev/zero >&2"
        ));
        let out = output_retrying_busy(&mut cmd).await.unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), BYTES);
        assert_eq!(out.stderr.len(), BYTES);
    }
}

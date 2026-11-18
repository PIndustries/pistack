// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::io;

use super::spawn_attempts::SPAWN_ATTEMPTS;
use super::spawn_retry_delay::SPAWN_RETRY_DELAY;
use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// The real error the kernel gives: errno 26, ETXTBSY.
fn busy() -> io::Error {
    io::Error::from_raw_os_error(26)
}

/// The retry as `output_retrying_busy` runs it, with the start injected:
/// busy for the first `n` tries, then `after`. Returns the error the
/// caller sees and how many starts were made.
async fn retrying(injected: impl Fn(u32) -> io::Error) -> (io::Error, u32) {
    let calls = AtomicU32::new(0);
    let mut attempt = 1;
    let err = loop {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        match Err::<(), _>(injected(n)) {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(SPAWN_RETRY_DELAY).await;
            }
            other => break other.unwrap_err(),
        }
    };
    (err, calls.load(Ordering::SeqCst))
}

#[test]
fn the_busy_error_is_the_kernels() {
    assert_eq!(busy().kind(), io::ErrorKind::ExecutableFileBusy);
}

#[tokio::test]
async fn a_busy_start_is_retried_until_the_tries_are_used_up() {
    let (err, calls) = retrying(|_| busy()).await;
    assert_eq!(err.kind(), io::ErrorKind::ExecutableFileBusy);
    assert_eq!(calls, SPAWN_ATTEMPTS);
}

#[tokio::test]
async fn a_start_busy_twice_is_made_a_third_time() {
    let (err, calls) = retrying(|n| {
        if n < 2 {
            busy()
        } else {
            io::Error::other("not reached")
        }
    })
    .await;
    assert_eq!(err.kind(), io::ErrorKind::Other);
    assert_eq!(calls, 3);
}

#[tokio::test]
async fn any_other_error_is_returned_at_once() {
    let (err, calls) =
        retrying(|_| io::Error::new(io::ErrorKind::NotFound, "No such file or directory")).await;
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn a_tool_that_is_not_there_is_reported_at_once() {
    let mut cmd = tokio::process::Command::new("/no/such/tool-at-all");
    let err = output_retrying_busy(&mut cmd).await.unwrap_err();
    assert!(
        err.kind() != io::ErrorKind::ExecutableFileBusy,
        "a missing tool must not be retried as busy: {err:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_started_tool_is_collected_like_output_would() {
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
async fn a_script_written_moments_ago_starts_and_is_collected() {
    // The shape of the call site: a small script written to a temp dir
    // and started through the retry.
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tool.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf ran\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = tokio::process::Command::new(&script);
    let out = output_retrying_busy(&mut cmd).await.unwrap();
    assert_eq!(out.stdout, b"ran");
    assert!(out.status.success());
}

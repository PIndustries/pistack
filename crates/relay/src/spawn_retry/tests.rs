// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Spawn-retry tests: the busy retry loop and the pass-through of any other
//! outcome.

use super::spawn_attempts::SPAWN_ATTEMPTS;
use super::{spawn_retrying_busy, status_retrying_busy};
use std::io;

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

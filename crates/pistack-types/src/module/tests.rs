// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::leadership::Leadership;
use super::CancellationToken;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn run_while_leader_starts_on_promotion_and_stops_on_demotion() {
    let leadership = Leadership::new(false);
    let shutdown = CancellationToken::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));

    let runs_t = runs.clone();
    let active_t = active.clone();
    let lead = leadership.clone();
    let stop = shutdown.clone();
    let task = tokio::spawn(async move {
        lead.run_while_leader(stop, move |child| {
            runs_t.fetch_add(1, Ordering::SeqCst);
            active_t.fetch_add(1, Ordering::SeqCst);
            let active_t = active_t.clone();
            async move {
                child.cancelled().await;
                active_t.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .await;
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert_eq!(active.load(Ordering::SeqCst), 0);

    leadership.set(true);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(active.load(Ordering::SeqCst), 1);

    leadership.set(false);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    leadership.set(true);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 1);

    shutdown.cancel();
    task.await.unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

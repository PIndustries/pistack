// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Samples written through the client survive a fresh ring loaded from disk.

use chrono::Utc;
use gauge::{Client, SampleRing, SAMPLES_FILE};
use pistack_store::Store;
use std::sync::Arc;

#[test]
fn client_samples_round_trip_on_disk() {
    let dir = std::env::temp_dir().join(format!(
        "gauge-client-persist-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let ring = Arc::new(SampleRing::memory());
    ring.attach(&dir).unwrap();
    let client = Client::new(Store::in_memory().unwrap(), ring);
    client.record_sample("cpu.usage", 42.5).unwrap();
    client.record_sample("cpu.usage", 77.0).unwrap();
    assert!(dir.join(SAMPLES_FILE).is_file());

    let again = Arc::new(SampleRing::memory());
    again.attach(&dir).unwrap();
    let client = Client::new(Store::in_memory().unwrap(), again);
    let samples = client.list_samples(Some("cpu.usage"));
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].value, 42.5);
    assert_eq!(samples[1].value, 77.0);

    let _ = std::fs::remove_dir_all(&dir);
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, PowerState};
use std::sync::Arc;

use super::*;
use crate::types::{new_machine, MachineSpec};

fn machine(name: &str) -> Machine {
    new_machine(
        name,
        None,
        MachineSpec {
            size: "s.1x1".into(),
            image: "x".into(),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn mock_addresses_are_deterministic_and_stable() {
    let d = MockDriver::new();
    let a = machine("a");
    let b = machine("b");
    assert_eq!(d.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
    assert_eq!(d.start(&b).await.unwrap().addresses, vec!["10.0.0.2"]);
    // restart keeps the address
    assert_eq!(d.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
    d.stop(&a.meta.id).await.unwrap();
    assert_eq!(
        d.list()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.power == PowerState::Off)
            .count(),
        1
    );
    d.delete(&a.meta.id).await.unwrap();
    d.delete(&a.meta.id).await.unwrap(); // idempotent
    assert_eq!(d.len(), 1);
    assert!(d.stop(&a.meta.id).await.is_err());
}

#[tokio::test]
async fn channel_driver_round_trips() {
    let (driver, rx) = ChannelDriver::new(8);
    let backend: Arc<dyn HostDriver> = Arc::new(MockDriver::new());
    let pump = tokio::spawn(serve_driver(rx, backend));
    let m = machine("c");
    let rt = driver.start(&m).await.unwrap();
    assert_eq!(rt.addresses, vec!["10.0.0.1"]);
    assert_eq!(driver.list().await.unwrap().len(), 1);
    driver.stop(&m.meta.id).await.unwrap();
    driver.delete(&m.meta.id).await.unwrap();
    assert!(driver.list().await.unwrap().is_empty());
    drop(driver);
    pump.await.unwrap();
}

#[tokio::test]
async fn channel_driver_reports_closed_receiver() {
    let (driver, rx) = ChannelDriver::new(1);
    drop(rx);
    let err = driver.list().await.unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
}

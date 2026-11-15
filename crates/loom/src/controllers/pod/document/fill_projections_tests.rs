// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::*;
use crate::types::{
    ConfigMap, ConfigMapSpec, Container, EnvVar, EnvVarSource, Pod, PodSpec, PodVolume,
    VolumeSource,
};
use pistack_store::Store;
use pistack_types::{kind, Id};
use std::collections::BTreeMap;

#[test]
fn sandbox_carries_projected_files_the_debug_view_omits() {
    let store = Store::in_memory().unwrap();
    let project = Id::new(kind::PROJECT);
    let keep = keep::Client::open(&store).unwrap();
    let secret = keep
        .create_secret(
            keep::Secret::new("db", Some(project.clone()), keep::SecretSpec::default()),
            br#"{"password":"s3cret-value"}"#,
        )
        .unwrap();
    let plain = keep
        .create_secret(
            keep::Secret::new("token", Some(project.clone()), keep::SecretSpec::default()),
            b"plain-token",
        )
        .unwrap();
    let mut data = BTreeMap::new();
    data.insert("MODE".into(), "prod".into());
    let cm = store
        .collection::<ConfigMap>()
        .create(ConfigMap::new(
            "app",
            Some(project.clone()),
            ConfigMapSpec {
                data,
                ..ConfigMapSpec::default()
            },
        ))
        .unwrap();
    let pod = Pod::new(
        "web",
        Some(project),
        PodSpec {
            containers: vec![Container {
                name: "web".into(),
                image: "img".into(),
                env: vec![
                    EnvVar {
                        name: "MODE".into(),
                        value_from: Some(EnvVarSource::ConfigMap {
                            name: "app".into(),
                            key: "MODE".into(),
                        }),
                        ..EnvVar::default()
                    },
                    EnvVar {
                        name: "PASSWORD".into(),
                        value_from: Some(EnvVarSource::Secret {
                            name: secret.meta.id.to_string(),
                            key: "password".into(),
                        }),
                        ..EnvVar::default()
                    },
                ],
                ..Container::default()
            }],
            volumes: vec![
                PodVolume {
                    name: "cfg".into(),
                    source: VolumeSource::ConfigMap(cm.meta.id.to_string()),
                },
                PodVolume {
                    name: "sec".into(),
                    source: VolumeSource::Secret(secret.meta.name.clone()),
                },
                PodVolume {
                    name: "tok".into(),
                    source: VolumeSource::Secret(plain.meta.id.to_string()),
                },
            ],
            ..PodSpec::default()
        },
    );
    let mut sandbox = PodSandbox::from_pod(&pod);
    let debug = serde_json::to_value(&sandbox).unwrap();
    assert!(debug.get("config_maps").is_none());
    assert!(debug.get("secrets").is_none());
    assert!(!debug.to_string().contains("s3cret-value"));
    fill_projections(&store, &mut sandbox);
    let by_name = sandbox.config_maps.get("app").unwrap();
    let by_id = sandbox.config_maps.get(&cm.meta.id.to_string()).unwrap();
    assert_eq!(by_name.data.get("MODE").map(String::as_str), Some("prod"));
    assert_eq!(by_id.data.get("MODE").map(String::as_str), Some("prod"));
    assert_eq!(
        sandbox
            .secrets
            .get("db")
            .unwrap()
            .data
            .get("password")
            .map(String::as_str),
        Some("s3cret-value")
    );
    assert_eq!(
        sandbox
            .secrets
            .get(&secret.meta.id.to_string())
            .unwrap()
            .data
            .get("password")
            .map(String::as_str),
        Some("s3cret-value")
    );
    assert_eq!(
        sandbox
            .secrets
            .get("token")
            .unwrap()
            .data
            .get("data")
            .map(String::as_str),
        Some("plain-token")
    );
    assert_eq!(
        sandbox
            .secrets
            .get(&plain.meta.id.to_string())
            .unwrap()
            .data
            .get("data")
            .map(String::as_str),
        Some("plain-token")
    );
    let machine = serde_json::to_string(&sandbox).unwrap();
    assert!(machine.contains("s3cret-value"));
    assert!(machine.contains("plain-token"));
}

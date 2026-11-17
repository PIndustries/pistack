// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

#[test]
fn id_roundtrip() {
    let id = Id::new(kind::MACHINE);
    let s = id.to_string();
    assert!(s.starts_with("mach-"));
    let back: Id = serde_json::from_str(&format!("\"{s}\"")).unwrap();
    assert_eq!(back, id);
    assert!(Id::parse(kind::MACHINE, &s).is_some());
    assert!(Id::parse(kind::VOLUME, &s).is_none());
}

#[test]
fn resources_fit() {
    let want = Resources {
        vcpus: 2,
        memory_mib: 2048,
        disk_gib: 20,
        gpus: 0,
    };
    let free = Resources {
        vcpus: 8,
        memory_mib: 32768,
        disk_gib: 500,
        gpus: 0,
    };
    assert!(want.fits_in(&free));
    assert!(!free.fits_in(&want));
    assert_eq!(free.saturating_sub(&want).vcpus, 6);
}

#[test]
fn label_selector_parses() {
    let q = ListQuery {
        labels: Some("env=prod, tier=web".into()),
        ..Default::default()
    };
    let sel = q.label_selector();
    assert_eq!(sel.get("env").unwrap(), "prod");
    assert_eq!(sel.get("tier").unwrap(), "web");
}

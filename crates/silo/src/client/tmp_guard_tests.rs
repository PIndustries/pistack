// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::tmp_guard::TmpGuard;

#[test]
fn disarm_skips_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keep-me");
    std::fs::write(&path, b"hi").unwrap();
    let mut g = TmpGuard::new(path.clone());
    g.disarm();
    drop(g);
    assert!(path.exists(), "disarmed guard must not delete the path");
}

#[test]
fn drop_deletes_when_armed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remove-me");
    std::fs::write(&path, b"hi").unwrap();
    drop(TmpGuard::new(path.clone()));
    assert!(!path.exists(), "armed guard must delete on drop");
}

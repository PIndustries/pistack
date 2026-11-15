// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::mode_bits;
use super::super::mode_of;
use std::path::Path;

pub(crate) fn assert_layer_modes(root: &Path) {
    assert_eq!(mode_of(&root.join("bin").join("httpd")), 0o755);
    assert_eq!(
        std::fs::read(root.join("bin").join("httpd")).unwrap(),
        b"httpd"
    );
    assert_eq!(mode_of(&root.join("etc").join("config")), 0o644);
    assert_eq!(
        std::fs::read(root.join("etc").join("config")).unwrap(),
        b"cfg"
    );
    assert_eq!(mode_of(&root.join("rodir")), 0o555);
    assert_eq!(mode_of(&root.join("rodir").join("inside")), 0o644);
    assert_eq!(
        std::fs::read(root.join("rodir").join("inside")).unwrap(),
        b"in"
    );
    assert_eq!(mode_bits(&root.join("bin").join("suid")), 0o755);
    assert!(std::fs::symlink_metadata(root.join("linkbin"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_link(root.join("linkbin")).unwrap(),
        Path::new("usr/bin")
    );
    assert_eq!(
        std::fs::read(root.join("usr").join("bin").join("extra")).unwrap(),
        b"ex"
    );
    assert_eq!(mode_of(&root.join("usr").join("bin").join("extra")), 0o644);
    assert_eq!(
        std::fs::read(root.join("bin").join("httpd-hl")).unwrap(),
        b"httpd"
    );
    assert_eq!(mode_of(&root.join("bin").join("httpd-hl")), 0o755);
}

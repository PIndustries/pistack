// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn assert_device(name: &str) {
    assert_ne!(name, "pistack0", "{name}");
    assert!(!name.starts_with("pi"), "{name} starts with pi");
    assert!(name.len() <= 15, "{name}");
}

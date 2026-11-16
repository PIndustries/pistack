// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::volfs::canonical_fs::canonical_fs;

#[test]
fn filesystem_type_rejects_a_command_string() {
    assert_eq!(canonical_fs("").unwrap(), "ext4");
    assert_eq!(canonical_fs("  XFS ").unwrap(), "xfs");
    assert!(canonical_fs("ext4;rm").is_err());
    assert!(canonical_fs("../ext4").is_err());
    assert!(canonical_fs("e").is_err());
}

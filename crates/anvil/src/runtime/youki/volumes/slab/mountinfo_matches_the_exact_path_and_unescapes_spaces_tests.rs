// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::mountinfo_lists;
use crate::runtime::youki::volumes::unescape_mount;

#[test]
fn mountinfo_matches_the_exact_path_and_unescapes_spaces() {
    let text = "\
36 35 98:0 / /mnt/data rw,noatime master:1 - ext4 /dev/root rw\n\
37 35 98:0 / /mnt/my\\040vol rw - ext4 /dev/sda1 rw\n\
38 35 98:0 / /mnt/data-extra rw - ext4 /dev/sdb1 rw\n\
50 1 8:3 / /mnt/elsewhere rw - ext4 /var/lib/pistack/vol rw\n";
    assert!(mountinfo_lists(text, "/mnt/data"));
    assert!(mountinfo_lists(text, "/mnt/data/"));
    assert!(mountinfo_lists(text, "/mnt/my vol"));
    assert!(!mountinfo_lists(text, "/mnt"));
    assert!(!mountinfo_lists(text, "/mnt/data-extra/child"));
    assert!(!mountinfo_lists(text, "/mnt/missing"));
    // The device column is not a mount point.
    assert!(!mountinfo_lists(text, "/var/lib/pistack/vol"));
    assert_eq!(unescape_mount("my\\040vol"), "my vol");
    assert_eq!(unescape_mount("line\\012break"), "line\nbreak");
}

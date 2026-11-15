// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::file_plan::file_plan;
use crate::runtime::volfs::attach_plan::attach_plan;

#[test]
fn existing_filesystem_is_not_in_the_plan() {
    let plan = attach_plan(&file_plan(
        "/var/lib/pistack/volumes/disk.raw",
        "/dev/loop7",
        "/mnt/vol",
        "ext4",
        true,
        false,
    ))
    .unwrap();
    assert!(plan.iter().all(|step| !step[0].starts_with("mkfs")));
    assert_eq!(plan.len(), 3);

    let ro = attach_plan(&file_plan(
        "/var/lib/pistack/volumes/disk.raw",
        "/dev/loop7",
        "/mnt/vol",
        "ext4",
        true,
        true,
    ))
    .unwrap();
    assert_eq!(
        ro.last().unwrap()[1..],
        ["-o", "ro", "/dev/loop7", "/mnt/vol"]
    );
}

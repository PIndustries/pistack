// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::file_plan::file_plan;
use crate::runtime::volfs::attach_plan::attach_plan;
use crate::runtime::volfs::disk_kind::DiskKind;
use crate::runtime::volfs::mount_plan::MountPlan;

#[test]
fn command_plan_for_each_backend() {
    let local = attach_plan(&file_plan(
        "/var/lib/pistack/volumes/disk.raw",
        "/dev/loop7",
        "/var/lib/pistack/slab/mounts/vol-1",
        "ext4",
        false,
        false,
    ))
    .unwrap();
    assert_eq!(
        local,
        vec![
            vec![
                "losetup",
                "--find",
                "--show",
                "/var/lib/pistack/volumes/disk.raw"
            ],
            vec!["blkid", "-p", "-o", "value", "-s", "TYPE", "/dev/loop7"],
            vec!["mkfs.ext4", "-q", "-F", "/dev/loop7"],
            vec!["mount", "/dev/loop7", "/var/lib/pistack/slab/mounts/vol-1"],
        ]
        .into_iter()
        .map(|step| step.into_iter().map(str::to_owned).collect())
        .collect::<Vec<Vec<String>>>()
    );

    let lvm = attach_plan(&MountPlan {
        kind: DiskKind::Device,
        source: "/dev/vg0/vol-1",
        loop_device: "",
        mount_dir: "/var/lib/pistack/slab/mounts/vol-1",
        fs_type: "ext4",
        has_filesystem: false,
        read_only: false,
    })
    .unwrap();
    assert_eq!(lvm[0][0], "blkid");
    assert_eq!(lvm[0].last().map(String::as_str), Some("/dev/vg0/vol-1"));
    assert!(lvm.iter().all(|step| step[0] != "losetup"));
    assert_eq!(
        lvm.iter().find(|step| step[0] == "mkfs.ext4").unwrap()[1..],
        ["-q", "-F", "/dev/vg0/vol-1"]
    );
    assert_eq!(lvm.last().unwrap()[0], "mount");

    let rbd = attach_plan(&MountPlan {
        kind: DiskKind::Device,
        source: "/dev/rbd0",
        loop_device: "",
        mount_dir: "/var/lib/pistack/slab/mounts/vol-1",
        fs_type: "",
        has_filesystem: false,
        read_only: false,
    })
    .unwrap();
    assert_eq!(rbd[0].last().map(String::as_str), Some("/dev/rbd0"));
    assert_eq!(rbd[1][0], "mkfs.ext4");
    assert!(rbd.iter().all(|step| step[0] != "losetup"));

    let xfs = attach_plan(&MountPlan {
        kind: DiskKind::Device,
        source: "/dev/vg0/vol-1",
        loop_device: "",
        mount_dir: "/mnt/vol",
        fs_type: "xfs",
        has_filesystem: false,
        read_only: false,
    })
    .unwrap();
    assert_eq!(
        xfs.iter().find(|step| step[0] == "mkfs.xfs").unwrap()[1..],
        ["-f", "-q", "/dev/vg0/vol-1"]
    );
}

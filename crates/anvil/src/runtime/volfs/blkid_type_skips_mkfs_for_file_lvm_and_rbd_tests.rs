// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::guard::EuidGuard;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;
use crate::runtime::volfs::release_volume::release_volume;
use crate::runtime::volfs::volume_paths::volume_paths;

#[tokio::test]
async fn blkid_type_skips_mkfs_for_file_lvm_and_rbd() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let _euid = EuidGuard::root();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_string_lossy().into_owned();
    let req = attach(&file, "", false, false);
    mount_for_pod(&req, &file).await.unwrap();
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    assert!(
        log.iter().all(|step| !step[0].starts_with("mkfs")),
        "{log:?}"
    );
    release_volume(&req.volume).await.unwrap();

    let _ = std::fs::write(&tools.log, b"");
    let lvm = attach("/dev/vg0/vol-1", "ext4", false, false);
    mount_for_pod(&lvm, "/dev/vg0/vol-1").await.unwrap();
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    assert_eq!(log[0][0], "blkid");
    assert_eq!(log[0].last().map(String::as_str), Some("/dev/vg0/vol-1"));
    assert!(log.iter().all(|step| step[0] != "losetup"));
    assert!(log.iter().all(|step| !step[0].starts_with("mkfs")));
    release_volume(&lvm.volume).await.unwrap();

    let _ = std::fs::write(&tools.log, b"");
    let rbd = attach("", "ext4", false, true);
    let mounted = mount_for_pod(&rbd, "/dev/rbd0").await.unwrap();
    let (dir, _) = volume_paths(&rbd.volume).unwrap();
    assert_eq!(mounted, dir.to_string_lossy());
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    assert_eq!(log[0].last().map(String::as_str), Some("/dev/rbd0"));
    assert!(log
        .iter()
        .all(|step| step[0] != "losetup" && !step[0].starts_with("mkfs")));
}

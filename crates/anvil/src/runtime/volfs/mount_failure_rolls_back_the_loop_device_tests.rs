// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::guard::EuidGuard;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;
use crate::runtime::volfs::volume_paths::volume_paths;

#[tokio::test]
async fn mount_failure_rolls_back_the_loop_device() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let _euid = EuidGuard::root();
    std::fs::write(&tools.empty, b"1").unwrap();
    std::fs::write(&tools.mount_fail, b"1").unwrap();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_string_lossy().into_owned();
    let req = attach(&file, "ext4", false, false);
    let err = mount_for_pod(&req, &file).await.unwrap_err();
    assert!(err.message.contains("mount"), "{err}");
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    let names: Vec<&str> = log.iter().map(|step| step[0].as_str()).collect();
    assert_eq!(
        names,
        [
            "losetup",
            "blkid",
            "mkfs.ext4",
            "mount",
            "umount",
            "losetup"
        ]
    );
    assert_eq!(log.last().unwrap()[1..], ["-d", "/dev/loop7"]);
    let (dir, sidecar) = volume_paths(&req.volume).unwrap();
    assert!(!dir.exists());
    assert!(!sidecar.exists());
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::guard::EuidGuard;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;

#[tokio::test]
async fn mkfs_failure_does_not_mount_and_detaches_the_loop() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let _euid = EuidGuard::root();
    std::fs::write(&tools.empty, b"1").unwrap();
    std::fs::write(&tools.mkfs_fail, b"1").unwrap();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_string_lossy().into_owned();
    let req = attach(&file, "ext4", false, false);
    let err = mount_for_pod(&req, &file).await.unwrap_err();
    assert!(err.message.contains("mkfs"), "{err}");
    let names: Vec<String> = invocations(&std::fs::read_to_string(&tools.log).unwrap())
        .into_iter()
        .map(|step| step[0].clone())
        .collect();
    assert_eq!(
        names,
        ["losetup", "blkid", "mkfs.ext4", "losetup"]
            .map(str::to_owned)
            .to_vec()
    );
}

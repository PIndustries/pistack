// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::guard::EuidGuard;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::detach_plan::detach_plan;
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;
use crate::runtime::volfs::release_volume::release_volume;
use crate::runtime::volfs::volume_paths::volume_paths;

#[tokio::test]
async fn detach_is_ordered_and_idempotent_when_already_unmounted() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let _euid = EuidGuard::root();
    std::fs::write(&tools.empty, b"1").unwrap();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_string_lossy().into_owned();
    let req = attach(&file, "ext4", false, false);
    mount_for_pod(&req, &file).await.unwrap();
    let _ = std::fs::write(&tools.log, b"");
    release_volume(&req.volume).await.unwrap();
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    let (dir, _) = volume_paths(&req.volume).unwrap();
    assert_eq!(log, detach_plan(dir.to_str().unwrap(), Some("/dev/loop7")));
    let before = std::fs::read_to_string(&tools.log).unwrap();
    release_volume(&req.volume).await.unwrap();
    assert_eq!(std::fs::read_to_string(&tools.log).unwrap(), before);

    let (dir, sidecar) = volume_paths(&req.volume).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&sidecar, "/dev/loop7\n").unwrap();
    std::fs::write(&tools.not_mounted, b"1").unwrap();
    let _ = std::fs::write(&tools.log, b"");
    release_volume(&req.volume).await.unwrap();
    let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
    assert_eq!(log[0][0], "umount");
    assert_eq!(log[1][1..], ["-d", "/dev/loop7"]);
    assert!(!sidecar.exists());
}

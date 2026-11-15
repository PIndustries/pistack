// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::file_plan::file_plan;
use super::test_support::guard::EuidGuard;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::attach_plan::attach_plan;
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;
use crate::runtime::volfs::volume_paths::volume_paths;

#[tokio::test]
async fn local_file_formats_and_records_the_mount_directory() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let _euid = EuidGuard::root();
    std::fs::write(&tools.empty, b"1").unwrap();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_str().unwrap();
    let req = attach(file, "ext4", false, false);
    let mounted = mount_for_pod(&req, file).await.unwrap();
    let (dir, _) = volume_paths(&req.volume).unwrap();
    assert_eq!(mounted, dir.to_string_lossy());
    assert!(dir.is_dir());
    let plan = attach_plan(&file_plan(
        file,
        "/dev/loop7",
        dir.to_str().unwrap(),
        "ext4",
        false,
        false,
    ))
    .unwrap();
    assert_eq!(
        invocations(&std::fs::read_to_string(&tools.log).unwrap()),
        plan
    );
}

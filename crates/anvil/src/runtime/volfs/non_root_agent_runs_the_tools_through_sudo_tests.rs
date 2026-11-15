// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::test_support::apply_tools::apply_tools;
use super::test_support::attach::attach;
use super::test_support::install::install;
use super::test_support::invocations::invocations;
use crate::runtime::volfs::consts::{SUDO_BIN_ENV, SUDO_ENV};
use crate::runtime::volfs::env_lock::env_lock;
use crate::runtime::volfs::mount_for_pod::mount_for_pod;
use crate::runtime::volfs::release_volume::release_volume;

#[tokio::test]
async fn non_root_agent_runs_the_tools_through_sudo() {
    let _lock = env_lock().lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let tools = install(tmp.path());
    let data = tmp.path().join("data");
    let _env = apply_tools(&tools, &data);
    let saved_euid = std::env::var_os("PISTACK_VOLFS_TEST_EUID");
    let saved_sudo = std::env::var_os(SUDO_ENV);
    let saved_sudo_bin = std::env::var_os(SUDO_BIN_ENV);
    std::env::set_var("PISTACK_VOLFS_TEST_EUID", "1000");
    std::env::set_var(SUDO_ENV, "1");
    std::env::set_var(SUDO_BIN_ENV, &tools.sudo);
    std::fs::write(&tools.empty, b"1").unwrap();
    let file = tmp.path().join("disk.raw");
    std::fs::write(&file, b"raw").unwrap();
    let file = file.to_string_lossy().into_owned();
    let req = attach(&file, "ext4", false, false);
    let mounted = mount_for_pod(&req, &file).await.unwrap();
    assert!(mounted.contains("/slab/mounts/"));
    let sudo_log = invocations(&std::fs::read_to_string(&tools.sudo_log).unwrap());
    let elevated: Vec<&str> = sudo_log
        .iter()
        .filter_map(|step| step.get(2).map(String::as_str))
        .collect();
    assert!(sudo_log
        .iter()
        .all(|step| step.get(1).map(String::as_str) == Some("-n")));
    assert!(elevated.iter().any(|path| path.ends_with("losetup")));
    assert!(elevated.iter().any(|path| path.ends_with("blkid")));
    assert!(elevated.iter().any(|path| path.ends_with("mkfs.ext4")));
    assert!(elevated.iter().any(|path| path.ends_with("mount")));
    release_volume(&req.volume).await.unwrap();
    let sudo_log = invocations(&std::fs::read_to_string(&tools.sudo_log).unwrap());
    assert!(sudo_log.iter().any(|step| {
        step.get(2).is_some_and(|path| path.ends_with("umount"))
            && step.get(1).map(String::as_str) == Some("-n")
    }));

    std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
    let _ = std::fs::write(&tools.sudo_log, b"");
    let _ = std::fs::write(&tools.log, b"");
    mount_for_pod(&req, &file).await.unwrap();
    assert_eq!(std::fs::read_to_string(&tools.sudo_log).unwrap(), "");
    assert!(!std::fs::read_to_string(&tools.log).unwrap().is_empty());

    match saved_euid {
        Some(value) => std::env::set_var("PISTACK_VOLFS_TEST_EUID", value),
        None => std::env::remove_var("PISTACK_VOLFS_TEST_EUID"),
    }
    match saved_sudo {
        Some(value) => std::env::set_var(SUDO_ENV, value),
        None => std::env::remove_var(SUDO_ENV),
    }
    match saved_sudo_bin {
        Some(value) => std::env::set_var(SUDO_BIN_ENV, value),
        None => std::env::remove_var(SUDO_BIN_ENV),
    }
}

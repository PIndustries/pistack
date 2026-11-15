// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::plain_rootfs;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
async fn create_deletes_a_leftover_container_before_create() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("leftover");
    let _dir = scopeguard_dir(&tmp);
    let log = tmp.join("youki.log");
    let bin = tmp.join("youki");
    write_script(&bin, &log, 0);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&bin, &state);
    let rt = YoukiRuntime;
    let request = plain_rootfs(&tmp);
    let track = tmp.join("youki-ids");
    std::fs::create_dir_all(&track).unwrap();
    std::fs::write(track.join(request.id.to_string()), b"").unwrap();
    rt.start(&request).await.unwrap();
    let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
    let delete_at = lines
        .windows(2)
        .position(|w| w[0] == "delete" && w[1] == "--force");
    let create_at = lines.iter().position(|line| line == "create");
    assert!(
        delete_at.is_some() && create_at.is_some() && delete_at.unwrap() < create_at.unwrap(),
        "{lines:?}"
    );
    assert!(state
        .join(request.id.to_string())
        .join("state.json")
        .is_file());
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;
use pistack_types::error::ErrorCode;

use crate::runtime::youki::consts::DEFAULT_STATE;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::state_dir::state_dir;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::plain_rootfs;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::youki::test_support::EnvSlot;
use std::path::PathBuf;

#[tokio::test]
pub(super) async fn state_dir_follows_data_dir_and_names_an_unwritable_path() {
    let _guard = env_lock().lock().await;
    let _state = EnvSlot::clear("PISTACK_YOUKI_STATE");
    let _data = EnvSlot::clear("PISTACK_DATA_DIR");
    assert_eq!(state_dir(), PathBuf::from(DEFAULT_STATE));
    let data = PathBuf::from("/tmp/pistack-data-example");
    std::env::set_var("PISTACK_DATA_DIR", &data);
    assert_eq!(state_dir(), data.join("youki"));
    std::env::set_var("PISTACK_YOUKI_STATE", "");
    assert_eq!(state_dir(), data.join("youki"));
    std::env::set_var("PISTACK_YOUKI_STATE", "/var/lib/custom-youki");
    assert_eq!(state_dir(), PathBuf::from("/var/lib/custom-youki"));

    let tmp = temp_dir("statedir");
    let _dir = scopeguard_dir(&tmp);
    let log = tmp.join("youki.log");
    let bin = tmp.join("youki");
    write_script(&bin, &log, 0);
    let blocker = tmp.join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let _env = EnvGuard::set(&bin, &blocker);
    let rt = YoukiRuntime;
    let request = plain_rootfs(&tmp);
    let err = rt.start(&request).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(
        err.message.contains(&blocker.display().to_string()),
        "{err}"
    );
    assert!(err.message.contains("PISTACK_YOUKI_STATE"), "{err}");
    assert!(err.message.contains("PISTACK_DATA_DIR"), "{err}");
    assert!(err.message.contains("agent user owns"), "{err}");
    assert!(
        std::fs::read_to_string(&log).unwrap_or_default().is_empty(),
        "a rejected state dir must not call youki"
    );
}

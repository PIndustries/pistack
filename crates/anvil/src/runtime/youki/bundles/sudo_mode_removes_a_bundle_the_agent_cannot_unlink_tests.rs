// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::bundles::checked_removal_path;
use crate::runtime::youki::bundles::privileged_remove;
use crate::runtime::youki::consts::CLEANUP_ENV;
use crate::runtime::youki::consts::SUDO_BIN_ENV;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::install_cleanup_helper;
use crate::runtime::youki::test_support::lock_dir;
use crate::runtime::youki::test_support::mode_of;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::write_sudo;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::youki::test_support::Unlock;

#[tokio::test]
async fn sudo_mode_removes_a_bundle_the_agent_cannot_unlink() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("sudo-rm");
    let _dir = scopeguard_dir(&tmp);
    let youki_log = tmp.join("youki.log");
    let sudo_log = tmp.join("sudo.log");
    let youki = tmp.join("youki");
    let sudo_bin = tmp.join("sudo");
    write_script(&youki, &youki_log, 0);
    write_sudo(&sudo_bin, &sudo_log);
    let helper = install_cleanup_helper(&tmp);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
    std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
    std::env::set_var(SUDO_ENV, "1");
    std::env::set_var(SUDO_BIN_ENV, &sudo_bin);
    std::env::set_var(CLEANUP_ENV, &helper);
    std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
    let privilege = Privilege::Sudo {
        sudo: sudo_bin.clone(),
    };

    let outside = tmp.join("outside");
    std::fs::write(&outside, b"keep").unwrap();
    let sibling = state.join("sibling-keep");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("file"), b"keep").unwrap();

    let rootfs = state.join("mach-1").join("rootfs");
    let locked_rootfs = rootfs.join("locked");
    lock_dir(&locked_rootfs);
    {
        let _unlock = Unlock(locked_rootfs.clone());
        assert!(std::fs::remove_file(locked_rootfs.join("file")).is_err());
        reset_dir(&rootfs, &privilege).unwrap();
        assert!(rootfs.is_dir());
        assert!(!locked_rootfs.exists());
        assert!(std::fs::read_dir(&rootfs).unwrap().next().is_none());
    }
    assert_eq!(std::fs::read(sibling.join("file")).unwrap(), b"keep");
    let helper_arg = helper.to_string_lossy().into_owned();
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
    assert!(
        sudo_text.contains(&helper_arg),
        "rootfs removal did not use the helper: {sudo_text}"
    );
    assert!(
        !sudo_text.contains(".."),
        "privileged removal saw a dot-dot path: {sudo_text}"
    );

    let image = tmp.join("image");
    std::fs::create_dir_all(&image).unwrap();
    std::fs::write(image.join("marker"), b"keep").unwrap();
    let rt = YoukiRuntime;
    let request = req("web", image.to_str().unwrap(), "10.8.1.1");
    rt.start(&request).await.unwrap();
    let bundle = state.join(request.id.to_string());
    assert_eq!(mode_of(&bundle), 0o700);
    let locked = bundle.join("rootfs").join("locked");
    lock_dir(&locked);
    let _unlock = Unlock(locked.clone());
    assert!(std::fs::remove_file(locked.join("file")).is_err());
    rt.stop(&request.id).await.unwrap();
    rt.delete(&request.id).await.unwrap();
    assert!(!bundle.exists());
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    assert_eq!(std::fs::read(sibling.join("file")).unwrap(), b"keep");
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
    let bundle_arg = bundle.to_string_lossy().into_owned();
    assert!(
        sudo_text.contains(&bundle_arg),
        "delete did not remove the bundle through sudo: {sudo_text}"
    );
    let lines = arg_lines(&sudo_text);
    assert!(
        lines.windows(4).any(|w| {
            w[0] == "-n" && w[1] == helper_arg && w[2] == "delete" && w[3] == "--path"
        }),
        "{lines:?}"
    );
    let youki_lines = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
    assert!(
        youki_lines
            .windows(2)
            .any(|w| w[0] == "delete" && w[1] == "--force"),
        "{youki_lines:?}"
    );

    let _ = std::fs::write(&sudo_log, b"");
    let plain = req("plain", image.to_str().unwrap(), "10.8.1.2");
    rt.start(&plain).await.unwrap();
    rt.delete(&plain.id).await.unwrap();
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
    assert!(
        !sudo_text.contains(&helper_arg),
        "agent-owned bundle used the helper: {sudo_text}"
    );
    assert!(sudo_text.contains("delete"), "{sudo_text}");

    let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let _ = std::fs::write(&sudo_log, b"");
    let tricky = state
        .join("..")
        .join(tmp.file_name().unwrap())
        .join("outside");
    let err = privileged_remove(&tricky, &privilege, &cause).unwrap_err();
    assert!(err.message.contains("Refusing privileged removal"), "{err}");
    assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    assert!(checked_removal_path(&state.join("abc-1")).is_ok());
    assert!(checked_removal_path(&state).is_err());
    assert!(checked_removal_path(&outside).is_err());
    assert!(checked_removal_path(&tricky).is_err());

    std::env::remove_var(SUDO_ENV);
    std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
    let rootless = req("less", image.to_str().unwrap(), "10.8.1.3");
    rt.start(&rootless).await.unwrap();
    let less_bundle = state.join(rootless.id.to_string());
    let less_locked = less_bundle.join("rootfs").join("locked");
    lock_dir(&less_locked);
    let _less_unlock = Unlock(less_locked);
    let err = rt.delete(&rootless.id).await.unwrap_err();
    assert!(err.message.contains("removing"), "{err}");
    assert!(less_bundle.exists());
    assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");

    std::env::set_var(SUDO_ENV, "1");
    std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
    std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
    let _ = std::fs::write(&sudo_log, b"");
    let rooted = req("root", image.to_str().unwrap(), "10.8.1.4");
    rt.start(&rooted).await.unwrap();
    rt.delete(&rooted.id).await.unwrap();
    assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
}

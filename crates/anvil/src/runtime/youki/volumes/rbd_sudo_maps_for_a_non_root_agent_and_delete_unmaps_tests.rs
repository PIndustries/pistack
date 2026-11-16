// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::count_rbd;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::rbd_attach;
use crate::runtime::youki::test_support::rbd_invocations;
use crate::runtime::youki::test_support::rbd_target;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::write_rbd;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::write_sudo;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::youki::volumes::read_disks;
use pistack_types::kind;
use pistack_types::Id;

#[tokio::test]
async fn rbd_sudo_maps_for_a_non_root_agent_and_delete_unmaps() {
    let _guard = env_lock().lock().await;
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-rbd-sudo-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let youki_log = tmp.join("youki.log");
    let youki = tmp.join("youki");
    write_script(&youki, &youki_log, 0);
    let rbd_log = tmp.join("rbd.log");
    let rbd_state = tmp.join("mapped.json");
    let rbd = tmp.join("rbd");
    write_rbd(&rbd, &rbd_log, &rbd_state);
    let sudo_log = tmp.join("sudo.log");
    let sudo = tmp.join("sudo");
    write_sudo(&sudo, &sudo_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_RBD", &rbd);
    std::env::set_var("PISTACK_RBD_SUDO", "1");
    std::env::set_var("PISTACK_RBD_SUDO_BIN", &sudo);
    std::env::set_var("PISTACK_RBD_TEST_EUID", "1000");

    let keyring = tmp.join("keyring");
    std::fs::write(&keyring, b"key-bytes").unwrap();
    let rt = YoukiRuntime;
    let machine = Id::new(kind::MACHINE);
    let volume = Id::new(kind::VOLUME);
    let bundle = state.join(machine.to_string());
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("config.json"), b"{}").unwrap();
    let attach = rbd_attach(
        machine.clone(),
        volume.clone(),
        rbd_target(Some(keyring.to_string_lossy().into_owned())),
    );
    assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
    assert_eq!(read_disks(&bundle).unwrap()[0].path, "/dev/rbd0");

    let rbd_arg = rbd.to_string_lossy().into_owned();
    let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
    assert!(
        sudo_lines
            .windows(4)
            .any(|w| { w[0] == "-n" && w[1] == rbd_arg && w[2] == "device" && w[3] == "map" }),
        "map was not run through sudo: {sudo_lines:?}"
    );
    assert!(sudo_lines
        .iter()
        .all(|line| line != "list" && line != "showmapped"));
    let direct = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
    assert!(count_rbd(&direct, "device", "list") >= 1);

    rt.delete(&machine).await.unwrap();
    assert!(!bundle.exists());
    let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
    assert!(
        sudo_lines
            .windows(4)
            .any(|w| { w[0] == "-n" && w[1] == rbd_arg && w[2] == "device" && w[3] == "unmap" }),
        "unmap was not run through sudo: {sudo_lines:?}"
    );

    std::env::set_var("PISTACK_RBD_TEST_EUID", "0");
    let _ = std::fs::write(&sudo_log, b"");
    let _ = std::fs::write(&rbd_log, b"");
    let root_machine = Id::new(kind::MACHINE);
    let root_bundle = state.join(root_machine.to_string());
    std::fs::create_dir_all(&root_bundle).unwrap();
    std::fs::write(root_bundle.join("config.json"), b"{}").unwrap();
    let root_attach = rbd_attach(
        root_machine,
        volume,
        rbd_target(Some(keyring.to_string_lossy().into_owned())),
    );
    assert_eq!(rt.attach_volume(&root_attach).await.unwrap(), "vdb");
    assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
    let direct = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
    assert_eq!(count_rbd(&direct, "device", "map"), 1);
    assert!(direct.iter().all(|args| args.iter().all(|arg| arg != "-n")));
}

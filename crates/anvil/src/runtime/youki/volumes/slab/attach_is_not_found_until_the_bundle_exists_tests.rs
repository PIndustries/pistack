// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use pistack_types::error::ErrorCode;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::youki::volumes::read_disks;
use pistack_types::kind;
use pistack_types::Id;

#[tokio::test]
async fn attach_is_not_found_until_the_bundle_exists() {
    let _guard = env_lock().lock().await;
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-attach-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let _env = EnvGuard::set(&tmp.join("youki"), &tmp.join("state"));
    let rt = YoukiRuntime;
    let machine = Id::new(kind::MACHINE);
    let volume = Id::new(kind::VOLUME);
    let attach = crate::runtime::VolumeAttach {
        machine: machine.clone(),
        volume: volume.clone(),
        path: "/var/lib/pistack/volumes/disk".into(),
        device: None,
        read_only: false,
        fs_type: String::new(),
        target: None,
    };
    assert_eq!(
        rt.attach_volume(&attach).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    let bundle = tmp.join("state").join(machine.to_string());
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("config.json"), b"{}").unwrap();
    let mut bad = attach.clone();
    bad.path = "relative".into();
    assert_eq!(
        rt.attach_volume(&bad).await.unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
    let mut moved = attach.clone();
    moved.path = "/var/lib/pistack/volumes/disk2".into();
    assert_eq!(rt.attach_volume(&moved).await.unwrap(), "vdb");
    let mut second = attach.clone();
    second.volume = Id::new(kind::VOLUME);
    assert_eq!(rt.attach_volume(&second).await.unwrap(), "vdc");
    let mut named = attach.clone();
    named.volume = Id::new(kind::VOLUME);
    named.device = Some("vdk".into());
    assert_eq!(rt.attach_volume(&named).await.unwrap(), "vdk");
    named.device = Some("vda".into());
    named.volume = Id::new(kind::VOLUME);
    assert_eq!(
        rt.attach_volume(&named).await.unwrap_err().code,
        ErrorCode::BadRequest
    );
    rt.detach_volume(&crate::runtime::VolumeDetach {
        machine: machine.clone(),
        volume,
        path: attach.path.clone(),
        device: None,
        target: None,
    })
    .await
    .unwrap();
    let disks = read_disks(&bundle).unwrap();
    assert!(disks.iter().all(|d| d.device != "vdb"));
    assert_eq!(disks.len(), 2);
    rt.detach_volume(&crate::runtime::VolumeDetach {
        machine,
        volume: second.volume,
        path: second.path,
        device: None,
        target: None,
    })
    .await
    .unwrap();
}

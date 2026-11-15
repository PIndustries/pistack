// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::test_support::attached_raw;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::inject_mountinfo;
use crate::runtime::youki::test_support::mount_at;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::slab_plan;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::EnvSlot;
use crate::runtime::youki::volumes::pod_binds;
use crate::runtime::youki::volumes::read_disks;
use crate::runtime::youki::volumes::refresh_disk_mounts;
use crate::runtime::youki::volumes::write_disks;

#[tokio::test]
async fn late_attach_and_recreate_bind_the_mounted_folder() {
    let _guard = env_lock().lock().await;
    let _netns = EnvSlot::clear(NETNS_ENV);
    let tmp = temp_dir("slab-late");
    let _dir = scopeguard_dir(&tmp);
    let image = tmp.join("image");
    std::fs::create_dir_all(&image).unwrap();
    std::fs::write(image.join("marker"), "a").unwrap();
    let folder = tmp.join("mounted");
    std::fs::create_dir_all(&folder).unwrap();
    let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
    let raw = tmp.join("disk.raw");
    std::fs::write(&raw, b"block").unwrap();
    let vol_id = "vol-44444444-4444-4444-4444-444444444444";
    let folder_s = folder.to_string_lossy().into_owned();
    let raw_s = raw.to_string_lossy().into_owned();
    let image_s = image.to_string_lossy().into_owned();
    let waiting = slab_plan(&image_s, vol_id, None, Some("vdb"));
    let bundle = tmp.join("pod");
    let limits = Limits {
        vcpus: 1,
        memory_mib: 64,
    };
    RunMode::privileged()
        .stage_pod_bundles(&bundle, "web", &limits, &waiting, &image_s, 7)
        .unwrap();
    let web: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(web["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|mount| mount["destination"] != "/data"));

    write_disks(&bundle, &[attached_raw(vol_id, &raw_s, false)]).unwrap();
    refresh_disk_mounts(&bundle).unwrap();
    let web: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(web["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|mount| mount["destination"] != "/data"));

    let ready = slab_plan(&image_s, vol_id, Some(&folder_s), None);
    let disks = read_disks(&bundle).unwrap();
    let binds = pod_binds(&bundle, &ready, &ready.containers[0].volume_mounts, &disks).unwrap();
    assert_eq!(binds[0].source, folder_s);
    assert!(binds[0].directory);
    RunMode::privileged()
        .stage_pod_bundles(&bundle, "web", &limits, &ready, &image_s, 7)
        .unwrap();
    let web: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
    )
    .unwrap();
    let data = mount_at(&web, "/data");
    assert_eq!(data["source"], folder_s);
    assert_eq!(
        data["options"],
        serde_json::json!(["rbind", "rprivate", "rw"])
    );

    RunMode::privileged()
        .stage_pod_bundles(&bundle, "web", &limits, &ready, &image_s, 7)
        .unwrap();
    let web: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
    )
    .unwrap();
    let data = mount_at(&web, "/data");
    assert_eq!(data["source"], folder_s);
    assert_eq!(
        data["options"],
        serde_json::json!(["rbind", "rprivate", "rw"])
    );
}

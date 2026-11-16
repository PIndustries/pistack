// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn unmounted_slab_folder_is_refused() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("slab-unmounted");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let empty = tmp.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let table = tmp.join("mountinfo");
        let parent = empty
            .parent()
            .unwrap()
            .to_string_lossy()
            .replace(' ', "\\040");
        std::fs::write(
            &table,
            format!("41 1 8:1 / {parent} rw - ext4 /dev/disk rw\n"),
        )
        .unwrap();
        let _mounts = EnvSlot::set(MOUNTINFO_ENV, table.to_str().expect("utf8"));
        let vol_id = "vol-66666666-6666-6666-6666-666666666666";
        let empty_s = empty.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&empty_s), None);
        let expected = format!("slab volume disk is not mounted at {empty_s}");
        let err = ensure_slab_sources(&plan, &[]).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains(&expected), "{err}");
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        let err = RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap_err();
        assert!(err.message.contains(&expected), "{err}");
        assert!(!bundle.join("config.json").exists());
        assert!(!bundle.join("containers").exists());

        let disks = vec![attached_raw(vol_id, &empty_s, false)];
        assert!(oci_binds(
            &bundle,
            &plan.volumes,
            &plan.containers[0].volume_mounts,
            &disks,
            &plan.volume_paths,
        )
        .is_empty());

        // The mount table cannot be read, so the device-number fallback
        // still refuses a folder that shares its parent's device.
        let missing = tmp.join("no-such-mountinfo");
        let _fallback = EnvSlot::set(MOUNTINFO_ENV, missing.to_str().expect("utf8"));
        let err = match pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]) {
            Err(err) => err,
            Ok(_) => panic!("an unmounted folder was bound"),
        };
        assert!(err.message.contains(&expected), "{err}");
        let late = slab_plan(&image_s, vol_id, Some(&empty_s), None);
        let err = RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &late, &image_s, 7)
            .unwrap_err();
        assert!(err.message.contains(&expected), "{err}");
    }

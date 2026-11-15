// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn mounted_slab_folder_is_accepted() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("slab-mounted");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let folder = tmp.join("my vol");
        std::fs::create_dir_all(&folder).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
        let vol_id = "vol-55555555-5555-5555-5555-555555555555";
        let folder_s = folder.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&folder_s), None);
        ensure_slab_sources(&plan, &[]).unwrap();
        let bundle = tmp.join("pod");
        let binds = pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]).unwrap();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].source, folder_s);
        assert!(binds[0].directory);
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
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

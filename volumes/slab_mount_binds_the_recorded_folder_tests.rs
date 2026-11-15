// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn slab_mount_binds_the_recorded_folder() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = temp_dir("slab-folder");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let folder = tmp.join("mounted");
        std::fs::create_dir_all(&folder).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
        let raw = tmp.join("disk.raw");
        std::fs::write(&raw, b"block").unwrap();
        let vol_id = "vol-33333333-3333-3333-3333-333333333333";
        let folder_s = folder.to_string_lossy().into_owned();
        let raw_s = raw.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&folder_s), None);
        let disks = vec![attached_raw(vol_id, &raw_s, false)];
        let bundle = tmp.join("pod");
        let binds = pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &disks).unwrap();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].source, folder_s);
        assert!(binds[0].directory);
        assert!(!binds[0].read_only);
        assert_ne!(binds[0].source, raw_s);

        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap();
        write_disks(&bundle, &disks).unwrap();
        let late = vec![attached_raw(vol_id, &raw_s, true)];
        write_disks(&bundle, &late).unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );

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
            serde_json::json!(["rbind", "rprivate", "ro"])
        );

        let file_plan = slab_plan(&image_s, vol_id, Some(&raw_s), None);
        let err = match pod_binds(
            &bundle,
            &file_plan,
            &file_plan.containers[0].volume_mounts,
            &[],
        ) {
            Err(err) => err,
            Ok(_) => panic!("a raw file was bound"),
        };
        assert!(err.message.contains("disk"), "{err}");
        assert!(err.message.contains(&raw_s), "{err}");
        assert!(err.message.contains("is a file"), "{err}");
        assert!(err.message.contains("not a mounted directory"), "{err}");

        let backing = match pod_binds(
            &bundle,
            &slab_plan(&image_s, vol_id, None, None),
            &plan.containers[0].volume_mounts,
            &disks,
        ) {
            Err(err) => err,
            Ok(_) => panic!("the backing file was bound"),
        };
        assert!(backing.message.contains("disk"), "{backing}");
        assert!(backing.message.contains(&raw_s), "{backing}");
        assert!(backing.message.contains("is a file"), "{backing}");

        let block = slab_source_error("disk", "/dev/vg/lv", SlabSource::Block);
        assert!(block.message.contains("disk"), "{block}");
        assert!(block.message.contains("block device"), "{block}");

        for path in ["/dev/disk0", "/dev/sda", "/dev/vda", "/dev/nvme0n1"] {
            if classify_slab_path(path).ok() != Some(SlabSource::Block) {
                continue;
            }
            let plan = slab_plan(&image_s, vol_id, Some(path), None);
            let err = match pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]) {
                Err(err) => err,
                Ok(_) => panic!("block device {path} was bound"),
            };
            assert!(err.message.contains("disk"), "{err}");
            assert!(err.message.contains(path), "{err}");
            assert!(err.message.contains("block device"), "{err}");
            break;
        }
    }

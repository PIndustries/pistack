// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn pod_bundles_mount_every_container() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-pod-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let image_a = tmp.join("image-a");
        let image_b = tmp.join("image-b");
        std::fs::create_dir_all(image_a.join("bin")).unwrap();
        std::fs::create_dir_all(&image_b).unwrap();
        std::fs::write(image_a.join("marker"), "a").unwrap();
        std::fs::write(image_b.join("marker"), "b").unwrap();
        let mounted = tmp.join("slab-mount");
        std::fs::create_dir_all(&mounted).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[mounted.as_path()]);
        let raw = tmp.join("slab-file");
        std::fs::write(&raw, "block").unwrap();
        let vol_id = "vol-11111111-1111-1111-1111-111111111111";
        let user_data = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [
                {
                    "name": "web",
                    "image": image_a,
                    "command": ["/usr/bin/web"],
                    "args": ["--serve"],
                    "working_dir": "/work",
                    "env": [
                        {"name": "PASSWORD", "value_from": {"secret": {"name": "db", "key": "password"}}}
                    ],
                    "volume_mounts": [
                        {"name": "scratch", "mount_path": "/tmp/scratch"},
                        {"name": "cfg", "mount_path": "/etc/cfg", "read_only": true},
                        {"name": "cfg", "mount_path": "/etc/hello", "sub_path": "hello", "read_only": true},
                        {"name": "sec", "mount_path": "/run/sec", "read_only": true},
                        {"name": "disk", "mount_path": "/data"},
                        {"name": "scratch", "mount_path": "/tmp/../escape"},
                        {"name": "cfg", "mount_path": "/etc/slip", "sub_path": "../escape"}
                    ]
                },
                {
                    "name": "side",
                    "image": image_b,
                    "command": ["/usr/bin/side"],
                    "args": ["--watch"],
                    "volume_mounts": [
                        {"name": "scratch", "mount_path": "/cache", "read_only": true}
                    ]
                }
            ],
            "volumes": [
                {"name": "scratch", "source": "empty_dir"},
                {"name": "cfg", "source": {"config_map": "app"}},
                {"name": "sec", "source": {"secret": "db"}},
                {"name": "disk", "source": {"volume": vol_id}}
            ],
            "config_maps": {
                "app": {
                    "data": {"MODE": "prod", "hello": "world"},
                    "binary_data": {"MODE": "aWdub3JlZA==", "blob": "Ymlu"}
                }
            },
            "secrets": {
                "db": {"data": {"password": "s3cret-value"}}
            },
            "volume_paths": { vol_id: mounted.to_string_lossy() }
        })
        .to_string();
        let mut request = req("web", "not-a-path", "10.9.0.1");
        request.user_data = Some(user_data);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        let privileged = RunMode::privileged();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        let scratch = bundle.join("volumes").join("0-scratch");
        std::fs::write(scratch.join("keep-me"), "yes").unwrap();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(scratch.join("keep-me")).unwrap(),
            "yes"
        );

        let cfg_dir = bundle.join("volumes").join("1-cfg");
        let sec_dir = bundle.join("volumes").join("2-sec");
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("MODE")).unwrap(),
            "prod"
        );
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("hello")).unwrap(),
            "world"
        );
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("blob")).unwrap(),
            "bin"
        );
        assert_eq!(mode_of(&cfg_dir.join("MODE")), 0o644);
        assert_eq!(
            std::fs::read_to_string(sec_dir.join("password")).unwrap(),
            "s3cret-value"
        );
        assert_eq!(mode_of(&sec_dir.join("password")), 0o400);
        assert_eq!(mode_of(&sec_dir), 0o700);
        std::fs::write(cfg_dir.join("extra"), "gone").unwrap();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert!(!cfg_dir.join("extra").exists());
        assert_eq!(
            std::fs::read_to_string(scratch.join("keep-me")).unwrap(),
            "yes"
        );

        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let side: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("1").join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(
            side["process"]["args"],
            serde_json::json!(["/usr/bin/side", "--watch"])
        );
        assert_eq!(
            ns_path(&web, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert_eq!(
            ns_path(&side, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert!(web.get("hostname").is_none());
        let sandbox: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert!(sandbox["linux"]["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("path").is_none()));
        assert_eq!(
            std::fs::read_to_string(
                bundle
                    .join("containers")
                    .join("0")
                    .join("rootfs")
                    .join("marker")
            )
            .unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(
                bundle
                    .join("containers")
                    .join("1")
                    .join("rootfs")
                    .join("marker")
            )
            .unwrap(),
            "b"
        );
        let env = web["process"]["env"].as_array().unwrap();
        assert!(env.iter().any(|v| v == "PASSWORD=s3cret-value"));
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
        let scratch_mount = mount_at(&web, "/tmp/scratch");
        assert_eq!(scratch_mount["source"], scratch.to_string_lossy().as_ref());
        assert_eq!(
            scratch_mount["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
        let hello = mount_at(&web, "/etc/hello");
        assert_eq!(
            hello["source"],
            cfg_dir.join("hello").to_string_lossy().as_ref()
        );
        assert_eq!(
            hello["options"],
            serde_json::json!(["bind", "rprivate", "ro"])
        );
        let sec = mount_at(&web, "/run/sec");
        assert_eq!(
            sec["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        let cache = mount_at(&side, "/cache");
        assert_eq!(cache["source"], scratch.to_string_lossy().as_ref());
        assert_eq!(
            cache["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        assert!(web["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["destination"] != "/tmp/../escape" && m["destination"] != "/etc/slip"));
        let plan_text = std::fs::read_to_string(bundle.join("mount-plan.json")).unwrap();
        assert!(!plan_text.contains("s3cret-value"));
        assert!(!plan_text.contains("do-not-log-user-data"));
        assert!(!std::fs::read_to_string(bundle.join("config.json"))
            .unwrap()
            .contains("s3cret-value"));

        write_disks(
            &bundle,
            &[AttachedDisk {
                volume: vol_id.into(),
                path: raw.to_string_lossy().into_owned(),
                device: "vdb".into(),
                read_only: true,
                rbd_mapped: true,
            }],
        )
        .unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        assert_ne!(data["source"], raw.to_string_lossy().as_ref());
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(
            mode_of(&bundle.join("containers").join("0").join("config.json")),
            0o600
        );
    }

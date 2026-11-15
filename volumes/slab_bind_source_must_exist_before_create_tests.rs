// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn slab_bind_source_must_exist_before_create() {
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-slab-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let missing = tmp.join("missing-slab");
        let vol_id = "vol-22222222-2222-2222-2222-222222222222";
        let user_data = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "name": "web",
                "volume_mounts": [{"name": "disk", "mount_path": "/data"}]
            }],
            "volumes": [{"name": "disk", "source": {"volume": vol_id}}],
            "volume_paths": { vol_id: missing.to_string_lossy() }
        })
        .to_string();
        let mut request = req("web", "/nope", "10.9.0.1");
        request.user_data = Some(user_data);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let bundle = tmp.join("pod");
        let err = RunMode::privileged()
            .stage_pod_bundles(
                &bundle,
                "web",
                &Limits {
                    vcpus: 1,
                    memory_mib: 64,
                },
                &plan,
                "/nope",
                1,
            )
            .unwrap_err();
        assert!(err.message.contains("does not exist"), "{err}");
        assert!(err.message.contains(missing.to_str().unwrap()), "{err}");
        assert!(!bundle.join("config.json").exists());
        assert!(!bundle.join("containers").exists());

        let bare = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "name": "web",
                "volume_mounts": [{"name": "disk", "mount_path": "/data"}]
            }],
            "volumes": [{"name": "disk", "source": {"volume": vol_id}}]
        })
        .to_string();
        request.user_data = Some(bare);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let err = ensure_slab_sources(&plan, &[]).unwrap_err();
        assert!(err.message.contains("has no host path"), "{err}");
        assert!(err.message.contains("disk"), "{err}");
    }

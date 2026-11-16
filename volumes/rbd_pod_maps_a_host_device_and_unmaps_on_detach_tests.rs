// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn rbd_pod_maps_a_host_device_and_unmaps_on_detach() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-rbd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let rbd_log = tmp.join("rbd.log");
        let rbd_state = tmp.join("mapped.json");
        let rbd = tmp.join("rbd");
        write_rbd(&rbd, &rbd_log, &rbd_state);
        let _env = EnvGuard::set(&tmp.join("youki"), &tmp.join("state"));
        std::env::set_var("PISTACK_RBD", &rbd);
        std::env::set_var("PISTACK_RBD_TEST_EUID", "0");

        let rt = YoukiRuntime;
        let machine = Id::new(kind::MACHINE);
        let volume = Id::new(kind::VOLUME);
        let secret = "super-secret-value";
        let missing = rbd_attach(
            machine.clone(),
            volume.clone(),
            rbd_target(Some(secret.into())),
        );
        assert_eq!(
            rt.attach_volume(&missing).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(!rbd_log.exists());

        let bundle = tmp.join("state").join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("config.json"), b"{}").unwrap();
        write_pod_mount(&bundle, &volume.to_string());
        let refused = rt.attach_volume(&missing).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert!(!refused.message.contains(secret));
        assert!(!rbd_log.exists());

        let keyring = tmp.join("keyring");
        std::fs::write(&keyring, secret).unwrap();
        let attach = rbd_attach(
            machine.clone(),
            volume.clone(),
            rbd_target(Some(keyring.to_string_lossy().into_owned())),
        );
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        let disks = read_disks(&bundle).unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].path, "/dev/rbd0");
        assert!(disks[0].rbd_mapped);
        assert_eq!(disks[0].device, "vdb");
        let mounted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(mounted["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["destination"] != "/data"));

        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        assert_eq!(read_disks(&bundle).unwrap()[0].path, "/dev/rbd0");
        let inv = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert_eq!(count_rbd(&inv, "device", "map"), 1);
        assert!(count_rbd(&inv, "device", "list") >= 2);
        let map = inv
            .iter()
            .find(|args| {
                args.first().map(String::as_str) == Some("device")
                    && args.get(1).map(String::as_str) == Some("map")
            })
            .unwrap();
        assert!(map.iter().any(|arg| arg == "volumes/img-1"));
        assert!(map.iter().any(|arg| arg == "--id"));
        assert!(map.iter().any(|arg| arg == "pistack"));
        assert!(map.iter().any(|arg| arg == "-m"));
        assert!(map
            .iter()
            .any(|arg| arg == "10.1.0.5:6789,mon.example:6789"));
        assert!(map.iter().any(|arg| arg == "--keyring"));
        assert!(map.iter().any(|arg| arg == keyring.to_str().unwrap()));
        assert!(map.iter().any(|arg| arg == "--read-only"));
        let logged = std::fs::read_to_string(&rbd_log).unwrap();
        assert!(!logged.contains(secret));

        rt.detach_volume(&crate::runtime::VolumeDetach {
            machine: machine.clone(),
            volume: volume.clone(),
            path: String::new(),
            device: None,
            target: Some(rbd_target(Some(keyring.to_string_lossy().into_owned()))),
        })
        .await
        .unwrap();
        assert!(read_disks(&bundle).unwrap().is_empty());
        let inv = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert_eq!(count_rbd(&inv, "device", "unmap"), 1);
        assert_eq!(std::fs::read_to_string(&rbd_state).unwrap().trim(), "[]");

        let dev_root = tmp.join("dev");
        let stable = dev_root.join("rbd").join("volumes").join("img-1");
        std::fs::create_dir_all(stable.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/dev/rbd0", &stable).unwrap();
        std::env::set_var("PISTACK_RBD_DEV_ROOT", &dev_root);
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        assert_eq!(
            read_disks(&bundle).unwrap()[0].path,
            stable.to_string_lossy().as_ref()
        );
        let mounted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let stable_s = stable.to_string_lossy();
        assert!(mounted["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["source"] != stable_s.as_ref()));
    }

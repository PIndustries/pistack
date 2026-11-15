// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn sudo_wraps_every_youki_command_and_root_ignores_it() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-sudo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let youki_log = tmp.join("youki.log");
        let sudo_log = tmp.join("sudo.log");
        let youki = tmp.join("youki");
        let sudo = tmp.join("sudo");
        write_script(&youki, &youki_log, 0);
        write_sudo(&sudo, &sudo_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &sudo);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");

        let image = tmp.join("image");
        std::fs::create_dir_all(image.join("bin")).unwrap();
        std::fs::write(image.join("marker"), "keep").unwrap();
        let mut request = req("web", "not-a-path", "10.8.0.1");
        request.user_data = Some(sandbox_doc(image.to_str().unwrap()));
        let rt = YoukiRuntime;
        rt.start(&request).await.unwrap();
        let bundle = state.join(request.id.to_string());
        let cfg: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert!(cfg["linux"].get("uidMappings").is_none());
        assert_eq!(
            ns_kinds(&cfg),
            vec!["pid", "network", "ipc", "uts", "mount"]
        );
        assert_eq!(cfg["process"]["user"]["uid"], 0);
        assert!(cfg["linux"]["resources"].get("memory").is_some());

        let youki_arg = youki.to_string_lossy().into_owned();
        let exec_out = rt
            .exec(
                &request.id,
                "web",
                &["/bin/echo".into(), "hello".into()],
                &[("MODE".into(), "prod".into())],
                Some("/work"),
                Duration::from_secs(5),
                EXEC_OUTPUT_CAP,
            )
            .await
            .unwrap();
        assert_eq!(exec_out.exit_code, 0);
        let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        for op in ["create", "start", "state", "exec"] {
            assert_sudo_op(&sudo_lines, &youki_arg, op);
        }
        let youki_lines = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
        let web_id = format!("{}-0", request.id);
        assert!(
            has_seq(
                &youki_lines,
                &[
                    "exec".into(),
                    "--cwd".into(),
                    "/work".into(),
                    "--env".into(),
                    "MODE=prod".into(),
                    web_id,
                    "--".into(),
                    "/bin/echo".into(),
                    "hello".into(),
                ],
            ),
            "{youki_lines:?}"
        );

        // Rootless fallback runs youki directly, the same way as create and start.
        std::env::remove_var(SUDO_ENV);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        let sudo_before = std::fs::read_to_string(&sudo_log).unwrap();
        let youki_before = std::fs::read_to_string(&youki_log).unwrap().len();
        let rootless = rt
            .exec(
                &request.id,
                "side",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(5),
                64,
            )
            .await
            .unwrap();
        assert_eq!(rootless.exit_code, 0);
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), sudo_before);
        let youki_now = std::fs::read_to_string(&youki_log).unwrap();
        let added = arg_lines(&youki_now[youki_before..]);
        let side_id = format!("{}-1", request.id);
        assert!(
            has_seq(
                &added,
                &["exec".into(), side_id, "--".into(), "/bin/true".into(),],
            ),
            "{added:?}"
        );
        assert!(added.iter().all(|line| line != "-n"));
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let youki_text = std::fs::read_to_string(&youki_log).unwrap();
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        assert!(!youki_text.contains("do-not-log-user-data"));
        assert!(!youki_text.contains("do-not-log-ssh-key"));
        assert!(!sudo_text.contains("do-not-log-user-data"));
        assert!(!sudo_text.contains("do-not-log-ssh-key"));
        assert!(!youki_text.contains("-n"));

        rt.stop(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        let after = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        assert_sudo_op(&after, &youki_arg, "kill");
        assert_sudo_op(&after, &youki_arg, "delete");

        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
        let _ = std::fs::write(&sudo_log, b"");
        let _ = std::fs::write(&youki_log, b"");
        let plain = req("plain", image.to_str().unwrap(), "10.8.0.2");
        rt.start(&plain).await.unwrap();
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
        let direct = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
        assert!(direct.iter().any(|line| line == "create"));
        assert!(direct.iter().all(|line| line != "-n"));
        rt.delete(&plain.id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
    }

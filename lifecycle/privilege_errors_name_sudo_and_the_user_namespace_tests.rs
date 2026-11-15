// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn privilege_errors_name_sudo_and_the_user_namespace() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-priv-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        write_script(&youki, &tmp.join("youki.log"), 0);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let rt = YoukiRuntime;
        let request = req("web", "not-a-path", "10.8.0.3");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains(SUDO_ENV), "{err}");
        assert!(err.message.contains("NOPASSWD"), "{err}");
        assert!(err.message.contains("newuidmap"), "{err}");
        assert!(!state.join(request.id.to_string()).exists());

        std::env::set_var(SUDO_ENV, "yes");
        std::env::set_var(SUDO_BIN_ENV, tmp.join("missing-sudo"));
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("sudo is not available"), "{err}");
        assert!(err.message.contains(&format!("unset {SUDO_ENV}")), "{err}");
        assert!(!state.join(request.id.to_string()).exists());

        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("NOPASSWD"), "{err}");
        assert!(!err.message.contains("unset"), "{err}");
    }

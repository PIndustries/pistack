// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn sandbox_command_replaces_the_image_process() {
        let cfg = ImageConfig {
            entrypoint: vec!["/entry".into()],
            cmd: vec!["from-image".into()],
            env: vec!["FROM=image".into()],
            working_dir: "/image".into(),
        };
        let both = Launch {
            command: vec!["/cmd".into()],
            args: vec!["--a".into()],
            env: vec![("APP_MODE".into(), "prod".into())],
            cwd: "/work".into(),
            ..Launch::default()
        };
        let (args, env, cwd) = process_spec(Some(&both), &cfg);
        assert_eq!(args, vec!["/cmd".to_string(), "--a".into()]);
        assert_eq!(cwd, "/work");
        assert!(env.iter().any(|e| e == "APP_MODE=prod"));
        assert!(env.iter().any(|e| e == "FROM=image"));
        let cmd_only = Launch {
            command: vec!["/cmd".into()],
            ..Launch::default()
        };
        assert_eq!(
            process_spec(Some(&cmd_only), &cfg).0,
            vec!["/cmd".to_string()]
        );
        let args_only = Launch {
            args: vec!["--only".into()],
            ..Launch::default()
        };
        assert_eq!(
            process_spec(Some(&args_only), &cfg).0,
            vec!["/entry".to_string(), "--only".into()]
        );
        assert_eq!(
            process_spec(None, &cfg).0,
            vec!["/entry".to_string(), "from-image".into()]
        );
        assert_eq!(
            process_spec(None, &ImageConfig::default()).0,
            vec!["/bin/sh".to_string()]
        );
        assert_eq!(process_spec(None, &cfg).2, "/image");

        let mut request = req("web", "ignored", "10.0.0.1");
        request.user_data = Some(sandbox_doc("/tmp/image"));
        let launch = launch_from(&request).unwrap();
        assert_eq!(launch.command, vec!["/usr/bin/web".to_string()]);
        assert_eq!(launch.args, vec!["--serve".to_string()]);
        assert_eq!(launch.env, vec![("APP_MODE".into(), "prod".into())]);
        assert!(launch
            .env
            .iter()
            .all(|(k, v)| !k.contains("do-not-log") && !v.contains("do-not-log")));
        request.user_data = Some("do-not-log-user-data".into());
        assert!(launch_from(&request).is_none());
        request.user_data = Some(r#"{"kind":"Other"}"#.into());
        assert!(launch_from(&request).is_none());
        request.user_data = Some(r#"{"kind":"PodSandbox","containers":"no"}"#.into());
        let err = sandbox_plan(&request).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("sandbox document is invalid"), "{err}");
        assert!(!err.message.contains("do-not-log"), "{err}");
    }

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn resolves_env_from_config_map_and_secret() {
        let mut app = Projected::default();
        app.data.insert("MODE".into(), "from-data".into());
        app.binary_data.insert("MODE".into(), "aWdub3JlZA==".into());
        app.binary_data.insert("token".into(), "c2VjcmV0".into());
        app.binary_data.insert("raw".into(), "/w==".into());
        let mut db = Projected::default();
        db.data.insert("password".into(), "s3cret-value".into());
        let mut config_maps = BTreeMap::new();
        config_maps.insert("app".into(), app);
        let mut secrets = BTreeMap::new();
        secrets.insert("db".into(), db);
        let env: Vec<SandboxEnv> = serde_json::from_value(serde_json::json!([
            {"name": "APP_MODE", "value": "prod"},
            {"name": "MODE", "value": "nope", "value_from": {"config_map": {"name": "app", "key": "MODE"}}},
            {"name": "TOKEN", "value_from": {"config_map": {"name": "app", "key": "token"}}},
            {"name": "PASSWORD", "value": "nope", "value_from": {"secret": {"name": "db", "key": "password"}}},
            {"name": "MISSING", "value": "nope", "value_from": {"secret": {"name": "db", "key": "nope"}}},
            {"name": "RAW", "value": "nope", "value_from": {"config_map": {"name": "app", "key": "raw"}}},
            {"name": "BAD", "value": "do-not-log-user-data", "value_from": {"secret": "do-not-log-user-data"}},
            {"name": "", "value": "skip"}
        ]))
        .unwrap();
        let resolved = resolve_process_env(
            &["FROM=image".into(), "APP_MODE=image".into()],
            &env,
            &config_maps,
            &secrets,
        );
        assert!(resolved
            .iter()
            .any(|e| e == &format!("PATH={DEFAULT_PATH}")));
        assert!(resolved.iter().any(|e| e == "FROM=image"));
        assert!(resolved.iter().any(|e| e == "APP_MODE=prod"));
        assert!(resolved.iter().any(|e| e == "MODE=from-data"));
        assert!(resolved.iter().any(|e| e == "TOKEN=secret"));
        assert!(resolved.iter().any(|e| e == "PASSWORD=s3cret-value"));
        assert!(resolved.iter().all(|e| !e.starts_with("MISSING=")));
        assert!(resolved.iter().all(|e| !e.starts_with("RAW=")));
        assert!(resolved.iter().all(|e| !e.starts_with("BAD=")));
        assert!(resolved.iter().all(|e| !e.contains("do-not-log")));
        assert_eq!(
            parse_state_pid(br#"{"pid":4242,"status":"running"}"#),
            Some(4242)
        );
        assert_eq!(parse_state_pid(br#"{"pid":0}"#), None);
        assert_eq!(parse_state_pid(br"not json"), None);
    }

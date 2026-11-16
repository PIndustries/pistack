// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use pistack_types::error::ErrorCode;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::count_arg;
use crate::runtime::youki::test_support::docker_save;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::mode_of;
use crate::runtime::youki::test_support::ns_path;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::sandbox_doc;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::tree_text;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::PowerState;
use pistack_types::kind;
use pistack_types::Id;

#[tokio::test]
async fn start_is_idempotent_stop_kills_and_missing_bin_is_unavailable() {
    let _guard = env_lock().lock().await;
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let log = tmp.join("youki.log");
    let bin = tmp.join("youki");
    let state = tmp.join("state");
    write_script(&bin, &log, 0);
    let _env = EnvGuard::set(&bin, &state);

    let rt = YoukiRuntime;
    let unknown = Id::new(kind::MACHINE);
    assert_eq!(
        rt.stop(&unknown).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    rt.delete(&unknown).await.unwrap();

    let external = tmp.join("external-rootfs");
    std::fs::create_dir_all(external.join("bin")).unwrap();
    std::fs::write(external.join("marker"), "keep").unwrap();
    std::fs::write(external.join("bin").join("web"), "#!/bin/sh\n").unwrap();
    let image = external.to_str().unwrap();
    let mut request = req("web", "not-a-path", "10.1.0.5");
    request.user_data = Some(sandbox_doc(image));
    let started = rt.start(&request).await.unwrap();
    assert_eq!(started.id, request.id);
    assert_eq!(started.power, PowerState::On);
    assert_eq!(started.addresses, vec!["10.1.0.5".to_string()]);

    let bundle = state.join(request.id.to_string());
    let cfg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["root"]["path"], "rootfs");
    assert_eq!(cfg["root"]["readonly"], false);
    assert_eq!(cfg["hostname"], "web");
    assert_eq!(cfg["process"]["args"], serde_json::json!(["/pause"]));
    assert_eq!(cfg["process"]["cwd"], "/");
    let sandbox_ns = cfg["linux"]["namespaces"].as_array().unwrap();
    assert!(sandbox_ns.iter().all(|n| n.get("path").is_none()));
    assert_eq!(mode_of(&bundle.join("rootfs").join("pause")), 0o755);
    assert!(!bundle.join("image").exists());
    let rootfs_meta = std::fs::symlink_metadata(bundle.join("rootfs")).unwrap();
    assert!(rootfs_meta.file_type().is_dir());
    assert!(!rootfs_meta.file_type().is_symlink());
    assert!(!bundle.join("rootfs").join("marker").exists());

    let c0 = bundle.join("containers").join("0");
    let c1 = bundle.join("containers").join("1");
    let web: serde_json::Value =
        serde_json::from_slice(&std::fs::read(c0.join("config.json")).unwrap()).unwrap();
    let side: serde_json::Value =
        serde_json::from_slice(&std::fs::read(c1.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        web["process"]["args"],
        serde_json::json!(["/usr/bin/web", "--serve"])
    );
    assert_eq!(web["process"]["cwd"], "/work");
    assert!(web.get("hostname").is_none());
    let env = web["process"]["env"].as_array().unwrap();
    assert!(env.iter().any(|v| v == "APP_MODE=prod"));
    assert!(env.iter().all(|v| {
        let text = v.as_str().unwrap_or("");
        !text.contains("do-not-log")
    }));
    assert_eq!(
        side["process"]["args"],
        serde_json::json!(["/usr/bin/side", "--watch"])
    );
    assert!(side.get("hostname").is_none());
    for cfg in [&web, &side] {
        assert_eq!(
            ns_path(cfg, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert_eq!(ns_path(cfg, "pid").as_deref(), Some("/proc/4242/ns/pid"));
        assert_eq!(ns_path(cfg, "ipc").as_deref(), Some("/proc/4242/ns/ipc"));
        assert_eq!(ns_path(cfg, "uts").as_deref(), Some("/proc/4242/ns/uts"));
        assert_eq!(ns_path(cfg, "mount"), None);
    }
    assert_eq!(mode_of(&c0.join("config.json")), 0o600);
    assert_eq!(
        std::fs::read_to_string(c0.join("rootfs").join("marker")).unwrap(),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(c1.join("rootfs").join("marker")).unwrap(),
        "keep"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("state.json")).unwrap()).unwrap();
    assert_eq!(
        saved["containers"],
        serde_json::json!([format!("{}-0", request.id), format!("{}-1", request.id)])
    );
    let bundle_text = tree_text(&bundle);
    assert!(!bundle_text.contains("do-not-log-user-data"));
    assert!(!bundle_text.contains("do-not-log-ssh-key"));

    let after_first = std::fs::read_to_string(&log).unwrap();
    assert!(!after_first.contains("do-not-log-user-data"));
    assert!(!after_first.contains("do-not-log-ssh-key"));
    let lines = arg_lines(&after_first);
    assert_eq!(count_arg(&lines, "create"), 3);
    assert_eq!(count_arg(&lines, "start"), 3);
    // One state per create (leftover check) plus the sandbox pid query.
    assert_eq!(count_arg(&lines, "state"), 4);
    assert_eq!(count_arg(&lines, "kill"), 0);
    let bundle_arg = bundle.to_string_lossy().into_owned();
    let id_arg = request.id.to_string();
    assert!(lines.windows(4).any(|w| {
        w[0] == "create" && w[1] == "--bundle" && w[2] == bundle_arg && w[3] == id_arg
    }));
    let c0_arg = c0.to_string_lossy().into_owned();
    assert!(lines.windows(4).any(|w| {
        w[0] == "create"
            && w[1] == "--bundle"
            && w[2] == c0_arg
            && w[3] == format!("{}-0", request.id)
    }));

    let again = rt.start(&request).await.unwrap();
    assert_eq!(again.power, PowerState::On);
    assert_eq!(again.addresses, vec!["10.1.0.5".to_string()]);
    let after_second = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        after_first, after_second,
        "second start must not invoke youki"
    );

    let listed = rt.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, request.id);
    assert_eq!(listed[0].power, PowerState::On);

    rt.stop(&request.id).await.unwrap();
    let after_stop = arg_lines(&std::fs::read_to_string(&log).unwrap());
    assert_eq!(count_arg(&after_stop, "kill"), 3);
    assert_eq!(count_arg(&after_stop, "SIGTERM"), 3);
    let kill_ids: Vec<String> = after_stop
        .windows(3)
        .filter(|w| w[0] == "kill" && w[2] == "SIGTERM")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(
        kill_ids,
        vec![
            format!("{}-1", request.id),
            format!("{}-0", request.id),
            request.id.to_string(),
        ]
    );
    assert_eq!(rt.list().await.unwrap()[0].power, PowerState::Off);

    let missing = tmp.join("missing-youki");
    std::env::set_var("PISTACK_YOUKI_BIN", &missing);
    for err in [
        rt.start(&request).await.unwrap_err(),
        rt.stop(&request.id).await.unwrap_err(),
        rt.delete(&request.id).await.unwrap_err(),
        rt.list().await.unwrap_err(),
    ] {
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    }
    std::env::set_var("PISTACK_YOUKI_BIN", &bin);

    let before_bad = std::fs::read_to_string(&log).unwrap();
    let named = req("named", "nginx -g daemon off;", "10.2.0.7");
    let err = rt.start(&named).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("image cannot be unpacked"), "{err}");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before_bad);
    assert!(rt.list().await.unwrap().iter().all(|m| m.id != named.id));

    let empty = tmp.join("empty-rootfs");
    std::fs::create_dir(&empty).unwrap();
    let empty_req = req("empty", empty.to_str().unwrap(), "10.2.0.8");
    let err = rt.start(&empty_req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("image rootfs is empty"), "{err}");
    assert!(!state
        .join(empty_req.id.to_string())
        .join("state.json")
        .exists());
    assert!(rt
        .list()
        .await
        .unwrap()
        .iter()
        .all(|m| m.id != empty_req.id));

    let link = tmp.join("link-rootfs");
    std::os::unix::fs::symlink(&external, &link).unwrap();
    let link_req = req("link", link.to_str().unwrap(), "10.2.0.9");
    let err = rt.start(&link_req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("image cannot be unpacked"), "{err}");

    let dotted = format!(
        "{}/../{}/external-rootfs",
        tmp.parent().unwrap().display(),
        tmp.file_name().unwrap().to_string_lossy()
    );
    assert!(dotted.contains(".."));
    let dot_req = req("dotted", &dotted, "10.2.0.10");
    let err = rt.start(&dot_req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert_eq!(
        std::fs::read_to_string(external.join("marker")).unwrap(),
        "keep"
    );

    let archive = tmp.join("saved.tar");
    std::fs::write(&archive, docker_save()).unwrap();
    let tar_req = req("saved", archive.to_str().unwrap(), "10.2.0.11");
    rt.start(&tar_req).await.unwrap();
    let tar_bundle = state.join(tar_req.id.to_string());
    let tar_cfg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tar_bundle.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        tar_cfg["process"]["args"],
        serde_json::json!(["/bin/app", "serve"])
    );
    assert_eq!(tar_cfg["process"]["cwd"], "/app");
    let tar_root = tar_bundle.join("rootfs");
    assert_eq!(
        std::fs::read_to_string(tar_root.join("keep")).unwrap(),
        "yes"
    );
    assert!(!tar_root.join("gone").exists());
    assert!(!tar_root.join("sub").join("old").exists());
    assert_eq!(
        std::fs::read_to_string(tar_root.join("sub").join("new")).unwrap(),
        "n"
    );
    let tar_text = std::fs::read_to_string(tar_bundle.join("config.json")).unwrap()
        + &std::fs::read_to_string(tar_bundle.join("state.json")).unwrap()
        + &tree_text(&tar_root);
    assert!(!tar_text.contains("do-not-log-user-data"));
    assert!(!tar_text.contains("do-not-log-ssh-key"));
    let tar_log = std::fs::read_to_string(&log).unwrap();
    assert!(!tar_log.contains(archive.to_str().unwrap()));

    write_script(&bin, &log, 1);
    let bad_root = tmp.join("bad-rootfs");
    std::fs::create_dir(&bad_root).unwrap();
    std::fs::write(bad_root.join("marker"), "keep").unwrap();
    let bad = req("bad", bad_root.to_str().unwrap(), "10.3.0.1");
    let err = rt.start(&bad).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.contains("youki exploded"), "{}", err.message);
    assert!(!state.join(bad.id.to_string()).join("state.json").exists());
    assert!(rt
        .list()
        .await
        .unwrap()
        .iter()
        .all(|m| m.id != bad.id || m.power != PowerState::On));

    write_script(&bin, &log, 0);
    rt.delete(&bad.id).await.unwrap();
    rt.delete(&tar_req.id).await.unwrap();
    rt.delete(&request.id).await.unwrap();
    rt.delete(&request.id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(external.join("marker")).unwrap(),
        "keep"
    );
    assert!(external.join("bin").join("web").is_file());
    assert_eq!(
        rt.stop(&request.id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(rt.list().await.unwrap().is_empty());
}

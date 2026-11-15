// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::file_uid::file_uid;
use crate::runtime::youki::network::host_netns_from;
use crate::runtime::youki::privilege::CgroupCaps;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::sandbox_namespaces;
use crate::runtime::youki::spec::sandbox_plan;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::ns_kinds;
use crate::runtime::youki::test_support::ns_path;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::sandbox_doc;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::EnvSlot;
use std::path::PathBuf;

#[tokio::test]
async fn rootless_pod_maps_one_id_and_joins_the_sandbox_user_namespace() {
    let _guard = env_lock().lock().await;
    let _netns = EnvSlot::clear(NETNS_ENV);
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-rootless-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let image = tmp.join("image");
    std::fs::create_dir_all(image.join("bin")).unwrap();
    std::fs::write(image.join("marker"), "a").unwrap();
    let mut request = req("web", "not-a-path", "10.9.0.2");
    request.user_data = Some(sandbox_doc(image.to_str().unwrap()));
    let plan = sandbox_plan(&request).unwrap().unwrap();
    let bundle = tmp.join("pod");
    let limits = Limits {
        vcpus: 2,
        memory_mib: 64,
    };
    let mode = RunMode::rootless(1000, 1000);
    mode.stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
        .unwrap();

    let sandbox: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        ns_kinds(&sandbox),
        vec!["user", "pid", "network", "ipc", "uts", "mount"]
    );
    assert!(sandbox["linux"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|ns| ns.get("path").is_none()));
    assert_eq!(
        sandbox["linux"]["uidMappings"],
        serde_json::json!([{ "containerID": 0, "hostID": 1000, "size": 1 }])
    );
    assert_eq!(
        sandbox["linux"]["gidMappings"],
        serde_json::json!([{ "containerID": 0, "hostID": 1000, "size": 1 }])
    );
    assert_eq!(sandbox["process"]["user"]["uid"], 0);
    assert_eq!(sandbox["process"]["user"]["gid"], 0);
    assert!(sandbox["linux"].get("resources").is_none());

    for index in ["0", "1"] {
        let cfg: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join(index).join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ns_kinds(&cfg),
            vec!["user", "pid", "network", "ipc", "uts", "mount"]
        );
        assert_eq!(ns_path(&cfg, "user").as_deref(), Some("/proc/4242/ns/user"));
        assert_eq!(ns_path(&cfg, "pid").as_deref(), Some("/proc/4242/ns/pid"));
        assert_eq!(
            ns_path(&cfg, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert_eq!(ns_path(&cfg, "ipc").as_deref(), Some("/proc/4242/ns/ipc"));
        assert_eq!(ns_path(&cfg, "uts").as_deref(), Some("/proc/4242/ns/uts"));
        assert_eq!(ns_path(&cfg, "mount"), None);
        assert!(cfg["linux"].get("uidMappings").is_none());
        assert!(cfg["linux"].get("gidMappings").is_none());
        assert_eq!(cfg["process"]["user"]["uid"], 0);
        assert!(cfg["linux"].get("resources").is_none());
    }

    let mut delegated = mode.clone();
    delegated.cgroup = CgroupCaps {
        memory: true,
        cpu: false,
    };
    delegated
        .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
        .unwrap();
    let sandbox: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        sandbox["linux"]["resources"]["memory"]["limit"],
        64 * 1024 * 1024
    );
    assert!(sandbox["linux"]["resources"].get("cpu").is_none());

    let netns = tmp.join("netns");
    std::fs::write(&netns, b"ns").unwrap();
    let owner = file_uid(&netns).unwrap();
    let owned = RunMode::rootless(owner, owner);
    let joined = sandbox_namespaces(&owned, Some(&netns));
    assert_eq!(
        joined
            .iter()
            .find(|ns| ns["type"] == "network")
            .and_then(|ns| ns["path"].as_str()),
        Some(netns.to_str().unwrap())
    );
    let foreign = RunMode::rootless(owner.wrapping_add(1), owner);
    let skipped = sandbox_namespaces(&foreign, Some(&netns));
    assert!(skipped
        .iter()
        .find(|ns| ns["type"] == "network")
        .unwrap()
        .get("path")
        .is_none());
    assert_eq!(
        skipped.first().and_then(|ns| ns["type"].as_str()),
        Some("user")
    );
    let root = sandbox_namespaces(&RunMode::privileged(), Some(&netns));
    assert_eq!(
        root.iter()
            .find(|ns| ns["type"] == "network")
            .and_then(|ns| ns["path"].as_str()),
        Some(netns.to_str().unwrap())
    );
    assert!(root.iter().all(|ns| ns["type"] != "user"));
    assert!(host_netns_from(Some(std::ffi::OsStr::new("rel/ns"))).is_none());
    assert!(host_netns_from(Some(std::ffi::OsStr::new("/tmp/../etc"))).is_none());
    assert_eq!(
        host_netns_from(Some(std::ffi::OsStr::new("/var/run/netns/pod"))),
        Some(PathBuf::from("/var/run/netns/pod"))
    );
}

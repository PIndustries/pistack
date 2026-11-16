// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::file_uid::file_uid;
use crate::runtime::youki::privilege::CgroupCaps;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::sandbox_plan;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::mode_of;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::EnvSlot;
use std::path::PathBuf;

#[tokio::test]
async fn sudo_mode_opens_secret_volumes_and_keeps_the_bundle_private() {
    let _guard = env_lock().lock().await;
    let _netns = EnvSlot::clear(NETNS_ENV);
    let tmp = temp_dir("sudo-vol");
    let _dir = scopeguard_dir(&tmp);
    let image = tmp.join("image");
    std::fs::create_dir_all(&image).unwrap();
    std::fs::write(image.join("marker"), "a").unwrap();
    let user_data = serde_json::json!({
        "kind": "PodSandbox",
        "containers": [{
            "name": "web",
            "image": image,
            "command": ["/bin/web"]
        }],
        "volumes": [
            {"name": "cfg", "source": {"config_map": "app"}},
            {"name": "sec", "source": {"secret": "db"}},
            {"name": "empty-sec", "source": {"secret": "missing"}}
        ],
        "config_maps": {"app": {"data": {"MODE": "prod"}}},
        "secrets": {"db": {"data": {"password": "s3cret-value"}}}
    })
    .to_string();
    let mut request = req("web", "not-a-path", "10.9.1.1");
    request.user_data = Some(user_data);
    let plan = sandbox_plan(&request).unwrap().unwrap();
    let limits = Limits {
        vcpus: 1,
        memory_mib: 64,
    };
    let sudo_mode = RunMode {
        privilege: Privilege::Sudo {
            sudo: PathBuf::from("/bin/sudo"),
        },
        cgroup: CgroupCaps {
            memory: true,
            cpu: true,
        },
    };
    let sudo_bundle = tmp.join("sudo");
    sudo_mode
        .stage_pod_bundles(&sudo_bundle, "web", &limits, &plan, "not-a-path", 4242)
        .unwrap();
    assert_eq!(mode_of(&sudo_bundle), 0o700);
    let cfg = sudo_bundle.join("volumes").join("0-cfg");
    let sec = sudo_bundle.join("volumes").join("1-sec");
    let empty = sudo_bundle.join("volumes").join("2-empty-sec");
    assert_eq!(mode_of(&cfg), 0o755);
    assert_eq!(mode_of(&cfg.join("MODE")), 0o644);
    assert_eq!(std::fs::read_to_string(cfg.join("MODE")).unwrap(), "prod");
    assert_eq!(mode_of(&sec), 0o755);
    assert_eq!(mode_of(&sec.join("password")), 0o644);
    assert_eq!(
        std::fs::read_to_string(sec.join("password")).unwrap(),
        "s3cret-value"
    );
    assert_eq!(mode_of(&empty), 0o755);
    assert_eq!(
        file_uid(&sec.join("password")).unwrap(),
        file_uid(&image.join("marker")).unwrap()
    );

    let root_bundle = tmp.join("root");
    RunMode::privileged()
        .stage_pod_bundles(&root_bundle, "web", &limits, &plan, "not-a-path", 4242)
        .unwrap();
    assert_eq!(mode_of(&root_bundle.join("volumes").join("0-cfg")), 0o755);
    assert_eq!(
        mode_of(&root_bundle.join("volumes").join("0-cfg").join("MODE")),
        0o644
    );
    assert_eq!(mode_of(&root_bundle.join("volumes").join("1-sec")), 0o700);
    assert_eq!(
        mode_of(&root_bundle.join("volumes").join("1-sec").join("password")),
        0o400
    );
    assert_eq!(
        mode_of(&root_bundle.join("volumes").join("2-empty-sec")),
        0o700
    );

    let less_bundle = tmp.join("rootless");
    RunMode::rootless(1000, 1000)
        .stage_pod_bundles(&less_bundle, "web", &limits, &plan, "not-a-path", 4242)
        .unwrap();
    assert_eq!(mode_of(&less_bundle.join("volumes").join("1-sec")), 0o700);
    assert_eq!(
        mode_of(&less_bundle.join("volumes").join("1-sec").join("password")),
        0o400
    );
    assert_eq!(
        mode_of(&less_bundle.join("volumes").join("0-cfg").join("MODE")),
        0o644
    );
}

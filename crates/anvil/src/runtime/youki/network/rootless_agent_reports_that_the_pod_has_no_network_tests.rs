// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::lab_request;
use crate::runtime::youki::test_support::ns_path;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::read_cfg;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;
use crate::runtime::PowerState;

#[tokio::test]
pub(in crate::runtime::youki) async fn rootless_agent_reports_that_the_pod_has_no_network() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-rootless");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let ip_log = tmp.join("ip.log");
    write_script(&youki, &tmp.join("youki.log"), 0);
    write_ip(&ip, &ip_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
    std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
    std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let request = lab_request(&pod_image(&tmp));
    let rt = YoukiRuntime;
    let started = rt.start(&request).await.unwrap();
    assert!(started.power == PowerState::On);
    assert!(started.addresses.is_empty(), "{:?}", started.addresses);
    assert!(
        started.message.contains("pod has no network"),
        "{}",
        started.message
    );
    assert!(
        started.message.contains("PISTACK_YOUKI_SUDO"),
        "{}",
        started.message
    );
    let cfg = read_cfg(&state.join(request.id.to_string()));
    assert!(ns_path(&cfg, "network").is_none(), "{cfg}");
    assert!(std::fs::read_to_string(&ip_log)
        .unwrap_or_default()
        .is_empty());
    let again = rt.start(&request).await.unwrap();
    assert!(again.addresses.is_empty());
    assert_eq!(again.message, started.message);
    let listed = rt.list().await.unwrap();
    assert_eq!(listed[0].message, started.message);
    assert!(listed[0].addresses.is_empty());
}

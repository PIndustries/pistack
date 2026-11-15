// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::consts::NETNS_ENV;
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

#[tokio::test]
pub(in crate::runtime::youki) async fn explicit_netns_skips_the_veth() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-env");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let ip_log = tmp.join("ip.log");
    write_script(&youki, &tmp.join("youki.log"), 0);
    write_ip(&ip, &ip_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    let dev = tmp.join("dev.ns");
    std::fs::write(&dev, b"ns").unwrap();
    std::env::set_var(NETNS_ENV, &dev);
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let request = lab_request(&pod_image(&tmp));
    let started = YoukiRuntime.start(&request).await.unwrap();
    assert_eq!(started.addresses[0], "10.90.0.10");
    assert!(started.message.is_empty(), "{}", started.message);
    assert!(std::fs::read_to_string(&ip_log)
        .unwrap_or_default()
        .is_empty());
    let cfg = read_cfg(&state.join(request.id.to_string()));
    assert_eq!(
        ns_path(&cfg, "network").as_deref(),
        Some(dev.to_str().unwrap())
    );
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::ns_path;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::read_cfg;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::sandbox_doc;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
pub(in crate::runtime::youki) async fn a_pod_without_a_port_does_not_call_ip() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-none");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let ip_log = tmp.join("ip.log");
    write_script(&youki, &tmp.join("youki.log"), 0);
    write_ip(&ip, &ip_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let image = pod_image(&tmp);
    let mut request = req("web", "not-a-path", "10.1.0.5");
    request.user_data = Some(sandbox_doc(&image));
    let started = YoukiRuntime.start(&request).await.unwrap();
    assert_eq!(started.addresses, vec!["10.1.0.5".to_string()]);
    assert!(started.message.is_empty());
    let cfg = read_cfg(&state.join(request.id.to_string()));
    assert!(ns_path(&cfg, "network").is_none(), "{cfg}");
    assert!(std::fs::read_to_string(&ip_log)
        .unwrap_or_default()
        .is_empty());
}

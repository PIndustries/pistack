// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;
use pistack_types::error::ErrorCode;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::lab_request;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
pub(in crate::runtime::youki) async fn a_bad_mac_is_rejected_before_ip() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-mac");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let ip_log = tmp.join("ip.log");
    write_script(&youki, &tmp.join("youki.log"), 0);
    write_ip(&ip, &ip_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let mut request = lab_request(&pod_image(&tmp));
    request.nics[0].mac = "not-a-mac".into();
    let err = YoukiRuntime.start(&request).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest, "{err}");
    assert!(err.message.contains("mac"), "{err}");
    assert!(std::fs::read_to_string(&ip_log)
        .unwrap_or_default()
        .is_empty());
}

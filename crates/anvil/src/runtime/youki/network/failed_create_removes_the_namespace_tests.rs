// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;
use pistack_types::error::ErrorCode;

use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::ip_seq;
use crate::runtime::youki::test_support::lab_request;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::seq_count;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
pub(in crate::runtime::youki) async fn failed_create_removes_the_namespace() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-fail");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let ip_log = tmp.join("ip.log");
    write_script(&youki, &tmp.join("youki.log"), 1);
    write_ip(&ip, &ip_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let request = lab_request(&pod_image(&tmp));
    let err = YoukiRuntime.start(&request).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    let log = std::fs::read_to_string(&ip_log).unwrap();
    assert!(ip_seq(&log, &["netns", "add", "pn018f3a2b1c4d"]), "{log}");
    assert!(
        seq_count(&log, &["netns", "del", "pn018f3a2b1c4d"]) >= 2,
        "{log}"
    );
    assert!(ip_seq(&log, &["link", "del", "vh525400aabbcc"]), "{log}");
}

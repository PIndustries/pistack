// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::consts::SUDO_BIN_ENV;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::ip_seq;
use crate::runtime::youki::test_support::lab_request;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::write_sudo;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
pub(in crate::runtime::youki) async fn sudo_prefixes_ip_when_the_agent_is_not_root() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet-sudo");
    let _dir = scopeguard_dir(&tmp);
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let sudo = tmp.join("sudo");
    let ip_log = tmp.join("ip.log");
    let sudo_log = tmp.join("sudo.log");
    write_script(&youki, &tmp.join("youki.log"), 0);
    write_ip(&ip, &ip_log);
    write_sudo(&sudo, &sudo_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
    std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
    std::env::set_var(SUDO_ENV, "1");
    std::env::set_var(SUDO_BIN_ENV, &sudo);
    std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
    std::env::set_var("PISTACK_IP_BIN", &ip);
    let request = lab_request(&pod_image(&tmp));
    YoukiRuntime.start(&request).await.unwrap();
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
    let ip_path = ip.to_string_lossy().into_owned();
    assert!(
        ip_seq(
            &sudo_text,
            &["-n", &ip_path, "netns", "add", "pn018f3a2b1c4d"]
        ),
        "{sudo_text}"
    );
    assert!(
        ip_seq(
            &sudo_text,
            &[
                "-n",
                &ip_path,
                "link",
                "set",
                "vh525400aabbcc",
                "master",
                "pi01a102de3f33",
            ]
        ),
        "{sudo_text}"
    );
    let ip_text = std::fs::read_to_string(&ip_log).unwrap();
    assert!(
        ip_seq(
            &ip_text,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "route",
                "add",
                "default",
                "via",
                "10.90.0.1",
            ]
        ),
        "{ip_text}"
    );
    // `ip -n <netns>` is the namespace selector. The sudo flag is only
    // on the sudo argv, in front of the ip binary.
    assert!(
        !ip_text.contains(&ip_path),
        "ip argv must not include the sudo wrapper: {ip_text}"
    );
}

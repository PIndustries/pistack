// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use crate::runtime::youki::consts::SUDO_BIN_ENV;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::count_arg;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::ip_seq;
use crate::runtime::youki::test_support::lab_request;
use crate::runtime::youki::test_support::ns_path;
use crate::runtime::youki::test_support::pod_image;
use crate::runtime::youki::test_support::read_cfg;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::seq_count;
use crate::runtime::youki::test_support::temp_dir;
use crate::runtime::youki::test_support::write_ip;
use crate::runtime::youki::test_support::write_script;
use crate::runtime::youki::test_support::write_sudo;
use crate::runtime::youki::test_support::EnvGuard;

#[tokio::test]
pub(in crate::runtime::youki) async fn pod_joins_its_bridge_and_teardown_removes_the_namespace() {
    let _guard = env_lock().lock().await;
    let tmp = temp_dir("podnet");
    let _dir = scopeguard_dir(&tmp);
    let youki_log = tmp.join("youki.log");
    let ip_log = tmp.join("ip.log");
    let sudo_log = tmp.join("sudo.log");
    let youki = tmp.join("youki");
    let ip = tmp.join("ip");
    let sudo = tmp.join("sudo");
    write_script(&youki, &youki_log, 0);
    write_ip(&ip, &ip_log);
    write_sudo(&sudo, &sudo_log);
    let state = tmp.join("state");
    let _env = EnvGuard::set(&youki, &state);
    // A root agent ignores PISTACK_YOUKI_SUDO, including for ip.
    std::env::set_var(SUDO_ENV, "1");
    std::env::set_var(SUDO_BIN_ENV, &sudo);
    std::env::set_var("PISTACK_IP_BIN", &ip);

    let request = lab_request(&pod_image(&tmp));
    let rt = YoukiRuntime;
    let started = rt.start(&request).await.unwrap();
    assert_eq!(
        started.addresses,
        vec!["10.90.0.10".to_string(), "10.90.0.11".to_string()]
    );
    assert!(started.message.is_empty(), "{}", started.message);
    let bundle = state.join(request.id.to_string());
    let cfg = read_cfg(&bundle);
    assert_eq!(
        ns_path(&cfg, "network").as_deref(),
        Some("/var/run/netns/pn018f3a2b1c4d")
    );

    let log = std::fs::read_to_string(&ip_log).unwrap();
    assert!(ip_seq(&log, &["link", "del", "vh525400aabbcc"]), "{log}");
    assert!(ip_seq(&log, &["link", "del", "vp525400aabbcc"]), "{log}");
    assert!(ip_seq(&log, &["link", "del", "vh525400112233"]), "{log}");
    assert!(ip_seq(&log, &["netns", "del", "pn018f3a2b1c4d"]), "{log}");
    assert!(ip_seq(&log, &["netns", "add", "pn018f3a2b1c4d"]), "{log}");
    assert!(
        ip_seq(
            &log,
            &[
                "link",
                "add",
                "vh525400aabbcc",
                "type",
                "veth",
                "peer",
                "name",
                "vp525400aabbcc",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &["link", "set", "vh525400aabbcc", "master", "pi01a102de3f33"]
        ),
        "{log}"
    );
    assert!(
        ip_seq(&log, &["link", "set", "vh525400aabbcc", "up"]),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &["link", "set", "vh525400112233", "master", "po01a102de3f33"]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &["link", "set", "vp525400aabbcc", "netns", "pn018f3a2b1c4d",]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "vp525400aabbcc",
                "name",
                "eth0",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "eth0",
                "address",
                "52:54:00:aa:bb:cc",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "addr",
                "add",
                "10.90.0.10/24",
                "dev",
                "eth0",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(&log, &["-n", "pn018f3a2b1c4d", "link", "set", "lo", "up"]),
        "{log}"
    );
    assert!(
        ip_seq(&log, &["-n", "pn018f3a2b1c4d", "link", "set", "eth0", "up"]),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "vp525400112233",
                "name",
                "eth1",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
            &[
                "-n",
                "pn018f3a2b1c4d",
                "addr",
                "add",
                "10.90.0.11/24",
                "dev",
                "eth1",
            ]
        ),
        "{log}"
    );
    assert!(
        ip_seq(
            &log,
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
        "{log}"
    );
    assert_eq!(seq_count(&log, &["route", "add", "default"]), 1, "{log}");
    assert!(!log.contains("pistack0"), "{log}");
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap_or_default();
    assert!(sudo_text.is_empty(), "root must not sudo ip: {sudo_text}");

    let again = rt.start(&request).await.unwrap();
    assert_eq!(again.addresses, started.addresses);
    assert_eq!(
        std::fs::read_to_string(&ip_log).unwrap(),
        log,
        "a running pod is not rewired"
    );
    let listed = rt.list().await.unwrap();
    assert_eq!(listed[0].addresses, started.addresses);
    assert!(listed[0].message.is_empty());

    rt.stop(&request.id).await.unwrap();
    let after_stop = std::fs::read_to_string(&ip_log).unwrap();
    assert!(
        seq_count(&after_stop, &["netns", "del", "pn018f3a2b1c4d"]) >= 2,
        "{after_stop}"
    );
    assert!(
        ip_seq(&after_stop, &["link", "del", "vh525400112233"]),
        "{after_stop}"
    );

    // Stop deleted the namespace. The next start must create it again
    // and create the sandbox again so youki joins that namespace.
    let creates_before = count_arg(
        &arg_lines(&std::fs::read_to_string(&youki_log).unwrap()),
        "create",
    );
    let adds_before = seq_count(&after_stop, &["netns", "add", "pn018f3a2b1c4d"]);
    let restarted = rt.start(&request).await.unwrap();
    assert_eq!(restarted.addresses, started.addresses);
    assert!(restarted.message.is_empty(), "{}", restarted.message);
    let cfg = read_cfg(&bundle);
    assert_eq!(
        ns_path(&cfg, "network").as_deref(),
        Some("/var/run/netns/pn018f3a2b1c4d")
    );
    let after_restart = std::fs::read_to_string(&ip_log).unwrap();
    assert!(
        seq_count(&after_restart, &["netns", "add", "pn018f3a2b1c4d"]) > adds_before,
        "{after_restart}"
    );
    let creates_after = count_arg(
        &arg_lines(&std::fs::read_to_string(&youki_log).unwrap()),
        "create",
    );
    assert!(
        creates_after > creates_before,
        "sandbox must be created again ({creates_before} -> {creates_after})"
    );

    rt.delete(&request.id).await.unwrap();
    let after_delete = std::fs::read_to_string(&ip_log).unwrap();
    assert!(
        seq_count(&after_delete, &["netns", "del", "pn018f3a2b1c4d"])
            > seq_count(&log, &["netns", "del", "pn018f3a2b1c4d"]),
        "{after_delete}"
    );
}

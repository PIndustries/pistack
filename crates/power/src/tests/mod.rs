// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;
use crate::bmc_host::bmc_host;
use crate::bmc_user::bmc_user;

/// The tools are chosen by process-wide environment variables, so the
/// tests that set them run one at a time.
fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[test]
fn kinds_parse() {
    assert_eq!(Kind::parse("ipmi").unwrap(), Kind::Ipmi);
    assert_eq!(Kind::parse("Redfish").unwrap(), Kind::Redfish);
    assert!(Kind::parse("wg26").is_err());
    assert_eq!(Kind::Ipmi.as_str(), "ipmi");
    assert_eq!(Kind::Redfish.as_str(), "redfish");
}

#[test]
fn address_and_user_are_checked() {
    assert_eq!(bmc_host("10.9.1.8").unwrap(), ("10.9.1.8".into(), None));
    assert_eq!(
        bmc_host("10.9.1.8:623").unwrap(),
        ("10.9.1.8".into(), Some(623))
    );
    assert!(bmc_host("10.9.1.8; rm -rf /").is_err());
    assert!(bmc_host("").is_err());
    assert_eq!(bmc_user("admin").unwrap(), "admin");
    assert!(bmc_user("-flag").is_err());
}

#[tokio::test]
async fn ipmi_power_status_reads_off() {
    let _guard = env_lock().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("ipmitool");
    std::fs::write(
        &tool,
        "#!/bin/sh\nif [ \"$1\" = -I ]; then echo 'Chassis Power is off'; fi\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PISTACK_IPMITOOL", &tool);
    let ctl = Ipmitool::new(
        "10.9.1.8",
        "root",
        Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
    );
    assert_eq!(ctl.power_status().await.unwrap(), ChassisPower::Off);
    std::env::remove_var("PISTACK_IPMITOOL");
}

#[tokio::test]
async fn redfish_power_status_reads_json() {
    let _guard = env_lock().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let curl = dir.path().join("curl");
    std::fs::write(
        &curl,
        "#!/bin/sh\ncase \"$*\" in *--config*) for a in \"$@\"; do [ \"$p\" = --config ] && cat \"$a\"; p=$a; done;; esac\nprintf '%s' '{\"PowerState\":\"Off\"}'\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PISTACK_CURL", &curl);
    let ctl = Redfish::new(
        "https://bmc.example/redfish/v1",
        "root",
        Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
    );
    assert_eq!(ctl.power_status().await.unwrap(), ChassisPower::Off);
    std::env::remove_var("PISTACK_CURL");
}

#[tokio::test]
async fn secret_bytes_never_appear_on_the_command_line() {
    let _guard = env_lock().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("cli.log");
    let tool = dir.path().join("ipmitool");
    std::fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$POWER_CLI_LOG\"\necho 'Chassis Power is off'\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PISTACK_IPMITOOL", &tool);
    std::env::set_var("POWER_CLI_LOG", &log);
    let ctl = Ipmitool::new(
        "10.9.1.8",
        "root",
        Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
    );
    ctl.power_status().await.unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("s3cret-do-not-log"), "{text}");
    assert!(
        text.contains("-f"),
        "ipmitool needs a password file: {text}"
    );
    std::env::remove_var("PISTACK_IPMITOOL");
    std::env::remove_var("POWER_CLI_LOG");
}

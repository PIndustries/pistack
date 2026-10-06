// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Decide how a guest interface is plugged, and build the `ovs-vsctl` argv.
//!
//! A port binding whose `vif_type` is `ovs` names an OVS bridge and an
//! `iface_id`. Anything else keeps the Linux bridge the host already uses.
//! This module only builds arguments. The runtime runs them.

use pistack_rpc::proto::{NicAttachment, NicBinding};
use pistack_types::{ApiError, ApiResult};
use std::path::{Path, PathBuf};

/// `$PISTACK_OVS_VSCTL_BIN` replaces `ovs-vsctl` on `PATH`. Tests point it
/// at a fake. An empty value is ignored.
pub(crate) const OVS_VSCTL_BIN_ENV: &str = "PISTACK_OVS_VSCTL_BIN";

/// Where the host end of a tap or veth is attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostPlug {
    /// `ip link set <ifname> master <bridge>`.
    LinuxBridge { bridge: String },
    /// `ovs-vsctl add-port <bridge> <ifname>` with `iface-id`.
    Ovs { bridge: String, iface_id: String },
}

/// Linux bridge, or the OVS bridge from an `ovs` binding.
///
/// `vif_type` other than `ovs`, including an absent binding, keeps
/// [`crate::runtime::libvirt::port_bridge`].
pub(crate) fn host_plug(nic: &NicAttachment) -> ApiResult<HostPlug> {
    if let Some(binding) = nic.binding.as_ref().filter(|b| b.vif_type == "ovs") {
        return ovs_plug(binding);
    }
    Ok(HostPlug::LinuxBridge {
        bridge: crate::runtime::libvirt::port_bridge(nic)?,
    })
}

fn ovs_plug(binding: &NicBinding) -> ApiResult<HostPlug> {
    let bridge = binding
        .vif_details
        .get("bridge")
        .map(String::as_str)
        .unwrap_or("");
    let iface_id = binding
        .vif_details
        .get("iface_id")
        .map(String::as_str)
        .unwrap_or("");
    if !bridge_ok(bridge) {
        return Err(ApiError::bad_request("ovs bridge name is not allowed"));
    }
    if !iface_id_ok(iface_id) {
        return Err(ApiError::bad_request("ovs iface-id is not allowed"));
    }
    Ok(HostPlug::Ovs {
        bridge: bridge.to_owned(),
        iface_id: iface_id.to_owned(),
    })
}

/// `ip link set <ifname> master <bridge>`.
pub(crate) fn linux_master_argv(ifname: &str, bridge: &str) -> Vec<String> {
    ["link", "set", ifname, "master", bridge]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// `ovs-vsctl --may-exist add-port <bridge> <ifname> -- set Interface ...`.
///
/// `sudo` is the `sudo` binary when the same switch that runs `ip` as
/// `sudo -n` is on. `None` runs `ovs-vsctl` directly.
pub(crate) fn plug_argv(
    sudo: Option<&str>,
    ovs_vsctl: &str,
    bridge: &str,
    ifname: &str,
    iface_id: &str,
    mac: &str,
) -> Vec<String> {
    let mut argv = command(sudo, ovs_vsctl);
    argv.extend([
        "--may-exist".to_owned(),
        "add-port".to_owned(),
        bridge.to_owned(),
        ifname.to_owned(),
        "--".to_owned(),
        "set".to_owned(),
        "Interface".to_owned(),
        ifname.to_owned(),
        format!("external_ids:iface-id={iface_id}"),
        format!("external_ids:attached-mac={mac}"),
        "external_ids:iface-status=active".to_owned(),
    ]);
    argv
}

/// `ovs-vsctl --if-exists del-port <bridge> <ifname>`. A missing port
/// succeeds. `sudo` matches [`plug_argv`].
pub(crate) fn unplug_argv(
    sudo: Option<&str>,
    ovs_vsctl: &str,
    bridge: &str,
    ifname: &str,
) -> Vec<String> {
    let mut argv = command(sudo, ovs_vsctl);
    argv.extend([
        "--if-exists".to_owned(),
        "del-port".to_owned(),
        bridge.to_owned(),
        ifname.to_owned(),
    ]);
    argv
}

fn command(sudo: Option<&str>, ovs_vsctl: &str) -> Vec<String> {
    match sudo {
        Some(sudo) => vec![sudo.to_owned(), "-n".to_owned(), ovs_vsctl.to_owned()],
        None => vec![ovs_vsctl.to_owned()],
    }
}

/// OVS bridge name: a letter, then letters, digits, `_` or `-`, at most 15.
pub(crate) fn bridge_ok(name: &str) -> bool {
    let len = name.len();
    (1..=15).contains(&len)
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Port id string used as `external_ids:iface-id`. Letters, digits, `_`, `-`.
pub(crate) fn iface_id_ok(id: &str) -> bool {
    let len = id.len();
    (1..=64).contains(&len)
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// True when `ovs-vsctl` is reporting that the port is already gone.
pub(crate) fn already_gone(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("no port named")
        || msg.contains("no such port")
        || msg.contains("does not exist")
        || msg.contains("no such device")
        || msg.contains("not found")
}

/// `ovs-vsctl` binary. `$PISTACK_OVS_VSCTL_BIN` wins. A set value that is
/// not executable is an error and is not replaced with one from `PATH`.
pub(crate) fn ovs_vsctl_bin() -> ApiResult<PathBuf> {
    if let Some(value) = std::env::var_os(OVS_VSCTL_BIN_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(&value);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable("ovs-vsctl is not available"));
        }
    }
    crate::facts::which("ovs-vsctl")
        .ok_or_else(|| ApiError::unavailable("ovs-vsctl is not available"))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const IFACE: &str = "port-018f3a2b-1c4d-7e5f-8a6b-0c1d2e3f4a5b";
    const MAC: &str = "52:54:00:aa:bb:cc";

    fn nic(binding: Option<NicBinding>) -> NicAttachment {
        NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: MAC.into(),
            address: "10.90.0.10".into(),
            prefix: 24,
            gateway: "10.90.0.1".into(),
            routed: true,
            segment_id: None,
            binding,
        }
    }

    fn details(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn plug_args() -> Vec<String> {
        [
            "--may-exist",
            "add-port",
            "br-int",
            "ct525400aabbcc",
            "--",
            "set",
            "Interface",
            "ct525400aabbcc",
            &format!("external_ids:iface-id={IFACE}"),
            &format!("external_ids:attached-mac={MAC}"),
            "external_ids:iface-status=active",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn plug_and_unplug_argv_with_and_without_sudo() {
        let mut direct = vec!["ovs-vsctl".to_owned()];
        direct.extend(plug_args());
        assert_eq!(
            plug_argv(None, "ovs-vsctl", "br-int", "ct525400aabbcc", IFACE, MAC),
            direct
        );

        let mut sudo = vec!["sudo".to_owned(), "-n".to_owned(), "ovs-vsctl".to_owned()];
        sudo.extend(plug_args());
        assert_eq!(
            plug_argv(
                Some("sudo"),
                "ovs-vsctl",
                "br-int",
                "ct525400aabbcc",
                IFACE,
                MAC
            ),
            sudo
        );

        assert_eq!(
            unplug_argv(None, "ovs-vsctl", "br-int", "vh525400aabbcc"),
            vec![
                "ovs-vsctl",
                "--if-exists",
                "del-port",
                "br-int",
                "vh525400aabbcc",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        );
        assert_eq!(
            unplug_argv(
                Some("/usr/bin/sudo"),
                "/usr/bin/ovs-vsctl",
                "br-int",
                "ct525400aabbcc"
            ),
            vec![
                "/usr/bin/sudo",
                "-n",
                "/usr/bin/ovs-vsctl",
                "--if-exists",
                "del-port",
                "br-int",
                "ct525400aabbcc",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_non_ovs_binding_keeps_the_linux_bridge() {
        let cases = [
            None,
            Some(NicBinding {
                vif_type: "hostnat".into(),
                vif_details: BTreeMap::new(),
            }),
            Some(NicBinding {
                vif_type: "overlay".into(),
                vif_details: BTreeMap::new(),
            }),
            Some(NicBinding {
                vif_type: "mock".into(),
                vif_details: details(&[("bridge", "br-int")]),
            }),
            Some(NicBinding {
                vif_type: String::new(),
                vif_details: details(&[("bridge", "br-int"), ("iface_id", IFACE)]),
            }),
        ];
        for binding in cases {
            let plug = host_plug(&nic(binding)).unwrap();
            let HostPlug::LinuxBridge { bridge } = plug else {
                panic!("non-ovs binding must stay on the linux bridge");
            };
            assert_eq!(bridge, "pi01a102de3f33");
            assert_eq!(
                linux_master_argv("ct525400aabbcc", &bridge),
                vec!["link", "set", "ct525400aabbcc", "master", "pi01a102de3f33"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            );
        }

        let mut overlay = nic(None);
        overlay.segment_id = Some(4);
        let HostPlug::LinuxBridge { bridge } = host_plug(&overlay).unwrap() else {
            panic!("overlay segment stays on its linux bridge");
        };
        assert_eq!(bridge, "po01a102de3f33");
        assert_eq!(
            linux_master_argv("vh525400aabbcc", &bridge),
            ["link", "set", "vh525400aabbcc", "master", "po01a102de3f33"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_ovs_binding_uses_the_named_bridge_and_iface_id() {
        let mut nic = nic(Some(NicBinding {
            vif_type: "ovs".into(),
            vif_details: details(&[("bridge", "br-int"), ("iface_id", IFACE)]),
        }));
        nic.segment_id = Some(4);
        let HostPlug::Ovs { bridge, iface_id } = host_plug(&nic).unwrap() else {
            panic!("ovs binding must not use the linux bridge");
        };
        assert_eq!(bridge, "br-int");
        assert_eq!(iface_id, IFACE);
        assert_eq!(
            plug_argv(None, "ovs-vsctl", &bridge, "ct525400aabbcc", &iface_id, MAC),
            {
                let mut argv = vec!["ovs-vsctl".to_owned()];
                argv.extend(plug_args());
                argv
            }
        );

        nic.binding.as_mut().unwrap().vif_details.remove("iface_id");
        let err = host_plug(&nic).unwrap_err();
        assert!(err.message.contains("iface-id"), "{err}");
        nic.binding
            .as_mut()
            .unwrap()
            .vif_details
            .insert("iface_id".into(), IFACE.into());
        nic.binding
            .as_mut()
            .unwrap()
            .vif_details
            .insert("bridge".into(), "br int".into());
        let err = host_plug(&nic).unwrap_err();
        assert!(err.message.contains("bridge"), "{err}");
    }
}

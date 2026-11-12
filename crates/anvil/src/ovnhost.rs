// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Make this host an OVN chassis, and say whether it is one.
//!
//! The server hands out the southbound address and the tunnel type in its
//! hello answer. [`OvnHost::configure`] writes them to the local Open vSwitch
//! database, with this host's underlay address as the tunnel endpoint, and
//! creates the integration bridge. It writes only what differs, so a host
//! that is already set up is left alone. `system-id` is written only when it
//! is empty: changing it would register the host as a second chassis.
//!
//! [`OvnHost::check`] is what the heartbeat reports: the chassis name,
//! whether `br-int` exists and `ovn-controller` is connected, and the bridge
//! mappings. A machine with an OVN port is refused with `unavailable` and
//! the reason while the host is not ready ([`OvnHost::require_ready`]).
//!
//! This module does not install or start `ovsdb-server`, `ovs-vswitchd` or
//! `ovn-controller`, and it does not create provider bridges.
//!
//! # Privilege
//!
//! A root agent runs `ovs-vsctl` and `ovn-appctl` directly. When the agent
//! is not root and `PISTACK_OVN_SUDO` is `1`, `true`, or `yes`, both run as
//! `sudo -n`. Otherwise the host reports not ready and says so.
//! `$PISTACK_OVN_SUDO_BIN` replaces `sudo`, `$PISTACK_OVS_VSCTL_BIN` and
//! `$PISTACK_OVN_APPCTL_BIN` replace the two tools. Empty values are ignored.

use async_trait::async_trait;
use pistack_rpc::proto::{OvnHostConfig, OvnReport};
use pistack_types::{ApiError, ApiResult};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::runtime::ovsplug;

const SUDO_ENV: &str = "PISTACK_OVN_SUDO";
const SUDO_BIN_ENV: &str = "PISTACK_OVN_SUDO_BIN";
const APPCTL_BIN_ENV: &str = "PISTACK_OVN_APPCTL_BIN";
/// `physnet:bridge` pairs this host offers, comma separated. When set the
/// agent records them and marks the host as able to carry gateway ports.
pub(crate) const BRIDGE_MAPPINGS_ENV: &str = "PISTACK_OVN_BRIDGE_MAPPINGS";
const SUDOERS_SAMPLE: &str = "pistack ALL=(root) NOPASSWD: /usr/bin/ovs-vsctl, /usr/bin/ovn-appctl";

/// The bridge `ovn-controller` programs. The port binding names the same one.
const INTEGRATION_BRIDGE: &str = "br-int";
/// `ovn-cms-options` entry that lets a chassis host gateway ports.
const GATEWAY_OPTION: &str = "enable-chassis-as-gw";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs one command and returns its standard output. A non-zero exit is an
/// error carrying standard error. Tests replace it with an in-memory switch.
#[async_trait]
pub(crate) trait Runner: Send + Sync {
    async fn run(&self, argv: &[String]) -> ApiResult<String>;
}

struct SystemRunner;

#[async_trait]
impl Runner for SystemRunner {
    async fn run(&self, argv: &[String]) -> ApiResult<String> {
        let Some((program, args)) = argv.split_first() else {
            return Err(ApiError::internal("ovn host: empty command"));
        };
        let run = tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(COMMAND_TIMEOUT, run).await {
            Ok(Ok(out)) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
            }
            Ok(Ok(out)) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let err = err.trim();
                Err(ApiError::unavailable(if err.is_empty() {
                    format!("{program} failed ({})", out.status)
                } else {
                    err.to_owned()
                }))
            }
            Ok(Err(e)) => Err(ApiError::unavailable(format!("{program} did not run: {e}"))),
            Err(_) => Err(ApiError::unavailable(format!("{program} timed out"))),
        }
    }
}

/// The two tools as argv prefixes, `sudo -n` included when it applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tools {
    pub(crate) vsctl: Vec<String>,
    pub(crate) appctl: Vec<String>,
}

impl Tools {
    fn from_system() -> ApiResult<Self> {
        let sudo = sudo_prefix(agent_is_root(), env_flag(SUDO_ENV), sudo_bin())?;
        let vsctl = ovsplug::ovs_vsctl_bin()?;
        let appctl = tool(APPCTL_BIN_ENV, "ovn-appctl")?;
        let with = |bin: PathBuf| {
            let mut argv = sudo.clone();
            argv.push(bin.to_string_lossy().into_owned());
            argv
        };
        Ok(Self {
            vsctl: with(vsctl),
            appctl: with(appctl),
        })
    }
}

/// Nothing for root, `sudo -n` for a non-root agent with the switch on, and
/// an error that names the switch otherwise. Root ignores the switch.
fn sudo_prefix(is_root: bool, flag: bool, sudo: Option<PathBuf>) -> ApiResult<Vec<String>> {
    if is_root {
        return Ok(Vec::new());
    }
    if !flag {
        return Err(ApiError::unavailable(format!(
            "the agent is not root, so it cannot set up Open vSwitch for OVN. Run the agent as root, or set {SUDO_ENV}=1 and allow `{SUDOERS_SAMPLE}`"
        )));
    }
    match sudo {
        Some(sudo) => Ok(vec![sudo.to_string_lossy().into_owned(), "-n".to_owned()]),
        None => Err(ApiError::unavailable(format!(
            "{SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
        ))),
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn agent_is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid does not fail and does not touch Rust memory.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sudo_bin() -> Option<PathBuf> {
    match std::env::var_os(SUDO_BIN_ENV) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => crate::facts::which("sudo"),
    }
}

fn tool(env_key: &str, name: &str) -> ApiResult<PathBuf> {
    match std::env::var_os(env_key) {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => crate::facts::which(name)
            .ok_or_else(|| ApiError::unavailable(format!("{name} is not available"))),
    }
}

/// `physnet:bridge` pairs from `PISTACK_OVN_BRIDGE_MAPPINGS`, if any.
pub(crate) fn bridge_mappings_from_env() -> Option<String> {
    std::env::var(BRIDGE_MAPPINGS_ENV)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// This host's Open vSwitch and `ovn-controller`.
pub(crate) struct OvnHost {
    runner: Arc<dyn Runner>,
    /// `None` resolves the tools from the environment on each use.
    tools: Option<Tools>,
    /// Why the last [`OvnHost::configure`] failed, if it did.
    setup_error: Mutex<Option<String>>,
}

impl OvnHost {
    /// The real tools on this host.
    pub(crate) fn system() -> Self {
        Self {
            runner: Arc::new(SystemRunner),
            tools: None,
            setup_error: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn with(runner: Arc<dyn Runner>, tools: Tools) -> Self {
        Self {
            runner,
            tools: Some(tools),
            setup_error: Mutex::new(None),
        }
    }

    fn tools(&self) -> ApiResult<Tools> {
        match &self.tools {
            Some(t) => Ok(t.clone()),
            None => Tools::from_system(),
        }
    }

    fn set_setup_error(&self, error: Option<String>) {
        *self.setup_error.lock().unwrap_or_else(|e| e.into_inner()) = error;
    }

    fn setup_error(&self) -> Option<String> {
        self.setup_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    async fn vsctl(&self, tools: &Tools, args: &[&str]) -> ApiResult<String> {
        let mut argv = tools.vsctl.clone();
        argv.extend(args.iter().map(|a| (*a).to_owned()));
        self.runner.run(&argv).await
    }

    /// One key of `Open_vSwitch.external_ids`. A missing key is empty.
    async fn external_id(&self, tools: &Tools, key: &str) -> ApiResult<String> {
        let column = format!("external_ids:{key}");
        let out = self
            .vsctl(tools, &["--if-exists", "get", "open_vswitch", ".", &column])
            .await?;
        Ok(unquote(&out))
    }

    /// Point this host at the cluster's OVN. `encap_ip` is the host's
    /// underlay address. `mappings` are the operator's `physnet:bridge`
    /// pairs, if this host has an uplink. A failure is kept and shows as
    /// the reason in [`OvnHost::check`] until a later call succeeds.
    pub(crate) async fn configure(
        &self,
        cfg: &OvnHostConfig,
        encap_ip: Option<&str>,
        node_name: &str,
        mappings: Option<&str>,
    ) -> ApiResult<()> {
        let done = self.apply(cfg, encap_ip, node_name, mappings).await;
        self.set_setup_error(done.as_ref().err().map(|e| e.message.clone()));
        done
    }

    async fn apply(
        &self,
        cfg: &OvnHostConfig,
        encap_ip: Option<&str>,
        node_name: &str,
        mappings: Option<&str>,
    ) -> ApiResult<()> {
        let remote = cfg.sb_remote.trim();
        if !remote_ok(remote) {
            return Err(ApiError::unavailable(
                "the southbound address from the server is not a tcp:, ssl: or unix: remote",
            ));
        }
        let encap_type = cfg.encap_type.trim();
        if !matches!(encap_type, "geneve" | "vxlan") {
            return Err(ApiError::unavailable(format!(
                "tunnel type '{encap_type}' from the server is not geneve or vxlan"
            )));
        }
        let encap_ip = encap_ip
            .and_then(|ip| ip.trim().parse::<Ipv4Addr>().ok())
            .ok_or_else(|| {
                ApiError::unavailable(
                    "this host has no underlay address to use as the tunnel endpoint. Set PISTACK_UNDERLAY_IP",
                )
            })?
            .to_string();
        if let Some(m) = mappings {
            if !mappings_ok(m) {
                return Err(ApiError::unavailable(format!(
                    "{BRIDGE_MAPPINGS_ENV} is not a list of physnet:bridge pairs"
                )));
            }
        }
        let tools = self.tools()?;

        let mut want = vec![
            ("ovn-remote", remote.to_owned()),
            ("ovn-encap-type", encap_type.to_owned()),
            ("ovn-encap-ip", encap_ip),
        ];
        if let Some(m) = mappings {
            want.push(("ovn-bridge-mappings", m.to_owned()));
        }
        let mut set = Vec::new();
        for (key, value) in want {
            if self.external_id(&tools, key).await? != value {
                set.push(format!("external_ids:{key}=\"{value}\""));
            }
        }
        if self.external_id(&tools, "system-id").await?.is_empty() {
            if !system_id_ok(node_name) {
                return Err(ApiError::unavailable(
                    "the node name cannot be used as the chassis name; set external_ids:system-id by hand",
                ));
            }
            set.push(format!("external_ids:system-id=\"{node_name}\""));
        }
        if mappings.is_some() {
            let options = self.external_id(&tools, "ovn-cms-options").await?;
            if !options.split(',').any(|o| o.trim() == GATEWAY_OPTION) {
                let options = if options.is_empty() {
                    GATEWAY_OPTION.to_owned()
                } else {
                    format!("{options},{GATEWAY_OPTION}")
                };
                set.push(format!("external_ids:ovn-cms-options=\"{options}\""));
            }
        }
        if !set.is_empty() {
            let mut args = vec!["set", "open_vswitch", "."];
            args.extend(set.iter().map(String::as_str));
            self.vsctl(&tools, &args).await?;
            tracing::info!(remote, encap_type, "anvil: pointed Open vSwitch at OVN");
        }
        self.vsctl(
            &tools,
            &[
                "--may-exist",
                "add-br",
                INTEGRATION_BRIDGE,
                "--",
                "set",
                "bridge",
                INTEGRATION_BRIDGE,
                "fail-mode=secure",
                "other-config:disable-in-band=true",
            ],
        )
        .await?;
        Ok(())
    }

    /// OVN on this host right now.
    pub(crate) async fn check(&self) -> OvnReport {
        let mut report = OvnReport::default();
        match self.probe(&mut report).await {
            Ok(()) => match self.setup_error() {
                None => report.ready = true,
                Some(e) => report.message = e,
            },
            Err(e) => {
                report.message = match self.setup_error() {
                    Some(setup) if setup != e.message => format!("{setup}; {}", e.message),
                    _ => e.message,
                }
            }
        }
        report
    }

    async fn probe(&self, report: &mut OvnReport) -> ApiResult<()> {
        let tools = self.tools()?;
        report.chassis = self
            .external_id(&tools, "system-id")
            .await
            .map_err(|e| unavailable(format!("Open vSwitch did not answer: {}", e.message)))?;
        report.bridge_mappings = self.external_id(&tools, "ovn-bridge-mappings").await?;
        let remote = self.external_id(&tools, "ovn-remote").await?;
        if remote.is_empty() {
            return Err(unavailable(
                "external_ids:ovn-remote is not set, so ovn-controller has no southbound database",
            ));
        }
        if report.chassis.is_empty() {
            return Err(unavailable("external_ids:system-id is not set"));
        }
        if self
            .vsctl(&tools, &["br-exists", INTEGRATION_BRIDGE])
            .await
            .is_err()
        {
            return Err(unavailable(format!(
                "integration bridge {INTEGRATION_BRIDGE} does not exist"
            )));
        }
        let mut argv = tools.appctl.clone();
        argv.extend(["-t", "ovn-controller", "connection-status"].map(str::to_owned));
        match self.runner.run(&argv).await {
            Ok(status) if status.trim() == "connected" => Ok(()),
            Ok(status) => Err(unavailable(format!(
                "ovn-controller is not connected to the southbound database at {remote} (status: {})",
                status.trim()
            ))),
            Err(e) => Err(unavailable(format!(
                "ovn-controller did not answer; is it running? {}",
                e.message
            ))),
        }
    }

    /// `unavailable` with the reason unless this host is a working chassis.
    pub(crate) async fn require_ready(&self) -> ApiResult<()> {
        let report = self.check().await;
        if report.ready {
            return Ok(());
        }
        Err(unavailable(format!(
            "this host is not ready for OVN networks: {}",
            report.message
        )))
    }
}

fn unavailable(message: impl Into<String>) -> ApiError {
    ApiError::unavailable(message)
}

/// `ovs-vsctl get` prints a string in double quotes unless it is a plain
/// word.
fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    raw.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw)
        .to_owned()
}

/// One or more `tcp:`, `ssl:` or `unix:` remotes, comma separated, made of
/// characters that need no quoting.
fn remote_ok(remote: &str) -> bool {
    !remote.is_empty()
        && remote.len() <= 512
        && remote.split(',').all(|r| {
            ["tcp:", "ssl:", "unix:"]
                .iter()
                .any(|p| r.len() > p.len() && r.starts_with(p))
                && r.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ":./[]-_".contains(c))
        })
}

fn system_id_ok(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn mappings_ok(mappings: &str) -> bool {
    mappings.split(',').all(|pair| {
        pair.split_once(':').is_some_and(|(physnet, bridge)| {
            (1..=64).contains(&physnet.len())
                && physnet
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && ovsplug::bridge_ok(bridge)
        })
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// An Open vSwitch database and an `ovn-controller` that live in memory
    /// and answer the commands this module sends.
    #[derive(Default)]
    pub(crate) struct FakeSwitch {
        state: Mutex<State>,
    }

    #[derive(Default)]
    struct State {
        external_ids: BTreeMap<String, String>,
        bridges: BTreeSet<String>,
        /// What `connection-status` prints. `None` is a controller that is
        /// not running.
        controller: Option<String>,
        /// When set, every `ovs-vsctl` call fails with this.
        vsctl_down: Option<String>,
        log: Vec<Vec<String>>,
    }

    impl FakeSwitch {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub(crate) fn set(&self, key: &str, value: &str) {
            self.lock().external_ids.insert(key.into(), value.into());
        }

        pub(crate) fn get(&self, key: &str) -> Option<String> {
            self.lock().external_ids.get(key).cloned()
        }

        pub(crate) fn controller(&self, status: Option<&str>) {
            self.lock().controller = status.map(str::to_owned);
        }

        pub(crate) fn has_bridge(&self, name: &str) -> bool {
            self.lock().bridges.contains(name)
        }

        fn vsctl_down(&self, message: &str) {
            self.lock().vsctl_down = Some(message.into());
        }

        /// Every command run so far, and forget them.
        fn take_log(&self) -> Vec<Vec<String>> {
            std::mem::take(&mut self.lock().log)
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, State> {
            self.state.lock().unwrap()
        }
    }

    #[async_trait]
    impl Runner for FakeSwitch {
        async fn run(&self, argv: &[String]) -> ApiResult<String> {
            let mut st = self.lock();
            st.log.push(argv.to_vec());
            let args: Vec<&str> = argv
                .iter()
                .map(String::as_str)
                .skip_while(|a| *a == "sudo" || *a == "-n")
                .collect();
            match args.as_slice() {
                ["ovn-appctl", "-t", "ovn-controller", "connection-status"] => st
                    .controller
                    .clone()
                    .ok_or_else(|| ApiError::unavailable("cannot connect to ovn-controller.ctl")),
                ["ovs-vsctl", rest @ ..] => {
                    if let Some(down) = &st.vsctl_down {
                        return Err(ApiError::unavailable(down.clone()));
                    }
                    match rest {
                        ["--if-exists", "get", "open_vswitch", ".", column] => {
                            let key = column.strip_prefix("external_ids:").unwrap();
                            Ok(match st.external_ids.get(key) {
                                Some(v) => format!("\"{v}\""),
                                None => String::new(),
                            })
                        }
                        ["set", "open_vswitch", ".", settings @ ..] => {
                            for s in settings {
                                let (key, value) = s
                                    .strip_prefix("external_ids:")
                                    .and_then(|s| s.split_once('='))
                                    .unwrap();
                                st.external_ids.insert(key.into(), unquote(value));
                            }
                            Ok(String::new())
                        }
                        ["br-exists", name] => {
                            if st.bridges.contains(*name) {
                                Ok(String::new())
                            } else {
                                Err(ApiError::unavailable("ovs-vsctl failed (exit status: 2)"))
                            }
                        }
                        ["--may-exist", "add-br", name, "--", "set", "bridge", ..] => {
                            st.bridges.insert((*name).to_owned());
                            Ok(String::new())
                        }
                        other => panic!("unexpected ovs-vsctl {other:?}"),
                    }
                }
                other => panic!("unexpected command {other:?}"),
            }
        }
    }

    pub(crate) fn tools(sudo: bool) -> Tools {
        let with = |bin: &str| {
            let mut argv = if sudo {
                vec!["sudo".to_owned(), "-n".to_owned()]
            } else {
                Vec::new()
            };
            argv.push(bin.to_owned());
            argv
        };
        Tools {
            vsctl: with("ovs-vsctl"),
            appctl: with("ovn-appctl"),
        }
    }

    pub(crate) fn cfg() -> OvnHostConfig {
        OvnHostConfig {
            sb_remote: "tcp:10.66.66.1:6642".into(),
            encap_type: "geneve".into(),
        }
    }

    fn host(switch: &Arc<FakeSwitch>) -> OvnHost {
        OvnHost::with(switch.clone(), tools(false))
    }

    fn writes(log: &[Vec<String>]) -> Vec<&Vec<String>> {
        log.iter()
            .filter(|argv| argv.iter().any(|a| a == "set"))
            .filter(|argv| !argv.iter().any(|a| a == "add-br"))
            .collect()
    }

    #[tokio::test]
    async fn a_bare_host_becomes_a_chassis_and_a_repeat_writes_nothing() {
        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        let host = host(&switch);

        let before = host.check().await;
        assert!(!before.ready);
        assert!(before.message.contains("ovn-remote"), "{}", before.message);

        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();
        assert_eq!(
            switch.get("ovn-remote").as_deref(),
            Some("tcp:10.66.66.1:6642")
        );
        assert_eq!(switch.get("ovn-encap-type").as_deref(), Some("geneve"));
        assert_eq!(switch.get("ovn-encap-ip").as_deref(), Some("10.66.66.5"));
        assert_eq!(switch.get("system-id").as_deref(), Some("node-a"));
        assert_eq!(switch.get("ovn-bridge-mappings"), None);
        assert_eq!(switch.get("ovn-cms-options"), None);
        assert!(switch.has_bridge("br-int"));

        let log = switch.take_log();
        let set = writes(&log);
        assert_eq!(set.len(), 1, "one set for every key: {log:?}");
        assert_eq!(
            set[0][..4],
            ["ovs-vsctl", "set", "open_vswitch", "."].map(str::to_owned)
        );
        assert!(set[0].contains(&"external_ids:ovn-remote=\"tcp:10.66.66.1:6642\"".to_owned()));
        assert!(log.iter().any(|argv| argv[1..]
            == [
                "--may-exist",
                "add-br",
                "br-int",
                "--",
                "set",
                "bridge",
                "br-int",
                "fail-mode=secure",
                "other-config:disable-in-band=true"
            ]
            .map(str::to_owned)));

        let report = host.check().await;
        assert_eq!(
            report,
            OvnReport {
                chassis: "node-a".into(),
                ready: true,
                message: String::new(),
                bridge_mappings: String::new(),
            }
        );
        host.require_ready().await.unwrap();

        switch.take_log();
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();
        assert!(
            writes(&switch.take_log()).is_empty(),
            "a repeat must not set"
        );
    }

    #[tokio::test]
    async fn an_existing_system_id_is_kept_and_a_changed_remote_is_rewritten() {
        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        switch.set("system-id", "3f6c2c5e-chassis");
        switch.set("ovn-remote", "tcp:10.0.0.9:6642");
        switch.set("ovn-encap-type", "geneve");
        switch.set("ovn-encap-ip", "10.66.66.5");
        let host = host(&switch);
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();
        assert_eq!(switch.get("system-id").as_deref(), Some("3f6c2c5e-chassis"));
        let log = switch.take_log();
        let set = writes(&log);
        assert_eq!(set.len(), 1);
        assert_eq!(
            set[0][4..],
            ["external_ids:ovn-remote=\"tcp:10.66.66.1:6642\"".to_owned()]
        );
        assert_eq!(host.check().await.chassis, "3f6c2c5e-chassis");
    }

    #[tokio::test]
    async fn bridge_mappings_mark_the_host_as_a_gateway_and_keep_other_options() {
        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        switch.set("ovn-cms-options", "availability-zones=az1");
        let host = host(&switch);
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", Some("physnet1:br-ex"))
            .await
            .unwrap();
        assert_eq!(
            switch.get("ovn-bridge-mappings").as_deref(),
            Some("physnet1:br-ex")
        );
        assert_eq!(
            switch.get("ovn-cms-options").as_deref(),
            Some("availability-zones=az1,enable-chassis-as-gw")
        );
        assert!(
            !switch.has_bridge("br-ex"),
            "provider bridges are the operator's"
        );
        assert_eq!(host.check().await.bridge_mappings, "physnet1:br-ex");

        switch.take_log();
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", Some("physnet1:br-ex"))
            .await
            .unwrap();
        assert!(writes(&switch.take_log()).is_empty());

        let err = host
            .configure(&cfg(), Some("10.66.66.5"), "node-a", Some("physnet1"))
            .await
            .unwrap_err();
        assert!(err.message.contains(BRIDGE_MAPPINGS_ENV), "{err}");
    }

    #[tokio::test]
    async fn sudo_wraps_both_tools() {
        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        let host = OvnHost::with(switch.clone(), tools(true));
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();
        assert!(host.check().await.ready);
        let log = switch.take_log();
        assert!(!log.is_empty());
        for argv in &log {
            assert_eq!(argv[..2], ["sudo".to_owned(), "-n".to_owned()], "{argv:?}");
        }
        assert!(log.iter().any(|argv| argv[2] == "ovn-appctl"));
    }

    #[tokio::test]
    async fn each_missing_piece_is_named() {
        let switch = FakeSwitch::new();
        let host = host(&switch);
        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();

        // ovn-controller is not running.
        let report = host.check().await;
        assert!(!report.ready);
        assert_eq!(report.chassis, "node-a");
        assert!(
            report.message.contains("is it running"),
            "{}",
            report.message
        );
        let err = host.require_ready().await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
        assert!(err.message.contains("not ready for OVN"), "{err}");

        // Running, but it cannot reach the southbound database.
        switch.controller(Some("not connected"));
        let report = host.check().await;
        assert!(!report.ready);
        assert!(
            report.message.contains("tcp:10.66.66.1:6642")
                && report.message.contains("not connected"),
            "{}",
            report.message
        );

        switch.controller(Some("connected"));
        assert!(host.check().await.ready);

        // Someone deleted the bridge.
        switch.lock().bridges.clear();
        let report = host.check().await;
        assert!(report.message.contains("br-int"), "{}", report.message);

        // ovsdb-server is down.
        switch.vsctl_down("database connection failed (No such file or directory)");
        let report = host.check().await;
        assert!(!report.ready);
        assert!(report.message.contains("Open vSwitch did not answer"));
    }

    #[tokio::test]
    async fn a_failed_setup_is_the_reported_reason_until_one_succeeds() {
        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        // The operator set the host up by hand, so a check alone passes.
        switch.set("system-id", "node-a");
        switch.set("ovn-remote", "tcp:10.0.0.9:6642");
        switch.lock().bridges.insert("br-int".into());
        let host = host(&switch);
        assert!(host.check().await.ready);

        let err = host
            .configure(&cfg(), None, "node-a", None)
            .await
            .unwrap_err();
        assert!(err.message.contains("underlay"), "{err}");
        let report = host.check().await;
        assert!(!report.ready);
        assert!(report.message.contains("underlay"), "{}", report.message);
        assert_eq!(
            switch.get("ovn-remote").as_deref(),
            Some("tcp:10.0.0.9:6642")
        );

        for (remote, encap) in [
            ("10.66.66.1:6642", "geneve"),
            ("tcp:10.66.66.1:6642; reboot", "geneve"),
            ("tcp:", "geneve"),
            ("tcp:10.66.66.1:6642", "gre"),
        ] {
            let bad = OvnHostConfig {
                sb_remote: remote.into(),
                encap_type: encap.into(),
            };
            host.configure(&bad, Some("10.66.66.5"), "node-a", None)
                .await
                .unwrap_err();
        }
        assert_eq!(
            switch.get("ovn-remote").as_deref(),
            Some("tcp:10.0.0.9:6642")
        );

        host.configure(&cfg(), Some("10.66.66.5"), "node-a", None)
            .await
            .unwrap();
        assert!(host.check().await.ready);
    }

    #[test]
    fn privilege_follows_root_then_the_switch() {
        assert!(sudo_prefix(true, false, None).unwrap().is_empty());
        // Root ignores the switch.
        assert!(sudo_prefix(true, true, Some("/usr/bin/sudo".into()))
            .unwrap()
            .is_empty());
        assert_eq!(
            sudo_prefix(false, true, Some("/usr/bin/sudo".into())).unwrap(),
            vec!["/usr/bin/sudo".to_owned(), "-n".to_owned()]
        );
        let err = sudo_prefix(false, false, Some("/usr/bin/sudo".into())).unwrap_err();
        assert!(err.message.contains(SUDO_ENV), "{err}");
        assert!(err.message.contains("ovn-appctl"), "{err}");
        let err = sudo_prefix(false, true, None).unwrap_err();
        assert!(err.message.contains("sudo is not available"), "{err}");
    }

    #[test]
    fn remotes_and_mappings_are_checked() {
        for ok in [
            "tcp:10.0.0.11:6642",
            "tcp:10.0.0.11:6642,tcp:10.0.0.12:6642",
            "ssl:[fd00::1]:6642",
            "unix:/var/run/ovn/ovnsb_db.sock",
        ] {
            assert!(remote_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "tcp:",
            "10.0.0.11:6642",
            "tcp:a b",
            "tcp:a\"b",
            "tcp:a,",
        ] {
            assert!(!remote_ok(bad), "{bad}");
        }
        assert!(mappings_ok("physnet1:br-ex"));
        assert!(mappings_ok("physnet1:br-ex,physnet2:br-provider"));
        assert!(!mappings_ok("physnet1"));
        assert!(!mappings_ok("physnet1:br ex"));
        assert!(!mappings_ok("physnet1:br-ex,"));
        assert_eq!(unquote("\"tcp:10.0.0.1:6642\"\n"), "tcp:10.0.0.1:6642");
        assert_eq!(unquote("geneve"), "geneve");
        assert_eq!(unquote(""), "");
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Single-host floating IPs. Guests start on the libvirt NAT network.
//! `scripts/pistack-fip.py` installs each floating address on the underlay
//! interface named by its environment. That helper is the privilege
//! boundary: it refuses any address outside the configured pool, any fixed
//! address outside the guest CIDR allow-list, and the public NIC.
//!
//! `ensure_subnet` adds a non-external IPv4 CIDR to that allow-list.
//! External networks are not added. Networks, ports, and
//! routers have no virtual switch here, so those calls succeed and the
//! reconciler can mark them ready. Floating IPs and security groups change
//! the host: groups land in the filter chain `PISTACK-SG` through the same
//! helper. Metadata redirect is anvil's job, after the bridge exists.

use super::{FipView, NetBackend, RouterView};
use crate::types::{
    Direction, EtherType, Network, Port, PortBinding, Protocol, Remote, Rule, SecurityGroup, Subnet,
};
use async_trait::async_trait;
use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult, Id};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Each group's fixed addresses and the rules it gives them, as last applied
/// by this process. The helper holds one rule set per address and replaces
/// it whole, so an address in several groups is sent the rules of all of
/// them together. `delete_security_group` is only handed an id: the ports
/// have already left the group.
#[derive(Clone, Debug, Default)]
struct Applied {
    by_group: HashMap<Id, (Vec<IpAddr>, SgPolicy)>,
}

impl Applied {
    /// The rules of every group that holds `ip`, or `None` when no group
    /// does. Groups are taken in id order so the same set of groups always
    /// gives the same rules.
    fn policy_for(&self, ip: IpAddr) -> Option<SgPolicy> {
        let mut groups: Vec<_> = self
            .by_group
            .iter()
            .filter(|(_, (targets, _))| targets.contains(&ip))
            .collect();
        if groups.is_empty() {
            return None;
        }
        groups.sort_by(|a, b| a.0.cmp(b.0));
        let mut merged = SgPolicy::default();
        for (_, (_, policy)) in groups {
            for rule in &policy.ingress {
                if !merged.ingress.contains(rule) {
                    merged.ingress.push(rule.clone());
                }
            }
            for rule in &policy.egress {
                if !merged.egress.contains(rule) {
                    merged.egress.push(rule.clone());
                }
            }
        }
        Some(merged)
    }
}

/// Runs `pistack-fip` (via sudo, unless a test points at its own program).
#[derive(Clone, Debug)]
pub struct HostNat {
    program: PathBuf,
    sudo: bool,
    applied: Arc<Mutex<Applied>>,
}

impl HostNat {
    pub fn new() -> Self {
        Self {
            program: PathBuf::from("/usr/local/sbin/pistack-fip"),
            sudo: true,
            applied: Arc::new(Mutex::new(Applied::default())),
        }
    }

    /// Call `program` directly. Tests use this.
    pub fn command(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            sudo: false,
            applied: Arc::new(Mutex::new(Applied::default())),
        }
    }

    fn applied(&self) -> std::sync::MutexGuard<'_, Applied> {
        self.applied.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn run_stdin(&self, args: &[String], stdin_body: Option<&[u8]>) -> ApiResult<()> {
        let mut cmd = if self.sudo {
            let mut c = Command::new("sudo");
            c.arg("-n").arg(&self.program);
            c
        } else {
            Command::new(&self.program)
        };
        cmd.args(args)
            .stdin(if stdin_body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            ApiError::unavailable(format!(
                "floating ip helper {}: {e}",
                self.program.display()
            ))
        })?;
        if let Some(body) = stdin_body {
            if let Some(mut sin) = child.stdin.take() {
                if let Err(e) = sin.write_all(body).await {
                    let _ = child.kill().await;
                    return Err(ApiError::unavailable(format!(
                        "floating ip helper {}: {e}",
                        self.program.display()
                    )));
                }
            }
        }
        let out = child.wait_with_output().await.map_err(|e| {
            ApiError::unavailable(format!(
                "floating ip helper {}: {e}",
                self.program.display()
            ))
        })?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        let extra = String::from_utf8_lossy(&out.stdout);
        Err(ApiError::internal(format!(
            "floating ip helper failed: {}{}",
            err.trim(),
            extra.trim()
        )))
    }

    async fn run(&self, args: &[String]) -> ApiResult<()> {
        self.run_stdin(args, None).await
    }

    async fn sg_policy(&self, ip: IpAddr, body: &[u8]) -> ApiResult<()> {
        self.run_stdin(
            &[
                "sg".into(),
                "--fixed".into(),
                ip.to_string(),
                "--policy".into(),
                "-".into(),
            ],
            Some(body),
        )
        .await
    }

    /// Send `ip` the rules of the groups it is in now, or clear it when it
    /// is in none.
    async fn sg_sync(&self, ip: IpAddr) -> ApiResult<()> {
        let policy = self.applied().policy_for(ip);
        match policy {
            Some(policy) => {
                let body = serde_json::to_vec(&policy)
                    .map_err(|e| ApiError::internal(format!("security group policy: {e}")))?;
                self.sg_policy(ip, &body).await
            }
            None => self.sg_clear(ip).await,
        }
    }

    async fn sg_clear(&self, ip: IpAddr) -> ApiResult<()> {
        self.run_stdin(
            &[
                "sg".into(),
                "--fixed".into(),
                ip.to_string(),
                "--clear".into(),
            ],
            None,
        )
        .await
    }
}

impl Default for HostNat {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NetBackend for HostNat {
    fn name(&self) -> &'static str {
        "hostnat"
    }

    async fn ensure_network(&self, _net: &Network) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_network(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn ensure_subnet(&self, net: &Network, subnet: &Subnet) -> ApiResult<()> {
        if net.spec.external {
            return Ok(());
        }
        let cidr = match subnet.spec.cidr {
            IpNet::V4(v4) => v4,
            IpNet::V6(_) => return Ok(()),
        };
        self.run(&[
            "guest-cidr".into(),
            "add".into(),
            "--cidr".into(),
            cidr.to_string(),
        ])
        .await
    }
    async fn delete_subnet(&self, _id: &Id) -> ApiResult<()> {
        // Leave the allow-list in place. Dropping a CIDR that still backs a
        // floating IP would black-hole DNAT, and 10.88.0.0/24 is never removed.
        Ok(())
    }
    async fn ensure_port(
        &self,
        _net: &Network,
        _port: &Port,
        _groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        Ok(PortBinding {
            vif_type: "hostnat".into(),
            vif_details: Default::default(),
        })
    }
    async fn delete_port(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn ensure_router(&self, _view: &RouterView) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_router(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn apply_security_group(&self, sg: &SecurityGroup, members: &[Port]) -> ApiResult<()> {
        let targets = member_v4(members);
        let policy = sg_policy(sg, members);
        let previous = self
            .applied()
            .by_group
            .insert(sg.meta.id.clone(), (targets.clone(), policy))
            .map(|(ips, _)| ips)
            .unwrap_or_default();
        for ip in &targets {
            self.sg_sync(*ip).await?;
        }
        // An address that left this group keeps what its other groups give.
        for ip in previous {
            if !targets.contains(&ip) {
                self.sg_sync(ip).await?;
            }
        }
        Ok(())
    }

    async fn delete_security_group(&self, id: &Id) -> ApiResult<()> {
        let previous = self.applied().by_group.remove(id);
        for ip in previous.map(|(ips, _)| ips).unwrap_or_default() {
            self.sg_sync(ip).await?;
        }
        Ok(())
    }

    async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
        let id = view.fip.meta.id.to_string();
        match &view.target {
            None => self.run(&[String::from("delete"), "--id".into(), id]).await,
            Some((_, fixed)) => {
                let floating = view.fip.spec.address.ok_or_else(|| {
                    ApiError::unprocessable(format!("floating ip {id} has no address"))
                })?;
                self.run(&[
                    "ensure".into(),
                    "--id".into(),
                    id,
                    "--floating".into(),
                    floating.to_string(),
                    "--fixed".into(),
                    fixed.to_string(),
                ])
                .await
            }
        }
    }

    async fn delete_fip(&self, id: &Id) -> ApiResult<()> {
        self.run(&[String::from("delete"), "--id".into(), id.to_string()])
            .await
    }
}

fn member_v4(members: &[Port]) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for port in members {
        for ip in &port.spec.fixed_ips {
            if ip.is_ipv4() && !out.contains(ip) {
                out.push(*ip);
            }
        }
    }
    out
}

#[derive(Clone, Debug, Default, Serialize)]
struct SgPolicy {
    ingress: Vec<SgMatch>,
    egress: Vec<SgMatch>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct SgMatch {
    proto: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    port_min: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port_max: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cidr: Option<String>,
}

fn sg_policy(sg: &SecurityGroup, members: &[Port]) -> SgPolicy {
    let mut policy = SgPolicy {
        ingress: Vec::new(),
        egress: Vec::new(),
    };
    for rule in &sg.spec.rules {
        let matches = express_rule(rule, &sg.meta.id, members);
        match rule.direction {
            Direction::Ingress => policy.ingress.extend(matches),
            Direction::Egress => policy.egress.extend(matches),
        }
    }
    policy
}

/// One JSON rule per expressible remote. IPv6, other groups, and port
/// ranges on anything but tcp/udp are omitted.
fn express_rule(rule: &Rule, self_id: &Id, members: &[Port]) -> Vec<SgMatch> {
    if rule.ethertype != EtherType::IPv4 {
        return Vec::new();
    }
    let proto = match rule.protocol {
        Some(Protocol::Tcp) => "tcp",
        Some(Protocol::Udp) => "udp",
        Some(Protocol::Icmp) => "icmp",
        Some(Protocol::Any) | None => "any",
    };
    let (port_min, port_max) = match rule.protocol {
        Some(Protocol::Tcp) | Some(Protocol::Udp) => match rule.port_range {
            Some(range) => (Some(range.min), Some(range.max)),
            None => (None, None),
        },
        _ => (None, None),
    };
    let cidrs: Vec<Option<String>> = match &rule.remote {
        Remote::Any => vec![None],
        Remote::Cidr(net) => match net {
            IpNet::V4(v4) => vec![Some(v4.to_string())],
            IpNet::V6(_) => Vec::new(),
        },
        Remote::Group(id) if id == self_id => members
            .iter()
            .flat_map(|port| port.spec.fixed_ips.iter())
            .filter_map(|ip| match ip {
                IpAddr::V4(v4) => Some(Some(format!("{v4}/32"))),
                IpAddr::V6(_) => None,
            })
            .collect(),
        Remote::Group(_) => Vec::new(),
    };
    cidrs
        .into_iter()
        .map(|cidr| SgMatch {
            proto,
            port_min,
            port_max,
            cidr,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::FipView;
    use super::*;
    use crate::types::{
        Direction, EtherType, FloatingIp, FloatingIpSpec, FloatingIpStatus, Network, NetworkSpec,
        NetworkStatus, Port, PortSpec, PortStatus, Protocol, Remote, Rule, SecurityGroup,
        SecurityGroupSpec, SecurityGroupStatus, Subnet, SubnetSpec, SubnetStatus,
    };
    use pistack_types::{kind, Id, Meta};
    use std::net::IpAddr;
    use std::os::unix::fs::PermissionsExt;

    fn fip(addr: &str, port: Option<Id>) -> FloatingIp {
        FloatingIp {
            meta: Meta::new(kind::FLOATING_IP, "demo", None),
            spec: FloatingIpSpec {
                network: Id::new(kind::NETWORK),
                address: Some(addr.parse().unwrap()),
                port,
            },
            status: FloatingIpStatus::default(),
        }
    }

    fn guest_port(addr: IpAddr) -> Port {
        let spec = PortSpec {
            fixed_ips: vec![addr],
            ..Default::default()
        };
        Port {
            meta: Meta::new(kind::PORT, "demo", None),
            spec,
            status: PortStatus::default(),
        }
    }

    fn helper(dir: &std::path::Path) -> (HostNat, std::path::PathBuf) {
        let log = dir.join("log");
        let prog = dir.join("fip");
        std::fs::write(
            &prog,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        (HostNat::command(&prog), log)
    }

    #[tokio::test]
    async fn associate_calls_ensure_and_detach_calls_delete() {
        let dir = std::env::temp_dir().join(format!("pistack-fip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (nat, log) = helper(&dir);
        let port = guest_port("10.88.0.187".parse().unwrap());
        let floating = fip("10.66.66.82", Some(port.meta.id.clone()));
        nat.associate_fip(&FipView {
            fip: floating.clone(),
            target: Some((port, "10.88.0.187".parse().unwrap())),
            router: None,
        })
        .await
        .unwrap();
        nat.associate_fip(&FipView {
            fip: floating.clone(),
            target: None,
            router: None,
        })
        .await
        .unwrap();
        nat.delete_fip(&floating.meta.id).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("ensure"));
        assert!(text.contains("--floating 10.66.66.82"));
        assert!(text.contains("--fixed 10.88.0.187"));
        assert_eq!(text.matches("delete").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sg_helper(dir: &std::path::Path) -> (HostNat, std::path::PathBuf) {
        let log = dir.join("log");
        let prog = dir.join("fip");
        let path = log.display().to_string();
        std::fs::write(
            &prog,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{path}'\ncat >> '{path}'\nprintf '\\n' >> '{path}'\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        (HostNat::command(&prog), log)
    }

    #[tokio::test]
    async fn security_group_calls_sg_with_cidr_on_stdin() {
        let dir = std::env::temp_dir().join(format!("pistack-fip-sg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (nat, log) = sg_helper(&dir);
        let with_ip = guest_port("10.88.0.187".parse().unwrap());
        let mut without_ip = guest_port("10.88.0.188".parse().unwrap());
        without_ip.spec.fixed_ips.clear();
        let mut sg = SecurityGroup {
            meta: Meta::new(kind::SECGROUP, "web", None),
            spec: SecurityGroupSpec::default(),
            status: SecurityGroupStatus::default(),
        };
        sg.spec.rules.push(
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22)
                .remote(Remote::Cidr("10.66.66.0/24".parse().unwrap())),
        );
        nat.apply_security_group(&sg, &[with_ip.clone(), without_ip])
            .await
            .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("sg"), "{text}");
        assert!(text.contains("--fixed"), "{text}");
        assert!(text.contains("--fixed 10.88.0.187"), "{text}");
        assert!(!text.contains("10.88.0.188"), "{text}");
        assert!(text.contains("10.66.66.0/24"), "{text}");
        assert!(text.contains("\"proto\":\"tcp\""), "{text}");
        nat.delete_security_group(&sg.meta.id).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("--clear"), "{text}");
        assert!(text.contains("--fixed 10.88.0.187"), "{text}");
        // A second delete has no remembered members and must still succeed.
        nat.delete_security_group(&sg.meta.id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn security_group_expands_own_members_and_skips_others() {
        let dir = std::env::temp_dir().join(format!("pistack-fip-sgm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (nat, log) = sg_helper(&dir);
        let a = guest_port("10.88.0.187".parse().unwrap());
        let mut b = guest_port("10.88.0.188".parse().unwrap());
        b.spec.fixed_ips.push("fe80::1".parse().unwrap());
        let mut bare = guest_port("10.88.0.189".parse().unwrap());
        bare.spec.fixed_ips.clear();
        let mut sg = SecurityGroup {
            meta: Meta::new(kind::SECGROUP, "web", None),
            spec: SecurityGroupSpec::default(),
            status: SecurityGroupStatus::default(),
        };
        let other = Id::new(kind::SECGROUP);
        sg.spec.rules.push(
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22)
                .remote(Remote::Group(sg.meta.id.clone())),
        );
        sg.spec.rules.push(
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(80, 80)
                .remote(Remote::Group(other)),
        );
        sg.spec.rules.push(
            Rule::new(Direction::Egress, EtherType::IPv6)
                .protocol(Protocol::Tcp)
                .ports(443, 443)
                .remote(Remote::Any),
        );
        nat.apply_security_group(&sg, &[a, b, bare]).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let calls = text.lines().filter(|l| l.starts_with("sg ")).count();
        assert_eq!(calls, 2, "{text}");
        assert!(text.contains("10.88.0.187/32"), "{text}");
        assert!(text.contains("10.88.0.188/32"), "{text}");
        assert!(!text.contains("10.88.0.189"), "{text}");
        assert!(!text.contains("fe80"), "{text}");
        assert!(!text.contains("\"port_min\":80"), "{text}");
        assert!(!text.contains("\"port_min\":443"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn group(name: &str, port: u16) -> SecurityGroup {
        let mut sg = SecurityGroup {
            meta: Meta::new(kind::SECGROUP, name, None),
            spec: SecurityGroupSpec::default(),
            status: SecurityGroupStatus::default(),
        };
        sg.spec.rules.push(
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(port, port)
                .remote(Remote::Any),
        );
        sg
    }

    /// The last thing the helper was told for `ip`: its arguments and, for
    /// a policy, the JSON it read.
    fn last_call(log: &std::path::Path, ip: &str) -> (String, String) {
        let text = std::fs::read_to_string(log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .rposition(|l| l.starts_with(&format!("sg --fixed {ip} ")))
            .unwrap_or_else(|| panic!("no sg call for {ip}: {text}"));
        let body = lines.get(at + 1).copied().unwrap_or_default();
        (lines[at].to_owned(), body.to_owned())
    }

    #[tokio::test]
    async fn port_in_two_groups_gets_the_rules_of_both() {
        let dir = std::env::temp_dir().join(format!("pistack-fip-sg2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (nat, log) = sg_helper(&dir);
        let both = guest_port("10.88.0.187".parse().unwrap());
        let web_only = guest_port("10.88.0.188".parse().unwrap());
        let ssh = group("ssh", 22);
        let mut web = group("web", 80);

        nat.apply_security_group(&ssh, std::slice::from_ref(&both))
            .await
            .unwrap();
        nat.apply_security_group(&web, &[both.clone(), web_only.clone()])
            .await
            .unwrap();
        let (_, body) = last_call(&log, "10.88.0.187");
        assert!(body.contains("\"port_min\":22"), "{body}");
        assert!(body.contains("\"port_min\":80"), "{body}");
        let (_, body) = last_call(&log, "10.88.0.188");
        assert!(!body.contains("\"port_min\":22"), "{body}");
        assert!(body.contains("\"port_min\":80"), "{body}");

        // The other group applied again, and a second pass, change nothing.
        nat.apply_security_group(&ssh, std::slice::from_ref(&both))
            .await
            .unwrap();
        let (_, again) = last_call(&log, "10.88.0.187");
        nat.apply_security_group(&web, &[both.clone(), web_only.clone()])
            .await
            .unwrap();
        assert_eq!(last_call(&log, "10.88.0.187").1, again);
        assert!(again.contains("\"port_min\":22"), "{again}");
        assert!(again.contains("\"port_min\":80"), "{again}");

        // A rule taken out of one group leaves the other group's rule.
        let rules = std::mem::take(&mut web.spec.rules);
        nat.apply_security_group(&web, &[both.clone(), web_only.clone()])
            .await
            .unwrap();
        let (_, body) = last_call(&log, "10.88.0.187");
        assert!(body.contains("\"port_min\":22"), "{body}");
        assert!(!body.contains("\"port_min\":80"), "{body}");

        // The port leaves ssh: only web's rules are left on it.
        web.spec.rules = rules;
        nat.apply_security_group(&web, &[both.clone(), web_only.clone()])
            .await
            .unwrap();
        nat.apply_security_group(&ssh, &[]).await.unwrap();
        let (args, body) = last_call(&log, "10.88.0.187");
        assert!(args.ends_with("--policy -"), "{args}");
        assert!(!body.contains("\"port_min\":22"), "{body}");
        assert!(body.contains("\"port_min\":80"), "{body}");

        // Deleting ssh does not touch it; deleting web clears it.
        let before = std::fs::read_to_string(&log).unwrap();
        nat.delete_security_group(&ssh.meta.id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
        nat.delete_security_group(&web.meta.id).await.unwrap();
        assert!(last_call(&log, "10.88.0.187").0.ends_with("--clear"));
        assert!(last_call(&log, "10.88.0.188").0.ends_with("--clear"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn guest_net(external: bool) -> Network {
        let spec = NetworkSpec {
            external,
            ..Default::default()
        };
        Network {
            meta: Meta::new(kind::NETWORK, "net", None),
            spec,
            status: NetworkStatus::default(),
        }
    }

    fn guest_subnet(cidr: &str) -> Subnet {
        Subnet {
            meta: Meta::new(kind::SUBNET, "sub", None),
            spec: SubnetSpec {
                network: Id::new(kind::NETWORK),
                cidr: cidr.parse().unwrap(),
                gateway: None,
                allocation_pools: Vec::new(),
                dns_nameservers: Vec::new(),
                enable_dhcp: true,
                host_routes: Vec::new(),
            },
            status: SubnetStatus::default(),
        }
    }

    #[tokio::test]
    async fn ensure_subnet_adds_guest_cidr_unless_external() {
        let dir = std::env::temp_dir().join(format!("pistack-fip-cidr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (nat, log) = helper(&dir);
        nat.ensure_subnet(&guest_net(false), &guest_subnet("10.90.0.0/24"))
            .await
            .unwrap();
        nat.ensure_subnet(&guest_net(true), &guest_subnet("10.66.66.80/28"))
            .await
            .unwrap();
        nat.ensure_subnet(&guest_net(false), &guest_subnet("fd00::/64"))
            .await
            .unwrap();
        nat.delete_subnet(&guest_subnet("10.90.0.0/24").meta.id)
            .await
            .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["guest-cidr add --cidr 10.90.0.0/24"], "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ensure_subnet_returns_helper_failure() {
        let dir =
            std::env::temp_dir().join(format!("pistack-fip-cidr-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prog = dir.join("fip");
        std::fs::write(&prog, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let nat = HostNat::command(&prog);
        let err = nat
            .ensure_subnet(&guest_net(false), &guest_subnet("10.90.0.0/24"))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("floating ip helper failed"),
            "{err}"
        );
        nat.ensure_subnet(&guest_net(true), &guest_subnet("10.66.66.80/28"))
            .await
            .unwrap();
        nat.ensure_subnet(&guest_net(false), &guest_subnet("fd00::/64"))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

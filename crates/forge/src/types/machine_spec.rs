// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::desired_power::DesiredPower;
use super::machine_kind::MachineKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MachineSpec {
    #[serde(default)]
    pub kind: MachineKind,
    /// Size name (or id).
    pub size: String,
    pub image: String,
    #[serde(default)]
    pub networks: Vec<String>,
    /// Security group names or ids. Empty uses the project's default group.
    #[serde(default)]
    pub security_groups: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Volumes this machine will have attached later. Nothing is attached
    /// at start. Hard constraint: a volume held on one node (a local file
    /// or LVM) keeps the machine on that node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub near_volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    /// Key pair name whose public key was added to `ssh_keys` at create.
    /// Resolved in the caller's project and stored as given; read-only after
    /// create. The machine carries the key itself, so deleting the pair
    /// changes nothing about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keypair: Option<String>,
    /// Soft hint: prefer hosts that already run machines with these names.
    #[serde(default)]
    pub affinity: Vec<String>,
    /// Soft hint: avoid hosts that run machines with these names.
    #[serde(default)]
    pub anti_affinity: Vec<String>,
    /// Hard constraint: every pair must match a host label.
    #[serde(default)]
    pub host_selector: BTreeMap<String, String>,
    /// Availability zone. Hard constraint: the host's `zone` label must
    /// equal it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(default)]
    pub power: DesiredPower,
    /// Monotonic counter bumped by `POST /machines/{id}/reboot`. The
    /// reconciler restarts the machine when it exceeds `status.restarts`.
    #[serde(default)]
    pub restarts: u64,
    /// Secret names this guest may read. Payloads stay in keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// Baseboard address for a chassis that is not this host.
    /// The agent does not power off the host it is running on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_address: Option<String>,
    /// IPMI username. The password is a file under `PISTACK_IPMI_DIR`,
    /// named by the machine id, and is not stored on the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_user: Option<String>,
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Every noun the CLI knows: where it lives on the API, how it prints, and
//! how `apply` updates it.

use super::output::{age, join, scalar, short_id, str_at};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub struct Column {
    pub header: &'static str,
    pub get: fn(&Value, DateTime<Utc>) -> String,
}

/// How `PUT /<path>/{id}` wants its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStyle {
    /// `{description, labels, spec}` — forge and fabric.
    Spec,
    /// `{description, labels}` only — gate, slab, mold.
    Meta,
    /// No update endpoint at all (bindings, hosts, snapshots).
    None,
}

pub struct KindDef {
    /// CLI noun: `machine`.
    pub noun: &'static str,
    /// Other spellings accepted in `apply` manifests.
    pub aliases: &'static [&'static str],
    /// Collection path: `/compute/machines`.
    pub path: &'static str,
    /// Id prefix: `mach`.
    pub id_prefix: &'static str,
    pub update: UpdateStyle,
    /// Create bodies: `true` puts spec fields beside `name` (gate, sizes);
    /// `false` nests them under `spec` (everything else).
    pub flat_create: bool,
    pub columns: &'static [Column],
    pub wide: &'static [Column],
}

impl KindDef {
    pub fn item_path(&self, name_or_id: &str) -> String {
        format!("{}/{}", self.path, name_or_id)
    }
    /// Does `s` already look like one of our ids?
    pub fn is_id(&self, s: &str) -> bool {
        s.strip_prefix(self.id_prefix)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|r| uuid::Uuid::parse_str(r).is_ok())
    }
}

fn name(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/name")
}
fn id(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&str_at(v, "/id"))
}
fn full_id(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/id")
}
fn project(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&str_at(v, "/project"))
}
fn labels(v: &Value, _: DateTime<Utc>) -> String {
    v.pointer("/labels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", scalar(v)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}
fn description(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/description")
}
fn created(v: &Value, now: DateTime<Utc>) -> String {
    age(&str_at(v, "/created_at"), now)
}
fn phase(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/status/phase")
}

macro_rules! col {
    ($h:literal, $f:ident) => {
        Column {
            header: $h,
            get: $f,
        }
    };
    ($h:literal, $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| str_at(v, $ptr),
        }
    };
    ($h:literal, short $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| short_id(&str_at(v, $ptr)),
        }
    };
    ($h:literal, list $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| {
                v.pointer($ptr)
                    .and_then(Value::as_array)
                    .map(|a| join(a))
                    .unwrap_or_default()
            },
        }
    };
    ($h:literal, count $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| {
                v.pointer($ptr)
                    .and_then(Value::as_array)
                    .map(|a| a.len().to_string())
                    .unwrap_or_else(|| "0".into())
            },
        }
    };
}

const WIDE_PROJECT: &[Column] = &[
    col!("FULL-ID", full_id),
    col!("PROJECT", project),
    col!("LABELS", labels),
];
const WIDE_GLOBAL: &[Column] = &[col!("FULL-ID", full_id), col!("LABELS", labels)];

pub static KINDS: &[KindDef] = &[
    KindDef {
        noun: "project",
        aliases: &["projects", "proj"],
        path: "/identity/projects",
        id_prefix: "proj",
        update: UpdateStyle::Meta,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("ENABLED", "/spec/enabled"),
            col!("PARENT", short "/spec/parent"),
            col!("DESCRIPTION", description),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "user",
        aliases: &["users", "principal", "principals"],
        path: "/identity/principals",
        id_prefix: "prin",
        update: UpdateStyle::Meta,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("KIND", "/spec/kind"),
            col!("EMAIL", "/spec/email"),
            col!("ENABLED", "/spec/enabled"),
            col!("DEFAULT-PROJECT", short "/spec/default_project"),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "role",
        aliases: &["roles"],
        path: "/identity/roles",
        id_prefix: "role",
        update: UpdateStyle::Meta,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("BUILTIN", "/spec/builtin"),
            col!("DESCRIPTION", description),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "binding",
        aliases: &["bindings", "rolebinding", "rolebindings"],
        path: "/identity/bindings",
        id_prefix: "rb",
        update: UpdateStyle::None,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PRINCIPAL", short "/spec/principal"),
            col!("ROLE", "/spec/role"),
            col!("SCOPE", "/spec/scope/type"),
            col!("PROJECT", short "/spec/scope/project"),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "key",
        aliases: &["keys", "apikey", "apikeys"],
        path: "/identity/keys",
        id_prefix: "key",
        update: UpdateStyle::None,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PRINCIPAL", short "/spec/principal"),
            col!("PROJECT", short "/spec/project"),
            col!("EXPIRES", "/spec/expires_at"),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "machine",
        aliases: &["machines", "mach", "vm", "server"],
        path: "/compute/machines",
        id_prefix: "mach",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("KIND", "/spec/kind"),
            col!("SIZE", "/spec/size"),
            col!("IMAGE", "/spec/image"),
            col!("POWER", "/status/power"),
            col!("ADDRESSES", list "/status/addresses"),
            col!("AGE", created),
        ],
        wide: &[
            col!("HOST", short "/status/host"),
            col!("NETWORKS", list "/spec/networks"),
            col!("MESSAGE", "/status/message"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "size",
        aliases: &["sizes", "flavor", "flavors"],
        path: "/compute/sizes",
        id_prefix: "size",
        update: UpdateStyle::Meta,
        flat_create: true,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("VCPUS", "/spec/vcpus"),
            col!("MEMORY-MIB", "/spec/memory_mib"),
            col!("DISK-GIB", "/spec/disk_gib"),
            col!("GPUS", "/spec/gpus"),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "host",
        aliases: &["hosts"],
        path: "/compute/hosts",
        id_prefix: "host",
        update: UpdateStyle::None,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("NODE", "/spec/node_name"),
            col!("VCPUS", "/spec/capacity/vcpus"),
            col!("FREE-VCPUS", "/status/free/vcpus"),
            col!("MEMORY-MIB", "/spec/capacity/memory_mib"),
            col!("FREE-MIB", "/status/free/memory_mib"),
            col!("DRIVERS", list "/spec/drivers"),
            col!("LAST-SEEN", "/status/last_seen"),
        ],
        wide: &[
            col!("MESSAGE", "/status/message"),
            col!("FULL-ID", full_id),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "network",
        aliases: &["networks", "net"],
        path: "/network/networks",
        id_prefix: "net",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("TYPE", "/spec/kind/type"),
            col!("VLAN", "/spec/kind/vlan"),
            col!("MTU", "/spec/mtu"),
            col!("EXTERNAL", "/spec/external"),
            col!("SEGMENT", "/status/segment_id"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "subnet",
        aliases: &["subnets"],
        path: "/network/subnets",
        id_prefix: "subnet",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("NETWORK", short "/spec/network"),
            col!("CIDR", "/spec/cidr"),
            col!("GATEWAY", "/spec/gateway"),
            col!("DHCP", "/spec/enable_dhcp"),
            col!("ALLOCATED", "/status/allocated"),
            col!("AGE", created),
        ],
        wide: &[
            col!("DNS", list "/spec/dns_nameservers"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "port",
        aliases: &["ports"],
        path: "/network/ports",
        id_prefix: "port",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("NETWORK", short "/spec/network"),
            col!("IPS", list "/spec/fixed_ips"),
            col!("MAC", "/spec/mac"),
            col!("DEVICE", short "/spec/device"),
            col!("AGE", created),
        ],
        wide: &[
            col!("HOST", "/status/host"),
            col!("SECGROUPS", count "/spec/security_groups"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "router",
        aliases: &["routers", "rtr"],
        path: "/network/routers",
        id_prefix: "rtr",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("EXTERNAL-NET", short "/spec/external_network"),
            col!("EXTERNAL-IP", "/status/external_ip"),
            col!("INTERFACES", count "/spec/interfaces"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "secgroup",
        aliases: &["secgroups", "securitygroup", "securitygroups", "sg"],
        path: "/network/security-groups",
        id_prefix: "sg",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("RULES", count "/spec/rules"),
            col!("MEMBERS", "/status/members"),
            col!("DESCRIPTION", description),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "fip",
        aliases: &["fips", "floatingip", "floatingips", "floating-ip"],
        path: "/network/floating-ips",
        id_prefix: "fip",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("ADDRESS", "/spec/address"),
            col!("NETWORK", short "/spec/network"),
            col!("PORT", short "/spec/port"),
            col!("FIXED-IP", "/status/fixed_ip"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "volume",
        aliases: &["volumes", "vol"],
        path: "/volume/volumes",
        id_prefix: "vol",
        update: UpdateStyle::Meta,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("SIZE-GIB", "/spec/size_gib"),
            col!("TYPE", "/spec/volume_type"),
            col!("BOOTABLE", "/spec/bootable"),
            col!("ATTACHED-TO", short "/status/attached_to"),
            col!("DEVICE", "/status/device"),
            col!("AGE", created),
        ],
        wide: &[
            col!("SOURCE", "/spec/source/kind"),
            col!("BACKEND-REF", "/status/backend_ref"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "snapshot",
        aliases: &["snapshots", "snap"],
        path: "/volume/snapshots",
        id_prefix: "snap",
        update: UpdateStyle::None,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("VOLUME", short "/spec/volume"),
            col!("SIZE-GIB", "/status/size_gib"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "voltype",
        aliases: &["voltypes", "volumetype", "volumetypes", "volume-type"],
        path: "/volume/volume-types",
        id_prefix: "voltype",
        update: UpdateStyle::Meta,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("BACKEND", "/spec/backend"),
            col!("DESCRIPTION", description),
            col!("AGE", created),
        ],
        wide: WIDE_GLOBAL,
    },
    KindDef {
        noun: "image",
        aliases: &["images", "img"],
        path: "/image/images",
        id_prefix: "img",
        update: UpdateStyle::Meta,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("FORMAT", "/spec/format"),
            col!("OS", "/spec/os"),
            col!("ARCH", "/spec/arch"),
            col!("VISIBILITY", "/spec/visibility"),
            col!("SIZE", "/status/size_bytes"),
            col!("AGE", created),
        ],
        wide: &[
            col!("SHA256", "/status/sha256"),
            col!("MIN-DISK", "/spec/min_disk_gib"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "statefulset",
        aliases: &["statefulsets", "sts"],
        path: "/loom/statefulsets",
        id_prefix: "sts",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("REPLICAS", "/spec/replicas"),
            col!("READY", "/status/ready"),
            col!("CURRENT", "/status/current"),
            col!("UPDATED", "/status/updated"),
            col!("SERVICE", "/spec/service_name"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "daemonset",
        aliases: &["daemonsets", "ds"],
        path: "/loom/daemonsets",
        id_prefix: "ds",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("DESIRED", "/status/desired"),
            col!("CURRENT", "/status/current"),
            col!("READY", "/status/ready"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
    KindDef {
        noun: "autoscaler",
        aliases: &["autoscalers", "scaler", "hpa"],
        path: "/loom/autoscalers",
        id_prefix: "scaler",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("TARGET-KIND", "/spec/target_kind"),
            col!("TARGET", "/spec/target_name"),
            col!("METRIC", "/spec/metric"),
            col!("MIN", "/spec/min_replicas"),
            col!("MAX", "/spec/max_replicas"),
            col!("DESIRED", "/status/desired_replicas"),
            col!("AGE", created),
        ],
        wide: &[
            col!("TARGET-VALUE", "/spec/target_value"),
            col!("CURRENT", "/status/current_replicas"),
            col!("LAST-METRIC", "/status/last_metric"),
            col!("MESSAGE", "/status/message"),
            col!("FULL-ID", full_id),
            col!("PROJECT", project),
            col!("LABELS", labels),
        ],
    },
    KindDef {
        noun: "stack",
        aliases: &["stacks"],
        path: "/blueprint/stacks",
        id_prefix: "stack",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: &[
            col!("NAME", name),
            col!("ID", id),
            col!("PHASE", phase),
            col!("REVISION", "/status/revision"),
            col!("MESSAGE", "/status/message"),
            col!("AGE", created),
        ],
        wide: WIDE_PROJECT,
    },
];

pub fn by_noun(noun: &str) -> Option<&'static KindDef> {
    let n = noun.trim().to_lowercase().replace(['_', ' '], "-");
    let compact = n.replace('-', "");
    KINDS.iter().find(|k| {
        k.noun == n
            || k.noun.replace('-', "") == compact
            || k.aliases
                .iter()
                .any(|a| *a == n || a.replace('-', "") == compact)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nouns_resolve_with_aliases() {
        assert_eq!(by_noun("machine").unwrap().path, "/compute/machines");
        assert_eq!(by_noun("Machine").unwrap().noun, "machine");
        assert_eq!(by_noun("SecurityGroup").unwrap().noun, "secgroup");
        assert_eq!(by_noun("floating_ip").unwrap().noun, "fip");
        assert_eq!(by_noun("VolumeType").unwrap().noun, "voltype");
        assert_eq!(by_noun("principal").unwrap().noun, "user");
        assert_eq!(by_noun("sts").unwrap().path, "/loom/statefulsets");
        assert_eq!(by_noun("DaemonSet").unwrap().noun, "daemonset");
        assert_eq!(by_noun("hpa").unwrap().noun, "autoscaler");
        assert_eq!(by_noun("stack").unwrap().path, "/blueprint/stacks");
        assert!(by_noun("bogus").is_none());
    }

    #[test]
    fn id_detection() {
        let k = by_noun("machine").unwrap();
        assert!(k.is_id("mach-0199a5e0-1111-7000-8000-000000000001"));
        assert!(!k.is_id("web"));
        assert!(!k.is_id("vol-0199a5e0-1111-7000-8000-000000000001"));
    }

    #[test]
    fn every_kind_has_name_id_age_or_last_seen() {
        for k in KINDS {
            assert_eq!(k.columns[0].header, "NAME", "{}", k.noun);
            assert_eq!(k.columns[1].header, "ID", "{}", k.noun);
            let last = k.columns.last().unwrap().header;
            assert!(last == "AGE" || last == "LAST-SEEN", "{}: {last}", k.noun);
        }
    }
}

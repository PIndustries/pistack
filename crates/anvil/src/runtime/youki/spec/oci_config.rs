// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::bind_mount;
use crate::runtime::youki::spec::creates_user_namespace;
use crate::runtime::youki::spec::default_mounts;
use crate::runtime::youki::spec::id_map;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::volumes::OciBind;

impl RunMode {
    pub(in crate::runtime::youki) fn oci_config(
        &self,
        hostname: Option<&str>,
        limits: &Limits,
        process: (&[String], &[String], &str),
        namespaces: &[serde_json::Value],
        binds: &[OciBind],
    ) -> serde_json::Value {
        let (args, env, cwd) = process;
        let mut linux = serde_json::json!({ "namespaces": namespaces });
        if let Privilege::Rootless { uid, gid } = &self.privilege {
            if creates_user_namespace(namespaces) {
                linux["uidMappings"] = id_map(*uid);
                linux["gidMappings"] = id_map(*gid);
            }
        }
        let mut resources = serde_json::Map::new();
        if self.cgroup.memory && limits.memory_mib > 0 {
            let bytes = limits.memory_mib.saturating_mul(1024 * 1024);
            resources.insert("memory".into(), serde_json::json!({ "limit": bytes }));
        }
        if self.cgroup.cpu && limits.vcpus > 0 {
            let period: u64 = 100_000;
            let quota = period.saturating_mul(u64::from(limits.vcpus));
            resources.insert(
                "cpu".into(),
                serde_json::json!({ "period": period, "quota": quota }),
            );
        }
        if !resources.is_empty() {
            linux["resources"] = serde_json::Value::Object(resources);
        }
        let mut mounts = default_mounts();
        for bind in binds {
            mounts.push(bind_mount(bind));
        }
        let mut spec = serde_json::json!({
            "ociVersion": "1.0.2",
            "process": {
                "terminal": false,
                "user": {"uid": 0, "gid": 0},
                "args": args,
                "env": env,
                "cwd": cwd
            },
            "root": {"path": "rootfs", "readonly": false},
            "mounts": mounts,
            "linux": linux
        });
        if let Some(host) = hostname {
            spec["hostname"] = serde_json::Value::String(host.to_owned());
        }
        spec
    }
}

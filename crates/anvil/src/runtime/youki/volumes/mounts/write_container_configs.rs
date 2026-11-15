// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::PreparedProcess;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::container_namespaces;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::volumes::container_dir;
use crate::runtime::youki::volumes::materialize_volumes;
use crate::runtime::youki::volumes::pod_binds;
use crate::runtime::youki::volumes::write_json;
use crate::runtime::youki::volumes::AttachedDisk;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_container_configs(
    bundle: &Path,
    plan: &PodPlan,
    processes: &[PreparedProcess],
    disks: &[AttachedDisk],
    limits: &Limits,
    pid: u32,
    mode: &RunMode,
) -> ApiResult<()> {
    // Projected dirs are rewritten; EmptyDir and container rootfs are not.
    materialize_volumes(bundle, plan, &mode.privilege)?;
    let rootless = matches!(&mode.privilege, Privilege::Rootless { .. });
    let namespaces = container_namespaces(pid, rootless);
    for (index, process) in processes.iter().enumerate() {
        let mounts = plan
            .containers
            .get(index)
            .map(|container| container.volume_mounts.as_slice())
            .unwrap_or(&[]);
        let binds = pod_binds(bundle, plan, mounts, disks)?;
        let spec = mode.oci_config(
            None,
            limits,
            (&process.args, &process.env, &process.cwd),
            &namespaces,
            &binds,
        );
        write_json(
            &container_dir(bundle, index).join("config.json"),
            &spec,
            0o600,
        )?;
    }
    Ok(())
}

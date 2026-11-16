// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::volumes::ensure_slab_sources;
use crate::runtime::youki::volumes::prepare_pod;
use crate::runtime::youki::volumes::read_disks;
use crate::runtime::youki::volumes::write_container_configs;
use pistack_types::ApiResult;
use std::path::Path;

#[cfg(test)]
impl RunMode {
    pub(in crate::runtime::youki) fn stage_pod_bundles(
        &self,
        bundle: &Path,
        hostname: &str,
        limits: &Limits,
        plan: &PodPlan,
        fallback_image: &str,
        pid: u32,
    ) -> ApiResult<()> {
        let disks = read_disks(bundle)?;
        ensure_slab_sources(plan, &disks)?;
        let processes = prepare_pod(bundle, hostname, limits, plan, fallback_image, self, None)?;
        write_container_configs(bundle, plan, &processes, &disks, limits, pid, self)
    }
}

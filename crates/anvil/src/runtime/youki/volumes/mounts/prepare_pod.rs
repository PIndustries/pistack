// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::privatize_bundle;
use crate::runtime::youki::env::cannot_unpack_container;
use crate::runtime::youki::env::choose_container_image;
use crate::runtime::youki::env::resolve_env_pairs;
use crate::runtime::youki::env::PreparedProcess;
use crate::runtime::youki::image::unpack_image_with;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::process_spec;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::spec::rootfs_empty;
use crate::runtime::youki::spec::Launch;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::volumes::container_dir;
use crate::runtime::youki::volumes::materialize_volumes;
use crate::runtime::youki::volumes::volume_io;
use crate::runtime::youki::volumes::write_mount_plan;
use crate::runtime::youki::volumes::write_pause_rootfs;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn prepare_pod(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    plan: &PodPlan,
    fallback_image: &str,
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> ApiResult<Vec<PreparedProcess>> {
    std::fs::create_dir_all(bundle).map_err(|e| volume_io(bundle, e))?;
    privatize_bundle(bundle, &mode.privilege)?;
    materialize_volumes(bundle, plan, &mode.privilege)?;
    write_mount_plan(bundle, plan)?;
    write_pause_rootfs(bundle, hostname, limits, mode, host_netns)?;
    let mut processes = Vec::new();
    for (index, container) in plan.containers.iter().enumerate() {
        let image = choose_container_image(index, &container.image, fallback_image)
            .map_err(|_| cannot_unpack_container(container, index))?;
        let dir = container_dir(bundle, index);
        std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
        let rootfs = dir.join("rootfs");
        reset_dir(&rootfs, &mode.privilege)?;
        let cfg = unpack_image_with(&image, &rootfs, &mode.privilege)?;
        if rootfs_empty(&rootfs) {
            return Err(ApiError::unavailable(
                "youki runtime: image rootfs is empty",
            ));
        }
        let launch = Launch {
            image: container.image.clone(),
            command: container.command.clone(),
            args: container.args.clone(),
            env: resolve_env_pairs(&container.env, &plan.config_maps, &plan.secrets),
            cwd: container.working_dir.clone(),
        };
        let (args, env, cwd) = process_spec(Some(&launch), &cfg);
        processes.push(PreparedProcess { args, env, cwd });
    }
    privatize_bundle(bundle, &mode.privilege)?;
    Ok(processes)
}

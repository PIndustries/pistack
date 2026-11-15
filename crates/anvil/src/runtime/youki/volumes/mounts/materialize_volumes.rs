// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// ConfigMap and Secret directories are replaced. EmptyDir is created once
/// and kept. Slab volumes are not files on the pod directory.
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::VolumeSrc;
use crate::runtime::youki::volumes::projected_modes;
use crate::runtime::youki::volumes::set_mode;
use crate::runtime::youki::volumes::volume_host_dir;
use crate::runtime::youki::volumes::volume_io;
use crate::runtime::youki::volumes::write_projected;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn materialize_volumes(
    pod: &Path,
    plan: &PodPlan,
    privilege: &Privilege,
) -> ApiResult<()> {
    for (index, vol) in plan.volumes.iter().enumerate() {
        match &vol.source {
            VolumeSrc::EmptyDir => {
                let dir = volume_host_dir(pod, index, &vol.name);
                std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
            }
            VolumeSrc::ConfigMap(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                let (file_mode, dir_mode) = projected_modes(privilege, false);
                match plan.config_maps.get(name) {
                    Some(files) => write_projected(&dir, files, file_mode, dir_mode, privilege)?,
                    None => {
                        reset_dir(&dir, privilege)?;
                        if matches!(privilege, Privilege::Sudo { .. }) {
                            set_mode(&dir, dir_mode)?;
                        }
                    }
                }
            }
            VolumeSrc::Secret(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                let (file_mode, dir_mode) = projected_modes(privilege, true);
                match plan.secrets.get(name) {
                    Some(files) => write_projected(&dir, files, file_mode, dir_mode, privilege)?,
                    None => {
                        reset_dir(&dir, privilege)?;
                        set_mode(&dir, dir_mode)?;
                    }
                }
            }
            VolumeSrc::Volume(_) => {}
        }
    }
    Ok(())
}

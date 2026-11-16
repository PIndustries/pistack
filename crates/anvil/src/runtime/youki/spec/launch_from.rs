// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::resolve_env_pairs;
use crate::runtime::youki::spec::sandbox_plan;
use crate::runtime::youki::spec::Launch;
/// Pod sandbox JSON is the only `user_data` this runtime reads. Anything
/// else, including a raw secret, is ignored and never written down.
use crate::runtime::MachineRequest;

pub(in crate::runtime::youki) fn launch_from(req: &MachineRequest) -> Option<Launch> {
    let plan = sandbox_plan(req).ok()??;
    let env = plan
        .containers
        .first()
        .map(|c| resolve_env_pairs(&c.env, &plan.config_maps, &plan.secrets))
        .unwrap_or_default();
    let c = plan.containers.into_iter().next().unwrap_or_default();
    Some(Launch {
        image: c.image,
        command: c.command,
        args: c.args,
        env,
        cwd: c.working_dir,
    })
}

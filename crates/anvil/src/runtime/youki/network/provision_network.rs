// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;
use crate::runtime::podnet::PodNetwork;
use crate::runtime::youki::network::ip_privilege;
use crate::runtime::youki::network::requested_host_netns;
use crate::runtime::youki::privilege::RunMode;
/// Create the pod namespace unless `PISTACK_YOUKI_NETNS` already names one.
use crate::runtime::MachineRequest;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn provision_network(
    bundle: &Path,
    req: &MachineRequest,
    mode: &RunMode,
) -> ApiResult<PodNetwork> {
    if requested_host_netns().is_some() {
        return Ok(PodNetwork::None);
    }
    podnet::prepare(req, &ip_privilege(mode), bundle).await
}

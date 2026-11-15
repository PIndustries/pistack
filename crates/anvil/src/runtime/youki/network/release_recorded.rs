// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Remove a recorded namespace. Empty names do not run `ip`.
use crate::runtime::podnet;
use crate::runtime::youki::network::ip_privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::runtime::Saved;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn release_recorded(
    mode: &RunMode,
    saved: &Saved,
    bundle: &Path,
) -> ApiResult<()> {
    let recorded = if !saved.netns.is_empty() || !saved.veths.is_empty() || !saved.ovs.is_empty() {
        Some(podnet::RecordedNet {
            netns: saved.netns.clone(),
            veths: saved.veths.clone(),
            ovs: saved.ovs.clone(),
        })
    } else {
        podnet::read_record(bundle)
    };
    let Some(recorded) = recorded else {
        return Ok(());
    };
    podnet::release(
        &ip_privilege(mode),
        &recorded.netns,
        &recorded.veths,
        &recorded.ovs,
    )
    .await
}

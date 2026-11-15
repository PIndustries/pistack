// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `youki state` shows a container this agent has no `state.json` record of
/// as running. Remove it so `create` does not fail with "already exists".
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::lifecycle::container_missing;
use crate::runtime::youki::lifecycle::run_youki;
use crate::runtime::youki::privilege::RunMode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn clear_leftover(
    bin: &Path,
    mode: &RunMode,
    id: &str,
) -> ApiResult<()> {
    match run_youki(bin, mode, &["state", id], None).await {
        Ok(_) => exec_youki(bin, mode, &["delete", "--force", id], None).await,
        Err(e) if container_missing(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

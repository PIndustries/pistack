// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::path_arg;
use crate::runtime::youki::bundles::remove_bundle;
use crate::runtime::youki::exec::exec_detach;
use crate::runtime::youki::lifecycle::clear_leftover;
use crate::runtime::youki::lifecycle::forget_failed_create;
use crate::runtime::youki::lifecycle::DetachIo;
use crate::runtime::youki::privilege::RunMode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn create_container(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    id: &str,
    join_user_ns: Option<&Path>,
    io: &DetachIo,
    drop_bundle: Option<&Path>,
) -> ApiResult<()> {
    if let Err(e) = clear_leftover(bin, mode, id).await {
        if let Some(root) = drop_bundle {
            let _ = remove_bundle(root, &mode.privilege).await;
        }
        return Err(e);
    }
    let bundle_arg = path_arg(bundle);
    if let Err(e) = exec_detach(
        bin,
        mode,
        &["create", "--bundle", &bundle_arg, id],
        join_user_ns,
        io,
    )
    .await
    {
        forget_failed_create(bin, mode, id, drop_bundle).await;
        return Err(e);
    }
    Ok(())
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::delete_bundle;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn remove_bundle(
    bundle: &Path,
    privilege: &Privilege,
) -> ApiResult<()> {
    let bundle = bundle.to_path_buf();
    let privilege = privilege.clone();
    match tokio::task::spawn_blocking(move || delete_bundle(&bundle, &privilege)).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::unavailable(
            "youki runtime: bundle removal stopped",
        )),
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::runtime::Saved;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn write_state(bundle: &Path, saved: &Saved) -> ApiResult<()> {
    let path = bundle.join("state.json");
    let text = serde_json::to_vec_pretty(saved)
        .map_err(|e| ApiError::internal(format!("youki runtime: encoding state: {e}")))?;
    tokio::fs::write(&path, text).await.map_err(|e| {
        ApiError::unavailable(format!("youki runtime: writing {}: {e}", path.display()))
    })
}

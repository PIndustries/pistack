// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// No host reads `spec.volumes`: a machine that named volumes started
/// without them. Until a start can attach them, say so instead of taking
/// the request. Attaching to a running machine goes through the volume API.
pub(crate) fn refuse_volumes(volumes: &[String]) -> ApiResult<()> {
    if volumes.is_empty() {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "spec.volumes is not supported: a machine does not attach volumes when it starts. \
         Create the machine without them (`--near-volume` places it where a \
         volume can attach), then attach each one with \
         `pi volume attach <volume> --machine <machine>`",
    ))
}

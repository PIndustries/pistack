// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::path::Path;

pub(super) fn state_dir_error(dir: &Path, err: &std::io::Error) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: cannot write state directory {}: {err}. Set PISTACK_YOUKI_STATE or PISTACK_DATA_DIR to a directory the agent user owns",
        dir.display()
    ))
}

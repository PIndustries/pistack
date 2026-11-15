// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxContainer;
use pistack_types::ApiError;

pub(in crate::runtime::youki) fn cannot_unpack_container(
    container: &SandboxContainer,
    index: usize,
) -> ApiError {
    let who = if container.name.is_empty() {
        format!("container {index}")
    } else {
        format!("container {}", container.name)
    };
    ApiError::unavailable(format!("youki runtime: image for {who} cannot be unpacked"))
}

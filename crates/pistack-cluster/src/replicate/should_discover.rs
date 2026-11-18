// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub fn should_discover(existing_store: bool, init: bool, join: Option<&str>) -> bool {
    !existing_store && !init && join.map(str::trim).filter(|url| !url.is_empty()).is_none()
}

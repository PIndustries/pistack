// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::HeaderName;

pub(crate) const VERSION_ID: HeaderName = HeaderName::from_static("x-amz-version-id");

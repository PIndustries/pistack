// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::HeaderName;

pub(crate) const DELETE_MARKER: HeaderName = HeaderName::from_static("x-amz-delete-marker");

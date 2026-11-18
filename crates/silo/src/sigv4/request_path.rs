// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::extract::{OriginalUri, Request};

pub(crate) fn request_path(req: &Request) -> &str {
    if let Some(OriginalUri(uri)) = req.extensions().get::<OriginalUri>() {
        return uri.path();
    }
    req.uri().path()
}

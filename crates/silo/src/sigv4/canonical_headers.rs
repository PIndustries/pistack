// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::collapse_ws::collapse_ws;

pub(crate) fn canonical_headers(headers: &HeaderMap, signed: &[String]) -> ApiResult<String> {
    let mut out = String::new();
    for name in signed {
        let values: Vec<&str> = headers
            .get_all(name.as_str())
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        if values.is_empty() {
            // Host may come from URI authority when missing; require it signed.
            if name == "host" {
                return Err(s3_err("InvalidRequest", "signed header 'host' is missing"));
            }
            return Err(s3_err(
                "InvalidRequest",
                format!("signed header '{name}' is missing"),
            ));
        }
        let joined = values
            .iter()
            .map(|v| collapse_ws(v))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(name);
        out.push(':');
        out.push_str(&joined);
        out.push('\n');
    }
    Ok(out)
}

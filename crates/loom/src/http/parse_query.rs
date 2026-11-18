// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, ListQuery};

use crate::http::percent_decode::percent_decode;

pub(crate) fn parse_query(q: &str) -> ApiResult<ListQuery> {
    let mut out = ListQuery::default();
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "limit" => {
                out.limit = Some(
                    v.parse()
                        .map_err(|_| ApiError::bad_request("limit must be a number"))?,
                )
            }
            "after" => out.after = Some(v),
            "labels" => out.labels = Some(v),
            "name" => out.name = Some(v),
            _ => {}
        }
    }
    Ok(out)
}

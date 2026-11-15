// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, uuid};
use super::super::ovn_backend::OvnBackend;
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

impl OvnBackend {
    /// Make the one row matching `cond` look like `row`: insert it (plus
    /// `extra`, which may refer to it as `new`) when missing, update it
    /// when it differs, leave it alone when it matches. Returns its uuid.
    pub(in crate::backend::ovn) async fn ensure_row(
        &self,
        table: &str,
        cond: Vec<Value>,
        row: Value,
        extra: Vec<Value>,
    ) -> ApiResult<String> {
        let found = self.run(vec![ovsdb::select(table, cond.clone())]).await?;
        let found = ovsdb::rows(&found[0]);
        match found.as_slice() {
            [] => {
                let mut ops = vec![ovsdb::absent(table, cond), ovsdb::insert(table, row, "new")];
                ops.extend(extra);
                let out = self.run(ops).await?;
                out[1]["uuid"][1]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| ApiError::unavailable(format!("{table} insert: no uuid")))
            }
            [one] => {
                let u = ovsdb::row_uuid(one)
                    .ok_or_else(|| ApiError::unavailable(format!("{table} row: no uuid")))?;
                if !ovsdb::row_matches(one, &row) {
                    self.run(vec![ovsdb::update(table, vec![eq("_uuid", uuid(&u))], row)])
                        .await?;
                }
                Ok(u)
            }
            many => Err(ApiError::conflict(format!(
                "{} {table} rows match {}",
                many.len(),
                Value::Array(cond)
            ))),
        }
    }
}

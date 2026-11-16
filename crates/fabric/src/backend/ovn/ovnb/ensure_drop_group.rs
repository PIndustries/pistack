// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::{consts::DROP_GROUP, drop_acls::drop_acls, ovn_backend::OvnBackend};
use pistack_types::ApiResult;

impl OvnBackend {
    pub(in crate::backend::ovn) async fn ensure_drop_group(&self) -> ApiResult<()> {
        let ext = vec![
            ("pistack:id".to_string(), DROP_GROUP.to_string()),
            ("pistack:term".to_string(), self.term().to_string()),
        ];
        self.ensure_group(DROP_GROUP, ext, Some(&drop_acls()), None)
            .await
    }
}

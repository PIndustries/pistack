// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::consts::MAX_CONTENT_TYPE;
use crate::error::KeepError;
use crate::error::Result;
use crate::types::Secret;

use super::check_meta_fields;

pub(in crate::client) fn check_metadata(secret: &Secret) -> Result<()> {
    check_meta_fields(&secret.meta.labels, &secret.meta.description)?;
    if secret.spec.content_type.len() > MAX_CONTENT_TYPE {
        return Err(KeepError::Invalid(format!(
            "content_type exceeds {MAX_CONTENT_TYPE} bytes"
        )));
    }
    Ok(())
}

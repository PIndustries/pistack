// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use crate::client::consts::{MAX_DESCRIPTION, MAX_LABELS, MAX_LABEL_KEY, MAX_LABEL_VALUE};
use crate::error::{KeepError, Result};

/// Label and description limits shared by secrets and certificates.
pub(crate) fn check_meta_fields(
    labels: &BTreeMap<String, String>,
    description: &str,
) -> Result<()> {
    if labels.len() > MAX_LABELS {
        return Err(KeepError::Invalid(format!(
            "at most {MAX_LABELS} labels are allowed"
        )));
    }
    if let Some((k, _)) = labels
        .iter()
        .find(|(k, v)| k.is_empty() || k.len() > MAX_LABEL_KEY || v.len() > MAX_LABEL_VALUE)
    {
        return Err(KeepError::Invalid(format!(
            "label '{}' must have a 1-{MAX_LABEL_KEY} byte key and a value of at most \
         {MAX_LABEL_VALUE} bytes",
            k.chars().take(32).collect::<String>()
        )));
    }
    if description.len() > MAX_DESCRIPTION {
        return Err(KeepError::Invalid(format!(
            "description exceeds {MAX_DESCRIPTION} bytes"
        )));
    }
    Ok(())
}

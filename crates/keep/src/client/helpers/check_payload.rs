// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::consts::MAX_PAYLOAD;
use crate::error::{KeepError, Result};

pub(in crate::client) fn check_payload(payload: &[u8]) -> Result<()> {
    if payload.is_empty() {
        return Err(KeepError::Invalid("payload must not be empty".into()));
    }
    if payload.len() > MAX_PAYLOAD {
        return Err(KeepError::Invalid(format!(
            "payload exceeds {MAX_PAYLOAD} bytes"
        )));
    }
    Ok(())
}

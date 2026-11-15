// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::STANDARD as B64, Engine};

use crate::crypto::RootKey;
use crate::error::{KeepError, Result};

pub(in crate::backend) fn decode_software(b64: &str) -> Result<RootKey> {
    let raw = B64
        .decode(b64.trim())
        .map_err(|e| KeepError::Crypto(format!("root key is not base64: {e}")))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| KeepError::Crypto("root key must be 32 bytes".into()))?;
    Ok(RootKey::from_bytes(bytes))
}

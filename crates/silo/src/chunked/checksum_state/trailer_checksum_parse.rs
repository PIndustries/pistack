// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use crate::multipart::s3_err;
use pistack_types::ApiResult;

impl TrailerChecksum {
    pub fn parse(header_name: &str) -> ApiResult<Self> {
        match header_name.trim().to_ascii_lowercase().as_str() {
            "x-amz-checksum-crc32" => Ok(Self::Crc32),
            "x-amz-checksum-crc32c" => Ok(Self::Crc32c),
            "x-amz-checksum-sha256" => Ok(Self::Sha256),
            "x-amz-checksum-sha1" => Ok(Self::Sha1),
            other => Err(s3_err(
                "InvalidRequest",
                format!("unsupported x-amz-trailer checksum '{other}'"),
            )),
        }
    }
}

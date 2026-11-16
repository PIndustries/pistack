// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::chunk_signature::chunk_signature;
use crate::chunked::checksum_state::ct_eq_hex::ct_eq_hex;
use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::multipart::s3_err;
use pistack_types::ApiResult;

impl DecodedStream {
    pub(in crate::chunked) fn verify_chunk_sig(
        &mut self,
        declared: Option<&str>,
        data: &[u8],
    ) -> ApiResult<()> {
        if !self.auth.requires_chunk_signatures() {
            return Ok(());
        }
        let Some(declared) = declared else {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked chunk missing chunk-signature",
            ));
        };
        let expected = chunk_signature(
            &self.auth.signing_key,
            &self.auth.amz_date,
            &self.auth.scope,
            &self.prev_sig,
            data,
        );
        if !ct_eq_hex(&expected, &declared.to_ascii_lowercase()) {
            return Err(s3_err(
                "SignatureDoesNotMatch",
                "The request signature we calculated does not match the signature you provided.",
            ));
        }
        self.prev_sig = expected;
        Ok(())
    }
}

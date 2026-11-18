// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! AWS Signature Version 4 verification for silo's path-style S3 routes.
//!
//! Supports:
//! * Header auth (`Authorization: AWS4-HMAC-SHA256 …` plus `x-amz-date` /
//!   `x-amz-content-sha256`)
//! * Presigned URLs (query `X-Amz-*`) for any method, including GET/PUT
//! * `UNSIGNED-PAYLOAD` and a declared hex SHA-256 payload hash
//! * aws-chunked streaming bodies via [`crate::chunked`]:
//!   `STREAMING-AWS4-HMAC-SHA256-PAYLOAD`,
//!   `STREAMING-UNSIGNED-PAYLOAD-TRAILER`, and
//!   `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`
//!
//! ECDSA / SigV4A streaming variants return `NotImplemented`.
//!
//! When `x-amz-copy-source` is present (UploadPartCopy / CopyObject), it and
//! any present `x-amz-copy-source-*` condition/range headers and
//! `x-amz-metadata-directive` must appear in SignedHeaders (header auth and
//! presigned). When the directive is `REPLACE`, a present `Content-Type`
//! must be signed too (the only REPLACE-applied header silo stores).
//! Matching the streaming `x-amz-decoded-content-length` rule — otherwise
//! `AccessDenied`.
//!
//! On success the keep access-key's project-scoped [`Caller`] is attached
//! to the request. Keep lookup failures and signature mismatches map to
//! S3 XML error codes without distinguishing "why" beyond what S3 itself
//! exposes (`InvalidAccessKeyId` vs `SignatureDoesNotMatch`).

mod auth_ok;
mod authenticate;
mod build_canonical_request;
mod build_string_to_sign;
mod canonical_headers;
mod canonical_query;
mod canonical_uri;
mod check_copy_source_signed_headers;
mod check_payload_sha256;
mod check_region;
mod check_streaming_signed_headers;
mod check_time_skew;
mod classify_streaming;
mod collapse_ws;
mod copy_source_dependent_headers;
mod ct_eq_hex;
mod derive_signing_key;
mod detect_sigv4_material;
mod empty_payload_sha256;
mod expected_payload_sha256;
mod form_urlencoded_iter;
mod header_str;
mod hex_encode;
mod hex_sha256;
mod hmac_sha256;
mod hmac_sha256_hex;
mod is_aws4_authorization;
mod is_hex_sha256;
mod max_presign_expires_secs;
mod max_skew_secs;
mod middleware;
mod parse_amz_date;
mod parse_auth;
mod parse_authorization_header;
mod parse_presigned;
mod parse_query_map;
mod parsed_auth;
mod path_segment;
mod percent_decode;
mod presign_query;
mod query_has;
mod query_value;
mod request_path;
mod sig_v4_state;
mod sign_request;
mod sigv4_error;
mod split_credential;
#[cfg(test)]
mod tests;
mod trailer_from_headers;
mod unsigned_payload;
mod verify_body_sha256;
mod xml_escape;
pub use crate::chunked::{STREAMING_PAYLOAD, STREAMING_SIGNED_TRAILER, STREAMING_UNSIGNED_TRAILER};
pub use canonical_uri::canonical_uri;
pub use check_payload_sha256::check_payload_sha256;
pub use derive_signing_key::derive_signing_key;
pub use empty_payload_sha256::EMPTY_PAYLOAD_SHA256;
pub use expected_payload_sha256::ExpectedPayloadSha256;
pub use hex_sha256::hex_sha256;
pub use hmac_sha256::hmac_sha256;
pub use hmac_sha256_hex::hmac_sha256_hex;
pub use is_aws4_authorization::is_aws4_authorization;
pub use max_presign_expires_secs::MAX_PRESIGN_EXPIRES_SECS;
pub use max_skew_secs::MAX_SKEW_SECS;
pub use middleware::middleware;
pub use presign_query::presign_query;
pub use sig_v4_state::SigV4State;
pub use sign_request::sign_request;
pub use unsigned_payload::UNSIGNED_PAYLOAD;
pub use verify_body_sha256::verify_body_sha256;

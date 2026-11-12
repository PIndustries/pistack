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

use crate::chunked::{StreamingAuth, StreamingKind, TrailerChecksum};
use crate::multipart::{s3_code_of, s3_err};
use axum::{
    extract::{OriginalUri, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use keep::Client as KeepClient;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use pistack_types::{ApiError, ApiResult, Caller};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Empty-body SHA-256 (hex), also the hash of `""`.
pub const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
pub use crate::chunked::{STREAMING_PAYLOAD, STREAMING_SIGNED_TRAILER, STREAMING_UNSIGNED_TRAILER};
/// Max clock skew accepted between `x-amz-date` / `X-Amz-Date` and server time.
pub const MAX_SKEW_SECS: i64 = 15 * 60;
/// Presigned URL maximum lifetime (7 days), matching S3.
pub const MAX_PRESIGN_EXPIRES_SECS: i64 = 7 * 24 * 60 * 60;

/// Characters left unescaped in an S3 SigV4 path segment (AWS rules).
/// Encode everything in NON_ALPHANUMERIC except `-`, `_`, `.`, `~`.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Query-string encoding set for SigV4 (same as path, plus encode `/`).
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Declared `x-amz-content-sha256` for the request, when it is a hex digest
/// that handlers must check against the streamed body.
#[derive(Clone, Debug)]
pub struct ExpectedPayloadSha256(pub String);

/// Shared state for the SigV4 middleware.
#[derive(Clone)]
pub struct SigV4State {
    pub keep: Option<KeepClient>,
    pub region: String,
}

/// Axum middleware: if no [`Caller`] is present yet and the request carries
/// SigV4 (header or query), verify it via keep and insert `credential.caller`.
///
/// A request that already has a bearer [`Caller`] **and** SigV4 material
/// (AWS4 `Authorization` or presigned `X-Amz-*` query) is rejected — the
/// two schemes must not be combined. Bearer-only requests pass through.
pub async fn middleware(State(state): State<SigV4State>, mut req: Request, next: Next) -> Response {
    let has_caller = req.extensions().get::<Caller>().is_some();
    let sigv4 = match detect_sigv4_material(&req) {
        Ok(v) => v,
        Err(err) => return sigv4_error(err),
    };
    if has_caller && sigv4 {
        return sigv4_error(s3_err(
            "InvalidRequest",
            "cannot combine bearer credentials with SigV4",
        ));
    }
    if has_caller {
        return next.run(req).await;
    }
    match authenticate(&state, &req) {
        Ok(None) => next.run(req).await,
        Ok(Some(auth)) => {
            // The gateway's role check ran before this caller existed, so
            // the same rule is applied here: writes need admin or `member`.
            if !matches!(req.method().as_str(), "GET" | "HEAD" | "OPTIONS")
                && !auth.caller.can_write()
            {
                return sigv4_error(s3_err(
                    "AccessDenied",
                    "this access key's principal may only read",
                ));
            }
            req.extensions_mut().insert(auth.caller);
            if let Some(sha) = auth.expected_payload_sha256 {
                req.extensions_mut().insert(ExpectedPayloadSha256(sha));
            }
            if let Some(streaming) = auth.streaming {
                req.extensions_mut().insert(streaming);
            }
            next.run(req).await
        }
        Err(err) => sigv4_error(err),
    }
}

/// `true` when the request carries SigV4 header or presigned query auth.
/// Errors on multiple `Authorization` headers.
fn detect_sigv4_material(req: &Request) -> ApiResult<bool> {
    let auths: Vec<_> = req
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .collect();
    if auths.len() > 1 {
        return Err(s3_err("InvalidArgument", "multiple Authorization headers"));
    }
    if let Some(raw) = auths.first().and_then(|v| v.to_str().ok()) {
        if is_aws4_authorization(raw) {
            return Ok(true);
        }
    }
    let q = req.uri().query().unwrap_or("");
    Ok(query_has(q, "X-Amz-Algorithm")
        || query_has(q, "X-Amz-Credential")
        || query_has(q, "X-Amz-Signature"))
}

/// Scheme token is exactly `AWS4-HMAC-SHA256` followed by a space.
pub fn is_aws4_authorization(value: &str) -> bool {
    const PREFIX: &[u8] = b"AWS4-HMAC-SHA256 ";
    let v = value.as_bytes();
    v.len() >= PREFIX.len() && v[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

struct AuthOk {
    caller: Caller,
    expected_payload_sha256: Option<String>,
    streaming: Option<StreamingAuth>,
}

/// `Ok(None)` = no SigV4 material (leave for bearer/`require_caller`).
fn authenticate(state: &SigV4State, req: &Request) -> ApiResult<Option<AuthOk>> {
    let parsed = match parse_auth(req)? {
        None => return Ok(None),
        Some(p) => p,
    };

    let streaming_kind = classify_streaming(&parsed.payload_hash, req.headers())?;
    if let Some(ref kind) = streaming_kind {
        if parsed.presigned {
            return Err(s3_err(
                "InvalidRequest",
                "presigned URLs cannot use STREAMING-* payloads",
            ));
        }
        if *req.method() != Method::PUT {
            return Err(s3_err(
                "InvalidRequest",
                "STREAMING-* payload is only valid on PUT (PutObject/UploadPart)",
            ));
        }
        check_streaming_signed_headers(&parsed, kind)?;
    }
    check_copy_source_signed_headers(&parsed, req.headers())?;

    check_region(&parsed.region, &state.region)?;
    check_time_skew(parsed.amz_date, parsed.expires_secs)?;

    let Some(keep) = state.keep.as_ref() else {
        // Same external shape as a bad key: do not advertise that keep is down.
        return Err(s3_err(
            "InvalidAccessKeyId",
            "The AWS Access Key Id you provided does not exist in our records.",
        ));
    };

    let credential = match keep.lookup_access_key(&parsed.access_key_id) {
        Ok(c) => c,
        Err(keep::KeepError::Unauthorized(_)) => {
            return Err(s3_err(
                "InvalidAccessKeyId",
                "The AWS Access Key Id you provided does not exist in our records.",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "silo sigv4: keep lookup failed");
            return Err(ApiError::internal("access key lookup failed"));
        }
    };

    let canonical = build_canonical_request(
        req.method(),
        request_path(req),
        req.uri().query(),
        req.headers(),
        &parsed,
    )?;
    let string_to_sign = build_string_to_sign(&parsed, &canonical);
    let signing_key =
        derive_signing_key(&credential.secret, &parsed.date_stamp, &parsed.region, "s3");
    let expected = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    if !ct_eq_hex(&expected, &parsed.signature) {
        return Err(s3_err(
            "SignatureDoesNotMatch",
            "The request signature we calculated does not match the signature you provided.",
        ));
    }

    let expected_payload = if is_hex_sha256(&parsed.payload_hash) {
        Some(parsed.payload_hash.to_ascii_lowercase())
    } else {
        None
    };

    let streaming = if let Some(kind) = streaming_kind {
        let decoded = header_str(req.headers(), "x-amz-decoded-content-length")
            .ok_or_else(|| {
                s3_err(
                    "InvalidRequest",
                    "missing x-amz-decoded-content-length for aws-chunked upload",
                )
            })?
            .parse::<u64>()
            .map_err(|_| {
                s3_err(
                    "InvalidRequest",
                    "x-amz-decoded-content-length must be a number",
                )
            })?;
        let amz_date = parsed.amz_date.format("%Y%m%dT%H%M%S").to_string() + "Z";
        let scope = format!("{}/{}/s3/aws4_request", parsed.date_stamp, parsed.region);
        Some(StreamingAuth {
            kind,
            seed_signature: parsed.signature.clone(),
            signing_key: signing_key.clone(),
            amz_date,
            scope,
            decoded_content_length: decoded,
        })
    } else {
        None
    };

    Ok(Some(AuthOk {
        caller: credential.caller,
        expected_payload_sha256: expected_payload,
        streaming,
    }))
}

fn classify_streaming(payload_hash: &str, headers: &HeaderMap) -> ApiResult<Option<StreamingKind>> {
    let p = payload_hash.trim();
    if p.eq_ignore_ascii_case(STREAMING_PAYLOAD) {
        return Ok(Some(StreamingKind::Signed));
    }
    if p.eq_ignore_ascii_case(STREAMING_UNSIGNED_TRAILER) {
        let checksum = trailer_from_headers(headers)?;
        return Ok(Some(StreamingKind::UnsignedTrailer { checksum }));
    }
    if p.eq_ignore_ascii_case(STREAMING_SIGNED_TRAILER) {
        let checksum = trailer_from_headers(headers)?;
        return Ok(Some(StreamingKind::SignedTrailer { checksum }));
    }
    if p.to_ascii_uppercase().starts_with("STREAMING-AWS4-ECDSA")
        || p.to_ascii_uppercase().contains("ECDSA-P256")
    {
        return Err(s3_err(
            "NotImplemented",
            "STREAMING-AWS4-ECDSA-P256-SHA256-* is not supported",
        ));
    }
    if p.to_ascii_uppercase().starts_with("STREAMING-") {
        return Err(s3_err(
            "NotImplemented",
            format!("streaming payload mode '{p}' is not supported"),
        ));
    }
    Ok(None)
}

fn check_streaming_signed_headers(parsed: &ParsedAuth, kind: &StreamingKind) -> ApiResult<()> {
    let has = |name: &str| {
        parsed
            .signed_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name))
    };
    if !has("x-amz-decoded-content-length") {
        return Err(s3_err(
            "AccessDenied",
            "x-amz-decoded-content-length must be in SignedHeaders for streaming uploads",
        ));
    }
    match kind {
        StreamingKind::UnsignedTrailer { .. } | StreamingKind::SignedTrailer { .. } => {
            if !has("x-amz-trailer") {
                return Err(s3_err(
                    "AccessDenied",
                    "x-amz-trailer must be in SignedHeaders for streaming trailer uploads",
                ));
            }
        }
        StreamingKind::Signed => {}
    }
    Ok(())
}

/// Headers that must be signed when present alongside `x-amz-copy-source`.
/// Shared by UploadPartCopy and CopyObject.
const COPY_SOURCE_DEPENDENT_HEADERS: &[&str] = &[
    "x-amz-copy-source-range",
    "x-amz-copy-source-if-match",
    "x-amz-copy-source-if-none-match",
    "x-amz-copy-source-if-modified-since",
    "x-amz-copy-source-if-unmodified-since",
    "x-amz-metadata-directive",
];

/// Require copy-source (and present dependents, including
/// `x-amz-metadata-directive`) in SignedHeaders so a captured signature
/// cannot be replayed with a swapped source or directive.
///
/// When the directive is `REPLACE`, any present `Content-Type` must also be
/// signed — that is the only request header silo applies on REPLACE today
/// (no `x-amz-meta-*` / Content-Disposition / Encoding / Cache-Control).
fn check_copy_source_signed_headers(parsed: &ParsedAuth, headers: &HeaderMap) -> ApiResult<()> {
    if header_str(headers, "x-amz-copy-source").is_none() {
        return Ok(());
    }
    let has = |name: &str| {
        parsed
            .signed_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name))
    };
    if !has("x-amz-copy-source") {
        return Err(s3_err(
            "AccessDenied",
            "x-amz-copy-source must be in SignedHeaders",
        ));
    }
    for name in COPY_SOURCE_DEPENDENT_HEADERS {
        if header_str(headers, name).is_some() && !has(name) {
            return Err(s3_err(
                "AccessDenied",
                format!("{name} must be in SignedHeaders when present"),
            ));
        }
    }
    let directive = header_str(headers, "x-amz-metadata-directive").map(|v| v.to_ascii_uppercase());
    if directive.as_deref() == Some("REPLACE")
        && header_str(headers, "content-type").is_some()
        && !has("content-type")
    {
        return Err(s3_err(
            "AccessDenied",
            "content-type must be in SignedHeaders when x-amz-metadata-directive is REPLACE",
        ));
    }
    Ok(())
}

fn trailer_from_headers(headers: &HeaderMap) -> ApiResult<TrailerChecksum> {
    let raw = header_str(headers, "x-amz-trailer").ok_or_else(|| {
        s3_err(
            "InvalidRequest",
            "missing x-amz-trailer for streaming trailer upload",
        )
    })?;
    // Clients may send one name, or a comma-separated list; we accept the first
    // checksum header name.
    let first = raw.split(',').next().unwrap_or(raw).trim();
    TrailerChecksum::parse(first)
}

#[derive(Debug)]
struct ParsedAuth {
    access_key_id: String,
    date_stamp: String,
    region: String,
    amz_date: DateTime<Utc>,
    signed_headers: Vec<String>,
    signature: String,
    payload_hash: String,
    /// Presigned only: `X-Amz-Expires` seconds from `amz_date`.
    expires_secs: Option<i64>,
    /// Presigned auth uses UNSIGNED-PAYLOAD in the canonical request and
    /// excludes `X-Amz-Signature` from the canonical query string.
    presigned: bool,
}

fn parse_auth(req: &Request) -> ApiResult<Option<ParsedAuth>> {
    let headers = req.headers();
    let auths: Vec<_> = headers.get_all(header::AUTHORIZATION).iter().collect();
    if auths.len() > 1 {
        return Err(s3_err("InvalidArgument", "multiple Authorization headers"));
    }
    if let Some(auth) = auths.first().and_then(|v| v.to_str().ok()) {
        if is_aws4_authorization(auth) {
            return Ok(Some(parse_authorization_header(auth, headers)?));
        }
        // `AWS4-HMAC-SHA256garbage` is not SigV4; leave for other auth.
    }
    let q = req.uri().query().unwrap_or("");
    if query_has(q, "X-Amz-Algorithm") {
        return Ok(Some(parse_presigned(q, headers)?));
    }
    Ok(None)
}

fn parse_authorization_header(auth: &str, headers: &HeaderMap) -> ApiResult<ParsedAuth> {
    let rest = auth
        .get(16..)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            s3_err(
                "AuthorizationHeaderMalformed",
                "Authorization header malformed",
            )
        })?;
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("Credential=") {
            credential = Some(v.trim());
        } else if let Some(v) = part.strip_prefix("SignedHeaders=") {
            signed_headers = Some(v.trim());
        } else if let Some(v) = part.strip_prefix("Signature=") {
            signature = Some(v.trim());
        }
    }
    let credential = credential.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing Credential",
        )
    })?;
    let signed_headers = signed_headers.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing SignedHeaders",
        )
    })?;
    let signature = signature.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing Signature",
        )
    })?;

    let (access_key_id, date_stamp, region) = split_credential(credential)?;
    let amz_date_raw = header_str(headers, "x-amz-date")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing x-amz-date header"))?;
    let amz_date = parse_amz_date(amz_date_raw)?;
    if amz_date.format("%Y%m%d").to_string() != date_stamp {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date does not match x-amz-date",
        ));
    }
    let payload_hash = header_str(headers, "x-amz-content-sha256")
        .ok_or_else(|| s3_err("InvalidRequest", "missing x-amz-content-sha256 header"))?
        .to_owned();

    Ok(ParsedAuth {
        access_key_id,
        date_stamp,
        region,
        amz_date,
        signed_headers: signed_headers
            .split(';')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        signature: signature.to_ascii_lowercase(),
        payload_hash,
        expires_secs: None,
        presigned: false,
    })
}

fn parse_presigned(query: &str, headers: &HeaderMap) -> ApiResult<ParsedAuth> {
    let params = parse_query_map(query);
    let algo = params
        .get("X-Amz-Algorithm")
        .map(String::as_str)
        .unwrap_or("");
    if algo != "AWS4-HMAC-SHA256" {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "X-Amz-Algorithm must be AWS4-HMAC-SHA256",
        ));
    }
    let credential = params
        .get("X-Amz-Credential")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Credential"))?;
    let (access_key_id, date_stamp, region) = split_credential(credential)?;
    let amz_date_raw = params
        .get("X-Amz-Date")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Date"))?;
    let amz_date = parse_amz_date(amz_date_raw)?;
    if amz_date.format("%Y%m%d").to_string() != date_stamp {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date does not match X-Amz-Date",
        ));
    }
    let expires: i64 = params
        .get("X-Amz-Expires")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Expires"))?
        .parse()
        .map_err(|_| s3_err("InvalidRequest", "X-Amz-Expires must be a number"))?;
    if expires <= 0 || expires > MAX_PRESIGN_EXPIRES_SECS {
        return Err(s3_err(
            "InvalidRequest",
            format!("X-Amz-Expires must be 1..={MAX_PRESIGN_EXPIRES_SECS}"),
        ));
    }
    let signed_headers = params
        .get("X-Amz-SignedHeaders")
        .ok_or_else(|| {
            s3_err(
                "AuthorizationHeaderMalformed",
                "missing X-Amz-SignedHeaders",
            )
        })?
        .split(';')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let signature = params
        .get("X-Amz-Signature")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Signature"))?
        .to_ascii_lowercase();

    // Presigned GETs use UNSIGNED-PAYLOAD; a query X-Amz-Content-Sha256 may
    // override. Header x-amz-content-sha256 is unusual on presign but accepted
    // if signed.
    let payload_hash = params
        .get("X-Amz-Content-Sha256")
        .cloned()
        .or_else(|| header_str(headers, "x-amz-content-sha256").map(|s| s.to_owned()))
        .unwrap_or_else(|| UNSIGNED_PAYLOAD.to_owned());

    Ok(ParsedAuth {
        access_key_id,
        date_stamp,
        region,
        amz_date,
        signed_headers,
        signature,
        payload_hash,
        expires_secs: Some(expires),
        presigned: true,
    })
}

fn split_credential(credential: &str) -> ApiResult<(String, String, String)> {
    // access_key_id/date/region/service/aws4_request
    let parts: Vec<&str> = credential.split('/').collect();
    if parts.len() != 5 || parts[3] != "s3" || parts[4] != "aws4_request" {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential must be <id>/<date>/<region>/s3/aws4_request",
        ));
    }
    if parts[1].len() != 8 || !parts[1].bytes().all(|b| b.is_ascii_digit()) {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date must be YYYYMMDD",
        ));
    }
    Ok((
        parts[0].to_owned(),
        parts[1].to_owned(),
        parts[2].to_owned(),
    ))
}

fn check_region(got: &str, want: &str) -> ApiResult<()> {
    if got != want {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            format!("Region '{got}' does not match configured region '{want}'"),
        ));
    }
    Ok(())
}

fn check_time_skew(amz_date: DateTime<Utc>, expires_secs: Option<i64>) -> ApiResult<()> {
    let now = Utc::now();
    match expires_secs {
        Some(exp) => {
            let end = amz_date + chrono::Duration::seconds(exp);
            if now < amz_date - chrono::Duration::seconds(MAX_SKEW_SECS) || now > end {
                return Err(s3_err("AccessDenied", "Request has expired"));
            }
        }
        None => {
            let delta = (now - amz_date).num_seconds().abs();
            if delta > MAX_SKEW_SECS {
                return Err(s3_err(
                    "RequestTimeTooSkewed",
                    "The difference between the request time and the current time is too large."
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn build_canonical_request(
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    auth: &ParsedAuth,
) -> ApiResult<String> {
    let mut out = String::new();
    out.push_str(method.as_str());
    out.push('\n');
    out.push_str(&canonical_uri(path));
    out.push('\n');
    out.push_str(&canonical_query(query, auth.presigned));
    out.push('\n');
    out.push_str(&canonical_headers(headers, &auth.signed_headers)?);
    out.push('\n');
    out.push_str(&auth.signed_headers.join(";"));
    out.push('\n');
    out.push_str(&auth.payload_hash);
    Ok(out)
}

fn build_string_to_sign(auth: &ParsedAuth, canonical: &str) -> String {
    let scope = format!("{}/{}/s3/aws4_request", auth.date_stamp, auth.region);
    format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        auth.amz_date.format("%Y%m%dT%H%M%SZ"),
        scope,
        hex_sha256(canonical.as_bytes())
    )
}

/// URI-encode each path segment per S3 SigV4 rules; keep `/` separators.
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_owned();
    }
    let trailing = path.ends_with('/');
    let mut out = String::from("/");
    let mut first = true;
    for seg in path.trim_start_matches('/').split('/') {
        if seg.is_empty() {
            continue;
        }
        if !first {
            out.push('/');
        }
        first = false;
        out.push_str(&utf8_percent_encode(seg, PATH_SEGMENT).to_string());
    }
    if trailing {
        out.push('/');
    }
    out
}

fn canonical_query(query: Option<&str>, presigned: bool) -> String {
    let Some(q) = query.filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (k, v) in form_urlencoded_iter(q) {
        if presigned && k.eq_ignore_ascii_case("X-Amz-Signature") {
            continue;
        }
        pairs.push((
            utf8_percent_encode(&k, QUERY_VALUE).to_string(),
            utf8_percent_encode(&v, QUERY_VALUE).to_string(),
        ));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn canonical_headers(headers: &HeaderMap, signed: &[String]) -> ApiResult<String> {
    let mut out = String::new();
    for name in signed {
        let values: Vec<&str> = headers
            .get_all(name.as_str())
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        if values.is_empty() {
            // Host may come from URI authority when missing; require it signed.
            if name == "host" {
                return Err(s3_err("InvalidRequest", "signed header 'host' is missing"));
            }
            return Err(s3_err(
                "InvalidRequest",
                format!("signed header '{name}' is missing"),
            ));
        }
        let joined = values
            .iter()
            .map(|v| collapse_ws(v))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(name);
        out.push(':');
        out.push_str(&joined);
        out.push('\n');
    }
    Ok(out)
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

pub fn derive_signing_key(secret: &[u8], date: &str, region: &str, service: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + secret.len());
    key.extend_from_slice(b"AWS4");
    key.extend_from_slice(secret);
    let k_date = hmac_sha256(&key, date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    hex_encode(&hmac_sha256(key, data))
}

pub fn hex_sha256(data: &[u8]) -> String {
    hex_encode(Sha256::digest(data).as_slice())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn ct_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_amz_date(s: &str) -> ApiResult<DateTime<Utc>> {
    let s = s.trim();
    // Literal trailing `Z` (UTC). chrono's `%Z` is a timezone name, not this.
    if s.len() != 16 || !s.ends_with('Z') {
        return Err(s3_err(
            "InvalidRequest",
            "invalid AWS date format, want YYYYMMDDTHHMMSSZ",
        ));
    }
    chrono::NaiveDateTime::parse_from_str(&s[..15], "%Y%m%dT%H%M%S")
        .map(|n| n.and_utc())
        .map_err(|_| {
            s3_err(
                "InvalidRequest",
                "invalid AWS date format, want YYYYMMDDTHHMMSSZ",
            )
        })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn request_path(req: &Request) -> &str {
    if let Some(OriginalUri(uri)) = req.extensions().get::<OriginalUri>() {
        return uri.path();
    }
    req.uri().path()
}

fn query_has(query: &str, key: &str) -> bool {
    form_urlencoded_iter(query).any(|(k, _)| k.eq_ignore_ascii_case(key))
}

fn parse_query_map(query: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in form_urlencoded_iter(query) {
        // First wins for auth fields; last is fine for our use.
        map.insert(k, v);
    }
    map
}

/// Decode `application/x-www-form-urlencoded` pairs without treating `+` as
/// space for keys that are already percent-encoded the SigV4 way. We still
/// accept `+` as space for ordinary query values (S3 clients percent-encode).
fn form_urlencoded_iter(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query.split('&').filter(|p| !p.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(k), percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = percent_encoding::percent_decode_str(s).collect::<Vec<u8>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// After a streaming PUT, ensure the body SHA-256 matches the signed declaration.
pub fn check_payload_sha256(
    expected: Option<&ExpectedPayloadSha256>,
    actual_hex: &str,
) -> ApiResult<()> {
    let Some(ExpectedPayloadSha256(want)) = expected else {
        return Ok(());
    };
    if !ct_eq_hex(want, &actual_hex.to_ascii_lowercase()) {
        return Err(s3_err(
            "XAmzContentSHA256Mismatch",
            "The provided 'x-amz-content-sha256' header does not match what was computed.",
        ));
    }
    Ok(())
}

/// Hash `body` and compare to a declared SigV4 payload hash (no-op if none).
pub fn verify_body_sha256(expected: Option<&ExpectedPayloadSha256>, body: &[u8]) -> ApiResult<()> {
    check_payload_sha256(expected, &hex_sha256(body))
}

fn sigv4_error(err: ApiError) -> Response {
    let code = s3_code_of(&err).unwrap_or("AccessDenied");
    let status = match code {
        "NotImplemented" => StatusCode::NOT_IMPLEMENTED,
        "AuthorizationHeaderMalformed"
        | "InvalidRequest"
        | "InvalidArgument"
        | "IncompleteBody"
        | "BadDigest"
        | "XAmzContentSHA256Mismatch"
        | "XAmzContentChecksumMismatch" => StatusCode::BAD_REQUEST,
        "InvalidAccessKeyId"
        | "SignatureDoesNotMatch"
        | "AccessDenied"
        | "RequestTimeTooSkewed" => StatusCode::FORBIDDEN,
        _ => err.status(),
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{}</Code><Message>{}</Message></Error>",
        xml_escape(code),
        xml_escape(&err.message)
    );
    (status, [(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Helper used by tests and clients: sign a request the same way the server verifies.
#[allow(clippy::too_many_arguments)]
pub fn sign_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    access_key_id: &str,
    secret: &[u8],
    region: &str,
    amz_date: &str,
) -> String {
    let date_stamp: String = amz_date.chars().take(8).collect();
    let mut header_map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in headers {
        header_map
            .entry(k.to_ascii_lowercase())
            .and_modify(|e| {
                e.push(',');
                e.push_str(&collapse_ws(v));
            })
            .or_insert_with(|| collapse_ws(v));
    }
    let signed: Vec<String> = header_map.keys().cloned().collect();
    let mut canonical = String::new();
    canonical.push_str(method);
    canonical.push('\n');
    canonical.push_str(&canonical_uri(path));
    canonical.push('\n');
    // For signing (not presign verification strip), include all query params.
    let auth = ParsedAuth {
        access_key_id: access_key_id.to_owned(),
        date_stamp: date_stamp.clone(),
        region: region.to_owned(),
        amz_date: parse_amz_date(amz_date).expect("amz_date"),
        signed_headers: signed.clone(),
        signature: String::new(),
        payload_hash: payload_hash.to_owned(),
        expires_secs: None,
        presigned: false,
    };
    let cq = canonical_query(if query.is_empty() { None } else { Some(query) }, false);
    canonical.push_str(&cq);
    canonical.push('\n');
    for name in &signed {
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(header_map.get(name).map(String::as_str).unwrap_or(""));
        canonical.push('\n');
    }
    canonical.push('\n');
    canonical.push_str(&signed.join(";"));
    canonical.push('\n');
    canonical.push_str(payload_hash);
    let sts = build_string_to_sign(&auth, &canonical);
    let key = derive_signing_key(secret, &date_stamp, region, "s3");
    hmac_sha256_hex(&key, sts.as_bytes())
}

/// Build a presigned query string (without path) for GET/PUT.
#[allow(clippy::too_many_arguments)]
pub fn presign_query(
    method: &str,
    path: &str,
    host: &str,
    access_key_id: &str,
    secret: &[u8],
    region: &str,
    amz_date: &str,
    expires: i64,
    extra_query: &str,
) -> String {
    let date_stamp: String = amz_date.chars().take(8).collect();
    let credential = format!("{access_key_id}/{date_stamp}/{region}/s3/aws4_request");
    let mut params: Vec<(String, String)> = vec![
        ("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into()),
        ("X-Amz-Credential".into(), credential),
        ("X-Amz-Date".into(), amz_date.to_owned()),
        ("X-Amz-Expires".into(), expires.to_string()),
        ("X-Amz-SignedHeaders".into(), "host".into()),
    ];
    for (k, v) in form_urlencoded_iter(extra_query) {
        params.push((k, v));
    }
    params.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let canonical_qs = params
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                utf8_percent_encode(k, QUERY_VALUE),
                utf8_percent_encode(v, QUERY_VALUE)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let canonical = format!(
        "{method}\n{}\n{canonical_qs}\nhost:{host}\n\nhost\n{UNSIGNED_PAYLOAD}",
        canonical_uri(path)
    );
    let auth = ParsedAuth {
        access_key_id: access_key_id.to_owned(),
        date_stamp: date_stamp.clone(),
        region: region.to_owned(),
        amz_date: parse_amz_date(amz_date).expect("amz_date"),
        signed_headers: vec!["host".into()],
        signature: String::new(),
        payload_hash: UNSIGNED_PAYLOAD.to_owned(),
        expires_secs: Some(expires),
        presigned: true,
    };
    let sts = build_string_to_sign(&auth, &canonical);
    let key = derive_signing_key(secret, &date_stamp, region, "s3");
    let sig = hmac_sha256_hex(&key, sts.as_bytes());
    format!("{canonical_qs}&X-Amz-Signature={sig}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS published GET Object example (header auth).
    #[test]
    fn aws_get_object_header_vector() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let canonical = "GET\n\
/test.txt\n\
\n\
host:examplebucket.s3.amazonaws.com\n\
range:bytes=0-9\n\
x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
x-amz-date:20130524T000000Z\n\
\n\
host;range;x-amz-content-sha256;x-amz-date\n\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(
            hex_sha256(canonical.as_bytes()),
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        let sts = "AWS4-HMAC-SHA256\n\
20130524T000000Z\n\
20130524/us-east-1/s3/aws4_request\n\
7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        assert_eq!(
            hmac_sha256_hex(&key, sts.as_bytes()),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        let sig = sign_request(
            "GET",
            "/test.txt",
            "",
            &[
                ("host".into(), "examplebucket.s3.amazonaws.com".into()),
                ("range".into(), "bytes=0-9".into()),
                ("x-amz-content-sha256".into(), EMPTY_PAYLOAD_SHA256.into()),
                ("x-amz-date".into(), "20130524T000000Z".into()),
            ],
            EMPTY_PAYLOAD_SHA256,
            "AKIAIOSFODNN7EXAMPLE",
            secret,
            "us-east-1",
            "20130524T000000Z",
        );
        assert_eq!(
            sig,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn aws_presigned_get_vector() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let qs = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host";
        let canonical = format!(
            "GET\n/test.txt\n{qs}\nhost:examplebucket.s3.amazonaws.com\n\nhost\nUNSIGNED-PAYLOAD"
        );
        assert_eq!(
            hex_sha256(canonical.as_bytes()),
            "3bfa292879f6447bbcda7001decf97f4a54dc650c8942174ae0a9121cf58ad04"
        );
        let got = presign_query(
            "GET",
            "/test.txt",
            "examplebucket.s3.amazonaws.com",
            "AKIAIOSFODNN7EXAMPLE",
            secret,
            "us-east-1",
            "20130524T000000Z",
            86400,
            "",
        );
        assert!(got.ends_with(
            "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        ));
    }

    #[test]
    fn aws4_scheme_requires_trailing_space() {
        assert!(is_aws4_authorization(
            "AWS4-HMAC-SHA256 Credential=x/20200101/pistack/s3/aws4_request, Signature=ab"
        ));
        assert!(!is_aws4_authorization("AWS4-HMAC-SHA256garbage"));
        assert!(!is_aws4_authorization("AWS4-HMAC-SHA256"));
        assert!(!is_aws4_authorization("Bearer x"));
    }

    #[test]
    fn canonical_uri_encodes_segments() {
        assert_eq!(canonical_uri("/bucket/my key"), "/bucket/my%20key");
        assert_eq!(canonical_uri("/"), "/");
        assert_eq!(canonical_uri("/a/b"), "/a/b");
    }
}

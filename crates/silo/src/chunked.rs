// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! AWS `aws-chunked` body decoding for SigV4 streaming uploads.
//!
//! Supported `x-amz-content-sha256` values:
//! * [`STREAMING_PAYLOAD`] — signed chunks (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`)
//! * [`STREAMING_UNSIGNED_TRAILER`] — unsigned chunks + trailing checksum
//! * [`STREAMING_SIGNED_TRAILER`] — signed chunks + trailing checksum + trailer signature
//!
//! Identity-review rules:
//! 1. Chunk signatures chain from the Authorization seed signature; compares
//!    are constant-time ([`ct_eq_hex`]).
//! 2. A final zero-length chunk is required and verified; ending without it
//!    yields `IncompleteBody` and nothing is stored.
//! 3. `x-amz-decoded-content-length` must equal the decoded byte count.
//! 4. Each chunk is buffered (capped at [`MAX_CHUNK_BYTES`]), verified, then
//!    yielded — no bytes reach the temp-file writer before that chunk passes.
//! 5. Chunk-header lines and trailer sections are length-capped; CRLF search is
//!    incremental. Trailers wait for a full `\r\n\r\n` before parsing.
//!    Trailing junk after the final chunk/trailer yields `IncompleteBody`.

use crate::multipart::s3_err;
use crate::sigv4::EMPTY_PAYLOAD_SHA256;
use base64::Engine;
use bytes::{Bytes, BytesMut};
use futures::Stream;
use hmac::{Hmac, Mac};
use pistack_types::{ApiError, ApiResult};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::task::{Context, Poll};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Hard cap on one aws-chunked data chunk so verify-before-write stays bounded.
pub const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
/// Cap on a single chunk-header line (`size;chunk-signature=…`) before CRLF.
pub const MAX_CHUNK_HEADER_BYTES: usize = 128;
/// Cap on the entire trailing-headers section (including final blank line).
pub const MAX_TRAILER_BYTES: usize = 4 * 1024;
/// Cap on trailing header lines (excluding the blank terminator).
pub const MAX_TRAILER_LINES: usize = 8;

pub const STREAMING_PAYLOAD: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD";
pub const STREAMING_UNSIGNED_TRAILER: &str = "STREAMING-UNSIGNED-PAYLOAD-TRAILER";
pub const STREAMING_SIGNED_TRAILER: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER";

/// Checksum algorithm named by `x-amz-trailer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailerChecksum {
    Crc32,
    Crc32c,
    Sha256,
    Sha1,
}

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

    pub fn header_name(self) -> &'static str {
        match self {
            Self::Crc32 => "x-amz-checksum-crc32",
            Self::Crc32c => "x-amz-checksum-crc32c",
            Self::Sha256 => "x-amz-checksum-sha256",
            Self::Sha1 => "x-amz-checksum-sha1",
        }
    }
}

/// How the framed body is authenticated.
#[derive(Clone, Debug)]
pub enum StreamingKind {
    Signed,
    UnsignedTrailer { checksum: TrailerChecksum },
    SignedTrailer { checksum: TrailerChecksum },
}

/// Seed material from a verified SigV4 request, needed to decode the body.
#[derive(Clone, Debug)]
pub struct StreamingAuth {
    pub kind: StreamingKind,
    pub seed_signature: String,
    pub signing_key: Vec<u8>,
    pub amz_date: String,
    pub scope: String,
    pub decoded_content_length: u64,
}

impl StreamingAuth {
    pub fn requires_chunk_signatures(&self) -> bool {
        matches!(
            self.kind,
            StreamingKind::Signed | StreamingKind::SignedTrailer { .. }
        )
    }

    pub fn trailer_checksum(&self) -> Option<TrailerChecksum> {
        match self.kind {
            StreamingKind::Signed => None,
            StreamingKind::UnsignedTrailer { checksum }
            | StreamingKind::SignedTrailer { checksum } => Some(checksum),
        }
    }
}

/// AWS chunk signature (documented string-to-sign).
pub fn chunk_signature(
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    prev_signature: &str,
    chunk_data: &[u8],
) -> String {
    let chunk_hash = hex_sha256(chunk_data);
    let sts = format!(
        "AWS4-HMAC-SHA256-PAYLOAD\n{amz_date}\n{scope}\n{prev_signature}\n{EMPTY_PAYLOAD_SHA256}\n{chunk_hash}"
    );
    hmac_hex(signing_key, sts.as_bytes())
}

/// Trailer signature for `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`.
pub fn trailer_signature(
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    prev_signature: &str,
    trailer_header_line: &str,
) -> String {
    let trailer_hash = hex_sha256(trailer_header_line.as_bytes());
    let sts =
        format!("AWS4-HMAC-SHA256-TRAILER\n{amz_date}\n{scope}\n{prev_signature}\n{trailer_hash}");
    hmac_hex(signing_key, sts.as_bytes())
}

fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key");
    mac.update(msg);
    hex_encode(mac.finalize().into_bytes().as_slice())
}

fn hex_sha256(data: &[u8]) -> String {
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

/// Constant-time hex compare (length mismatch → false without leaking content).
pub fn ct_eq_hex(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.ct_eq(b))
}

struct ChecksumState {
    algo: TrailerChecksum,
    crc32: crc32fast::Hasher,
    crc32c: u32,
    sha256: Sha256,
    sha1: Sha1,
}

impl ChecksumState {
    fn new(algo: TrailerChecksum) -> Self {
        Self {
            algo,
            crc32: crc32fast::Hasher::new(),
            crc32c: 0,
            sha256: Sha256::new(),
            sha1: Sha1::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self.algo {
            TrailerChecksum::Crc32 => self.crc32.update(data),
            TrailerChecksum::Crc32c => {
                self.crc32c = crc32c::crc32c_append(self.crc32c, data);
            }
            TrailerChecksum::Sha256 => self.sha256.update(data),
            TrailerChecksum::Sha1 => self.sha1.update(data),
        }
    }

    fn finalize_b64(self) -> String {
        let engine = base64::engine::general_purpose::STANDARD;
        match self.algo {
            TrailerChecksum::Crc32 => engine.encode(self.crc32.finalize().to_be_bytes()),
            TrailerChecksum::Crc32c => engine.encode(self.crc32c.to_be_bytes()),
            TrailerChecksum::Sha256 => engine.encode(self.sha256.finalize().as_slice()),
            TrailerChecksum::Sha1 => engine.encode(self.sha1.finalize().as_slice()),
        }
    }
}

/// Decode a wire `aws-chunked` body into verified plain object bytes.
pub fn decode_stream<S, E>(body: S, auth: StreamingAuth) -> DecodedStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    use futures::StreamExt;
    let body = body
        .map(|item| item.map_err(|e| s3_err("IncompleteBody", format!("reading body: {e}"))))
        .boxed();
    let checksum = auth.trailer_checksum().map(ChecksumState::new);
    let prev_sig = auth.seed_signature.clone();
    DecodedStream {
        body,
        auth,
        buf: BytesMut::new(),
        prev_sig,
        decoded: 0,
        phase: Phase::NeedHeader,
        checksum,
        pending: None,
        finished: false,
        scan_from: 0,
    }
}

enum Phase {
    NeedHeader,
    NeedData {
        size: usize,
        declared_sig: Option<String>,
        chunk_buf: Vec<u8>,
    },
    NeedTrailer,
    /// Framing complete; any further body bytes are an error.
    ExpectEof,
    Done,
}

pub struct DecodedStream {
    body: Pin<Box<dyn Stream<Item = Result<Bytes, ApiError>> + Send>>,
    auth: StreamingAuth,
    buf: BytesMut,
    prev_sig: String,
    decoded: u64,
    phase: Phase,
    checksum: Option<ChecksumState>,
    pending: Option<Bytes>,
    finished: bool,
    /// Incremental CRLF / trailer-terminator scan offset.
    scan_from: usize,
}

impl Stream for DecodedStream {
    type Item = Result<Bytes, ApiError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(b) = this.pending.take() {
            return Poll::Ready(Some(Ok(b)));
        }
        if this.finished {
            return Poll::Ready(None);
        }

        loop {
            match this.pump() {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => match this.body.as_mut().poll_next(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(None) => {
                        if matches!(this.phase, Phase::ExpectEof) {
                            this.phase = Phase::Done;
                            this.finished = true;
                            return Poll::Ready(None);
                        }
                        return Poll::Ready(Some(Err(this.on_wire_eof())));
                    }
                    Poll::Ready(Some(Ok(chunk))) => {
                        if matches!(this.phase, Phase::ExpectEof) && !chunk.is_empty() {
                            this.finished = true;
                            return Poll::Ready(Some(Err(s3_err(
                                "IncompleteBody",
                                "unexpected bytes after aws-chunked final chunk",
                            ))));
                        }
                        if !chunk.is_empty() {
                            this.buf.extend_from_slice(&chunk);
                        }
                        continue;
                    }
                    Poll::Ready(Some(Err(e))) => {
                        this.finished = true;
                        return Poll::Ready(Some(Err(e)));
                    }
                },
                Poll::Ready(Some(Ok(()))) => {
                    if let Some(b) = this.pending.take() {
                        return Poll::Ready(Some(Ok(b)));
                    }
                    if this.finished {
                        return Poll::Ready(None);
                    }
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    this.finished = true;
                    return Poll::Ready(Some(Err(e)));
                }
            }
        }
    }
}

impl DecodedStream {
    /// `Ready(None)` = need more wire bytes.
    fn pump(&mut self) -> Poll<Option<Result<(), ApiError>>> {
        match &mut self.phase {
            Phase::Done => {
                self.finished = true;
                Poll::Ready(Some(Ok(())))
            }
            Phase::ExpectEof => {
                if !self.buf.is_empty() {
                    return Poll::Ready(Some(Err(s3_err(
                        "IncompleteBody",
                        "unexpected bytes after aws-chunked final chunk",
                    ))));
                }
                // Wait for body EOF (or more bytes → rejected in poll_next).
                Poll::Ready(None)
            }
            Phase::NeedHeader => {
                match take_line_capped(
                    &mut self.buf,
                    &mut self.scan_from,
                    MAX_CHUNK_HEADER_BYTES,
                    "aws-chunked chunk header line too long",
                ) {
                    Err(e) => Poll::Ready(Some(Err(e))),
                    Ok(None) => Poll::Ready(None),
                    Ok(Some(line)) => {
                        match parse_chunk_header(&line, self.auth.requires_chunk_signatures()) {
                            Err(e) => Poll::Ready(Some(Err(e))),
                            Ok((size, sig)) => {
                                if size > MAX_CHUNK_BYTES {
                                    return Poll::Ready(Some(Err(s3_err(
                                        "InvalidRequest",
                                        format!(
                                            "aws-chunked chunk size {size} exceeds limit {}",
                                            MAX_CHUNK_BYTES
                                        ),
                                    ))));
                                }
                                self.phase = Phase::NeedData {
                                    size,
                                    declared_sig: sig,
                                    chunk_buf: Vec::with_capacity(size.min(64 * 1024)),
                                };
                                Poll::Ready(Some(Ok(())))
                            }
                        }
                    }
                }
            }
            Phase::NeedData {
                size,
                declared_sig,
                chunk_buf,
            } => {
                let size = *size;
                if size == 0 {
                    // Final chunk: optional empty-data CRLF (AWS signed style).
                    if self.buf.starts_with(b"\r\n") {
                        let _ = self.buf.split_to(2);
                        self.scan_from = 0;
                    } else if self.buf.is_empty() {
                        return Poll::Ready(None);
                    } else if self.auth.trailer_checksum().is_none() {
                        if self.buf.len() >= 2 {
                            return Poll::Ready(Some(Err(s3_err(
                                "InvalidRequest",
                                "aws-chunked final chunk missing trailing CRLF",
                            ))));
                        }
                        return Poll::Ready(None);
                    }

                    let declared = declared_sig.clone();
                    if let Err(e) = self.verify_chunk_sig(declared.as_deref(), b"") {
                        return Poll::Ready(Some(Err(e)));
                    }
                    if self.decoded != self.auth.decoded_content_length {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            format!(
                                "decoded length {} does not match x-amz-decoded-content-length {}",
                                self.decoded, self.auth.decoded_content_length
                            ),
                        ))));
                    }
                    self.scan_from = 0;
                    if self.auth.trailer_checksum().is_some() {
                        self.phase = Phase::NeedTrailer;
                        return Poll::Ready(Some(Ok(())));
                    }
                    if !self.buf.is_empty() {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            "unexpected bytes after aws-chunked final chunk",
                        ))));
                    }
                    self.phase = Phase::ExpectEof;
                    return Poll::Ready(Some(Ok(())));
                }

                let need_total = size + 2;
                let have = chunk_buf.len() + self.buf.len();
                if have < need_total {
                    let take = (size - chunk_buf.len()).min(self.buf.len());
                    if take > 0 {
                        chunk_buf.extend_from_slice(&self.buf.split_to(take));
                        self.scan_from = 0;
                    }
                    return Poll::Ready(None);
                }
                let remain = size - chunk_buf.len();
                if remain > 0 {
                    chunk_buf.extend_from_slice(&self.buf.split_to(remain));
                }
                if self.buf.len() < 2 {
                    return Poll::Ready(None);
                }
                if &self.buf[..2] != b"\r\n" {
                    return Poll::Ready(Some(Err(s3_err(
                        "InvalidRequest",
                        "aws-chunked chunk missing trailing CRLF",
                    ))));
                }
                let _ = self.buf.split_to(2);
                self.scan_from = 0;

                let data = std::mem::take(chunk_buf);
                let declared = declared_sig.clone();
                if let Err(e) = self.verify_chunk_sig(declared.as_deref(), &data) {
                    return Poll::Ready(Some(Err(e)));
                }

                let next = self.decoded.saturating_add(data.len() as u64);
                if next > self.auth.decoded_content_length {
                    return Poll::Ready(Some(Err(s3_err(
                        "IncompleteBody",
                        format!(
                            "decoded length exceeds x-amz-decoded-content-length {}",
                            self.auth.decoded_content_length
                        ),
                    ))));
                }
                self.decoded = next;
                if let Some(cs) = self.checksum.as_mut() {
                    cs.update(&data);
                }
                self.pending = Some(Bytes::from(data));
                self.phase = Phase::NeedHeader;
                Poll::Ready(Some(Ok(())))
            }
            Phase::NeedTrailer => match finish_trailer(self) {
                Err(e) => Poll::Ready(Some(Err(e))),
                Ok(true) => {
                    if !self.buf.is_empty() {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            "unexpected bytes after aws-chunked trailer",
                        ))));
                    }
                    self.phase = Phase::ExpectEof;
                    Poll::Ready(Some(Ok(())))
                }
                Ok(false) => Poll::Ready(None),
            },
        }
    }

    fn verify_chunk_sig(&mut self, declared: Option<&str>, data: &[u8]) -> ApiResult<()> {
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

    fn on_wire_eof(&mut self) -> ApiError {
        self.finished = true;
        match self.phase {
            Phase::Done | Phase::ExpectEof => {
                s3_err("IncompleteBody", "unexpected state after body EOF")
            }
            Phase::NeedHeader | Phase::NeedData { .. } => s3_err(
                "IncompleteBody",
                "aws-chunked body ended before the final zero-length chunk",
            ),
            Phase::NeedTrailer => s3_err(
                "IncompleteBody",
                "aws-chunked body ended before trailing checksum headers",
            ),
        }
    }
}

/// Wait until a full trailer (`…\r\n\r\n`) is buffered, then parse it.
/// Returns `Ok(true)` when consumed; `Ok(false)` needs more bytes.
fn finish_trailer(this: &mut DecodedStream) -> ApiResult<bool> {
    let checksum_algo = this
        .auth
        .trailer_checksum()
        .ok_or_else(|| s3_err("InvalidRequest", "internal: trailer phase without checksum"))?;

    if this.buf.len() > MAX_TRAILER_BYTES {
        return Err(s3_err(
            "InvalidRequest",
            "aws-chunked trailing headers exceed size limit",
        ));
    }

    let Some(end) = find_double_crlf(&this.buf, this.scan_from) else {
        // No terminator yet — advance scan cursor (overlap of 3 for `\r\n\r`).
        this.scan_from = this.buf.len().saturating_sub(3);
        if this.buf.len() > MAX_TRAILER_BYTES {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked trailing headers exceed size limit",
            ));
        }
        return Ok(false);
    };

    let section = this.buf.split_to(end);
    this.scan_from = 0;
    // `section` ends with `\r\n\r\n`.
    let text = String::from_utf8_lossy(&section[..section.len().saturating_sub(4)]);
    let mut lines: Vec<&str> = text.split("\r\n").collect();
    // split may leave a trailing empty from final content; drop empties at end only
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.len() > MAX_TRAILER_LINES {
        return Err(s3_err(
            "InvalidRequest",
            "aws-chunked trailing headers exceed line limit",
        ));
    }

    let want_name = checksum_algo.header_name();
    let mut got_value: Option<String> = None;
    let mut trailer_sig: Option<String> = None;
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = split_header_line(line)?;
        let k = k.to_ascii_lowercase();
        if k == want_name {
            got_value = Some(v.to_owned());
        } else if k == "x-amz-trailer-signature" {
            trailer_sig = Some(v.to_ascii_lowercase());
        } else {
            return Err(s3_err(
                "InvalidRequest",
                format!("unexpected trailing header '{k}'"),
            ));
        }
    }
    let got_value = got_value.ok_or_else(|| {
        s3_err(
            "InvalidRequest",
            format!("missing trailing checksum header {want_name}"),
        )
    })?;

    if matches!(this.auth.kind, StreamingKind::SignedTrailer { .. }) {
        let declared = trailer_sig.ok_or_else(|| {
            s3_err(
                "InvalidRequest",
                "missing x-amz-trailer-signature trailing header",
            )
        })?;
        let header_line = format!("{want_name}:{got_value}\n");
        let expected = trailer_signature(
            &this.auth.signing_key,
            &this.auth.amz_date,
            &this.auth.scope,
            &this.prev_sig,
            &header_line,
        );
        if !ct_eq_hex(&expected, &declared) {
            return Err(s3_err(
                "SignatureDoesNotMatch",
                "The request signature we calculated does not match the signature you provided.",
            ));
        }
    } else if trailer_sig.is_some() {
        return Err(s3_err(
            "InvalidRequest",
            "x-amz-trailer-signature not expected for STREAMING-UNSIGNED-PAYLOAD-TRAILER",
        ));
    }

    let computed = this
        .checksum
        .take()
        .ok_or_else(|| s3_err("InternalError", "checksum state missing"))?
        .finalize_b64();
    if computed != got_value {
        return Err(s3_err(
            "XAmzContentChecksumMismatch",
            "The trailing checksum you specified did not match what the server calculated.",
        ));
    }
    Ok(true)
}

fn split_header_line(line: &str) -> ApiResult<(&str, &str)> {
    let (k, v) = line
        .split_once(':')
        .ok_or_else(|| s3_err("InvalidRequest", "malformed trailing header"))?;
    Ok((k.trim(), v.trim()))
}

/// Find `\r\n\r\n` starting search near `from`; returns end index after terminator.
fn find_double_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let start = from.saturating_sub(3);
    let mut i = start;
    while i + 3 < buf.len() {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

/// Find `\r\n` with incremental start; returns index of `\r`.
fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let start = from.saturating_sub(1);
    buf[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| start + p)
}

/// Cap line length; scan incrementally via `scan_from`.
fn take_line_capped(
    buf: &mut BytesMut,
    scan_from: &mut usize,
    max: usize,
    oversize_msg: &str,
) -> ApiResult<Option<String>> {
    if let Some(pos) = find_crlf(buf, *scan_from) {
        *scan_from = 0;
        let line = buf.split_to(pos);
        let _ = buf.split_to(2);
        return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
    }
    if buf.len() > max {
        return Err(s3_err("InvalidRequest", oversize_msg));
    }
    *scan_from = buf.len().saturating_sub(1);
    Ok(None)
}

fn parse_chunk_header(line: &str, require_sig: bool) -> ApiResult<(usize, Option<String>)> {
    let line = line.trim();
    if line.is_empty() {
        return Err(s3_err("InvalidRequest", "empty aws-chunked chunk header"));
    }
    if let Some((size_hex, rest)) = line.split_once(';') {
        let size = parse_hex_size(size_hex)?;
        let sig = parse_chunk_sig(rest.trim(), require_sig)?;
        Ok((size, sig))
    } else {
        let size = parse_hex_size(line)?;
        if require_sig {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked chunk missing chunk-signature",
            ));
        }
        Ok((size, None))
    }
}

fn parse_chunk_sig(rest: &str, require_sig: bool) -> ApiResult<Option<String>> {
    let rest = rest.trim();
    if rest.is_empty() {
        if require_sig {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked chunk missing chunk-signature",
            ));
        }
        return Ok(None);
    }
    let (k, v) = rest
        .split_once('=')
        .ok_or_else(|| s3_err("InvalidRequest", "malformed chunk-signature extension"))?;
    if !k.eq_ignore_ascii_case("chunk-signature") {
        return Err(s3_err(
            "InvalidRequest",
            format!("unsupported chunk extension '{k}'"),
        ));
    }
    let v = v.trim().to_ascii_lowercase();
    if v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err(
            "InvalidRequest",
            "chunk-signature must be 64 hex characters",
        ));
    }
    Ok(Some(v))
}

fn parse_hex_size(s: &str) -> ApiResult<usize> {
    let s = s.trim();
    if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err("InvalidRequest", "invalid aws-chunked chunk size"));
    }
    usize::from_str_radix(s, 16)
        .map_err(|_| s3_err("InvalidRequest", "invalid aws-chunked chunk size"))
}

/// Helper for tests: frame plain bytes as signed aws-chunked.
pub fn frame_signed_chunks(
    data: &[u8],
    chunk_size: usize,
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    seed_signature: &str,
) -> (Vec<u8>, String) {
    let mut out = Vec::new();
    let mut prev = seed_signature.to_ascii_lowercase();
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        let sig = chunk_signature(signing_key, amz_date, scope, &prev, chunk);
        out.extend_from_slice(format!("{:x};chunk-signature={sig}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        prev = sig;
        offset = end;
    }
    let sig0 = chunk_signature(signing_key, amz_date, scope, &prev, b"");
    out.extend_from_slice(format!("0;chunk-signature={sig0}\r\n").as_bytes());
    out.extend_from_slice(b"\r\n");
    (out, sig0)
}

/// Frame unsigned chunks + trailing checksum.
pub fn frame_unsigned_trailer(
    data: &[u8],
    chunk_size: usize,
    checksum: TrailerChecksum,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut state = ChecksumState::new(checksum);
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        state.update(chunk);
        out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        offset = end;
    }
    let b64 = state.finalize_b64();
    out.extend_from_slice(b"0\r\n");
    out.extend_from_slice(format!("{}:{b64}\r\n\r\n", checksum.header_name()).as_bytes());
    out
}

/// Frame signed chunks + trailing checksum + trailer signature.
pub fn frame_signed_trailer(
    data: &[u8],
    chunk_size: usize,
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    seed_signature: &str,
    checksum: TrailerChecksum,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = seed_signature.to_ascii_lowercase();
    let mut offset = 0;
    let mut state = ChecksumState::new(checksum);
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        state.update(chunk);
        let sig = chunk_signature(signing_key, amz_date, scope, &prev, chunk);
        out.extend_from_slice(format!("{:x};chunk-signature={sig}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        prev = sig;
        offset = end;
    }
    let sig0 = chunk_signature(signing_key, amz_date, scope, &prev, b"");
    out.extend_from_slice(format!("0;chunk-signature={sig0}\r\n").as_bytes());
    prev = sig0;
    let b64 = state.finalize_b64();
    let header_line = format!("{}:{b64}\n", checksum.header_name());
    let tsig = trailer_signature(signing_key, amz_date, scope, &prev, &header_line);
    out.extend_from_slice(format!("{}:{b64}\r\n", checksum.header_name()).as_bytes());
    out.extend_from_slice(format!("x-amz-trailer-signature:{tsig}\r\n\r\n").as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sigv4::derive_signing_key;
    use futures::StreamExt;

    /// AWS documented example (65_536 + 1_024 'a's).
    #[test]
    fn aws_documented_chunk_signatures() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "4f232c4386841ef735655705268965c44a0e4690baa4adea153f7db9fa80a0a9";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";

        let c1 = vec![b'a'; 65536];
        let sig1 = chunk_signature(&key, amz_date, scope, seed, &c1);
        assert_eq!(
            sig1,
            "ad80c730a21e5b8d04586a2213dd63b9a0e99e0e2307b0ade35a65485a288648"
        );

        let c2 = vec![b'a'; 1024];
        let sig2 = chunk_signature(&key, amz_date, scope, &sig1, &c2);
        assert_eq!(
            sig2,
            "0055627c9e194cb4542bae2aa5492e3c1575bbb81b612b7d234b86a503ef5497"
        );

        let sig0 = chunk_signature(&key, amz_date, scope, &sig2, b"");
        assert_eq!(
            sig0,
            "b6c6ea8a5354eaf15b3cb7646744f4275b71ea724fed81ceb9323e279d449df9"
        );
        assert!(ct_eq_hex(&sig0, &sig0));
        assert!(!ct_eq_hex(&sig0, &sig1));
    }

    #[tokio::test]
    async fn missing_final_zero_chunk_is_incomplete() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"hello";
        let sig = chunk_signature(&key, amz_date, scope, seed, data);
        let mut body = Vec::new();
        body.extend_from_slice(format!("{:x};chunk-signature={sig}\r\n", data.len()).as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
        // no zero chunk

        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: data.len() as u64,
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
        let mut decoded = decode_stream(stream, auth);
        let mut got = Vec::new();
        let mut err = None;
        while let Some(item) = decoded.next().await {
            match item {
                Ok(b) => got.extend_from_slice(&b),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert_eq!(&got[..], data);
        let e = err.expect("must error");
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("IncompleteBody")
        );
    }

    #[tokio::test]
    async fn decoded_length_too_short_rejected() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"hello-world";
        let (framed, _) = frame_signed_chunks(data, 64, &key, amz_date, scope, seed);
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: (data.len() as u64) + 5, // too long declared
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(framed)) });
        let mut decoded = decode_stream(stream, auth);
        let mut err = None;
        while let Some(item) = decoded.next().await {
            if let Err(e) = item {
                err = Some(e);
                break;
            }
        }
        let e = err.expect("must error");
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("IncompleteBody")
        );
    }

    #[tokio::test]
    async fn decoded_length_too_long_rejected() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"hello-world-extra";
        let (framed, _) = frame_signed_chunks(data, 64, &key, amz_date, scope, seed);
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: 4, // too short declared
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(framed)) });
        let mut decoded = decode_stream(stream, auth);
        let mut err = None;
        while let Some(item) = decoded.next().await {
            if let Err(e) = item {
                err = Some(e);
                break;
            }
        }
        let e = err.expect("must error");
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("IncompleteBody")
        );
    }

    #[tokio::test]
    async fn bad_chunk_signature_rejects_before_yield() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"secret-bytes";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "{:x};chunk-signature={}\r\n",
                data.len(),
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: data.len() as u64,
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
        let mut decoded = decode_stream(stream, auth);
        let first = decoded.next().await.expect("item");
        let e = first.unwrap_err();
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("SignatureDoesNotMatch")
        );
        // No bytes should have been yielded.
        assert!(decoded.next().await.is_none() || decoded.next().await.is_none());
    }

    #[tokio::test]
    async fn chunk_over_max_rejected() {
        let auth = StreamingAuth {
            kind: StreamingKind::UnsignedTrailer {
                checksum: TrailerChecksum::Crc32,
            },
            seed_signature: String::new(),
            signing_key: Vec::new(),
            amz_date: "20130524T000000Z".into(),
            scope: "20130524/us-east-1/s3/aws4_request".into(),
            decoded_content_length: 0,
        };
        let huge = format!("{:x}\r\n", MAX_CHUNK_BYTES + 1);
        let stream =
            futures::stream::once(async move { Ok::<_, String>(Bytes::from(huge.into_bytes())) });
        let mut decoded = decode_stream(stream, auth);
        let e = decoded.next().await.unwrap().unwrap_err();
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("InvalidRequest")
        );
    }

    #[tokio::test]
    async fn oversize_header_line_rejected() {
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: "aa".repeat(32),
            signing_key: vec![0; 32],
            amz_date: "20130524T000000Z".into(),
            scope: "20130524/us-east-1/s3/aws4_request".into(),
            decoded_content_length: 0,
        };
        let line = "a".repeat(MAX_CHUNK_HEADER_BYTES + 1);
        let stream =
            futures::stream::once(async move { Ok::<_, String>(Bytes::from(line.into_bytes())) });
        let mut decoded = decode_stream(stream, auth);
        let e = decoded.next().await.unwrap().unwrap_err();
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("InvalidRequest")
        );
    }

    #[tokio::test]
    async fn oversize_trailer_rejected() {
        let auth = StreamingAuth {
            kind: StreamingKind::UnsignedTrailer {
                checksum: TrailerChecksum::Crc32,
            },
            seed_signature: String::new(),
            signing_key: Vec::new(),
            amz_date: "20130524T000000Z".into(),
            scope: "20130524/us-east-1/s3/aws4_request".into(),
            decoded_content_length: 0,
        };
        // Final zero chunk then a huge trailer without terminator.
        let mut body = b"0\r\n".to_vec();
        body.extend(std::iter::repeat_n(b'x', MAX_TRAILER_BYTES + 1));
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
        let mut decoded = decode_stream(stream, auth);
        let e = decoded.next().await.unwrap().unwrap_err();
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("InvalidRequest")
        );
    }

    #[tokio::test]
    async fn trailer_split_across_reads_ok() {
        let payload = b"split-trailer";
        let framed = frame_unsigned_trailer(payload, 4, TrailerChecksum::Crc32);
        // Deliver one byte at a time.
        let pieces: Vec<Result<Bytes, String>> =
            framed.iter().map(|&b| Ok(Bytes::from(vec![b]))).collect();
        let auth = StreamingAuth {
            kind: StreamingKind::UnsignedTrailer {
                checksum: TrailerChecksum::Crc32,
            },
            seed_signature: String::new(),
            signing_key: Vec::new(),
            amz_date: "20130524T000000Z".into(),
            scope: "20130524/us-east-1/s3/aws4_request".into(),
            decoded_content_length: payload.len() as u64,
        };
        let mut decoded = decode_stream(futures::stream::iter(pieces), auth);
        let mut got = Vec::new();
        while let Some(item) = decoded.next().await {
            got.extend_from_slice(&item.expect("ok"));
        }
        assert_eq!(&got[..], payload);
    }

    #[tokio::test]
    async fn trailing_garbage_after_final_chunk_rejected() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"ok";
        let (mut framed, _) = frame_signed_chunks(data, 64, &key, amz_date, scope, seed);
        framed.extend_from_slice(b"GARBAGE");
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: data.len() as u64,
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(framed)) });
        let mut decoded = decode_stream(stream, auth);
        let mut err = None;
        while let Some(item) = decoded.next().await {
            if let Err(e) = item {
                err = Some(e);
                break;
            }
        }
        let e = err.expect("must error");
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("IncompleteBody")
        );
    }

    #[tokio::test]
    async fn bad_signature_after_good_chunk() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let c1 = b"good-chunk-data!!";
        let c2 = b"bad-next";
        let sig1 = chunk_signature(&key, amz_date, scope, seed, c1);
        let mut body = Vec::new();
        body.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
        body.extend_from_slice(c1);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(
            format!(
                "{:x};chunk-signature={}\r\n",
                c2.len(),
                "0000000000000000000000000000000000000000000000000000000000000000"
            )
            .as_bytes(),
        );
        body.extend_from_slice(c2);
        body.extend_from_slice(b"\r\n");
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: (c1.len() + c2.len()) as u64,
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
        let mut decoded = decode_stream(stream, auth);
        let first = decoded.next().await.expect("good chunk").expect("ok");
        assert_eq!(&first[..], c1);
        let second = decoded.next().await.expect("item");
        assert_eq!(
            second
                .unwrap_err()
                .details
                .get("s3_code")
                .and_then(|v| v.as_str()),
            Some("SignatureDoesNotMatch")
        );
    }

    #[tokio::test]
    async fn truncated_body_incomplete() {
        let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
        let seed = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let amz_date = "20130524T000000Z";
        let scope = "20130524/us-east-1/s3/aws4_request";
        let data = b"truncate-me";
        let sig = chunk_signature(&key, amz_date, scope, seed, data);
        // Header only — no data, no final chunk.
        let body = format!("{:x};chunk-signature={sig}\r\n", data.len()).into_bytes();
        let auth = StreamingAuth {
            kind: StreamingKind::Signed,
            seed_signature: seed.into(),
            signing_key: key,
            amz_date: amz_date.into(),
            scope: scope.into(),
            decoded_content_length: data.len() as u64,
        };
        let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
        let mut decoded = decode_stream(stream, auth);
        let e = decoded.next().await.unwrap().unwrap_err();
        assert_eq!(
            e.details.get("s3_code").and_then(|v| v.as_str()),
            Some("IncompleteBody")
        );
    }
}

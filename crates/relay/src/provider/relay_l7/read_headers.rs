// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The request head off the wire up to its blank line, bounded by size and
//! by a deadline.

use super::find_header_end::find_header_end;
use tokio::io::AsyncReadExt;

pub(super) async fn read_headers<R>(stream: &mut R) -> Option<(Vec<u8>, Vec<u8>)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if buf.len() > 65_536 {
            return None;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let n = match tokio::time::timeout(left, stream.read(&mut tmp)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = find_header_end(&buf) {
            let rest = buf.split_off(end);
            return Some((buf, rest));
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::Engine as _;
use pistack_types::ApiResult;

use super::b64::B64;
use super::bad::bad;
use super::fingerprint::fingerprint;
use super::parse_blob::parse_blob;
use super::public_key::PublicKey;

/// Read one OpenSSH public key line, the `<type> <base64> [comment]` form
/// `authorized_keys` uses. A key with no comment is refused: every real one
/// carries one, and requiring it turns a line cut short into an error
/// instead of a key nobody can name later.
pub fn parse(line: &str) -> ApiResult<PublicKey> {
    if line.lines().count() > 1 {
        return Err(bad("public key must be one line"));
    }
    // A quoted `authorized_keys` option list comes before the key.
    let line = match line.trim() {
        quoted if quoted.starts_with('"') => quoted
            .strip_prefix('"')
            .and_then(|r| r.split_once('"'))
            .map(|(_, after)| after.trim_start())
            .ok_or_else(|| bad("public key option list is not closed"))?,
        other => other,
    };
    let (prefix, rest) = line
        .split_once(char::is_whitespace)
        .ok_or_else(|| bad("public key must be `type base64 [comment]` on one line"))?;
    let rest = rest.trim_start();
    let (b64, comment) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    if comment.is_empty() {
        return Err(bad(
            "public key must be `type base64 [comment]` on one line",
        ));
    }
    if line.chars().any(char::is_control) {
        return Err(bad("public key must not contain control characters"));
    }
    // Padding is optional on the wire, so a length that is not a multiple of
    // four is only valid without it.
    if b64.len() % 4 != 0 && b64.contains('=') {
        return Err(bad("public key is not valid base64"));
    }
    let blob = B64
        .decode(b64)
        .map_err(|_| bad("public key is not valid base64"))?;

    let key_type = parse_blob(&blob)?;
    if prefix != key_type {
        return Err(bad("public key type does not match the key it carries"));
    }
    Ok(PublicKey {
        key_type: key_type.clone(),
        line: format!("{key_type} {b64} {comment}"),
        fingerprint: fingerprint(&blob),
    })
}

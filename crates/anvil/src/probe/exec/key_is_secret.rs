// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn key_is_secret(head: &str) -> bool {
    let token = head
        .rsplit(|c: char| c.is_ascii_whitespace() || matches!(c, ',' | ';' | '(' | '{'))
        .next()
        .unwrap_or("");
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if token.is_empty() {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "api_key", "apikey"]
        .iter()
        .any(|key| lower.contains(key))
        || lower.contains("authorization")
}

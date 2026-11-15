// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::exec::key_is_secret::key_is_secret;

pub(in crate::probe) fn redact_probe_secrets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(rel) = rest.find(['=', ':']) {
        let (head, tail) = rest.split_at(rel);
        let sep = tail.chars().next().unwrap();
        let after = &tail[sep.len_utf8()..];
        out.push_str(head);
        out.push(sep);
        if key_is_secret(head) {
            if head.to_ascii_lowercase().contains("authorization") {
                out.push_str(" ***");
                return out;
            }
            let spaces = after.len() - after.trim_start_matches(' ').len();
            out.push_str(&after[..spaces]);
            out.push_str("***");
            let value = &after[spaces..];
            let skip = value
                .find(|c: char| c.is_whitespace())
                .unwrap_or(value.len());
            rest = &value[skip..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

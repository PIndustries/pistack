// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn parse_pax(data: &[u8]) -> (Option<String>, Option<String>) {
    let mut path = None;
    let mut link = None;
    let mut i = 0;
    while i < data.len() {
        let rest = &data[i..];
        let Some(sp) = rest.iter().position(|b| *b == b' ') else {
            break;
        };
        let Ok(len) = std::str::from_utf8(&rest[..sp])
            .unwrap_or("")
            .parse::<usize>()
        else {
            break;
        };
        if len <= sp || i + len > data.len() {
            break;
        }
        let rec = &data[i + sp + 1..i + len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Ok(text) = std::str::from_utf8(rec) {
            if let Some((k, v)) = text.split_once('=') {
                if k == "path" {
                    path = Some(v.to_owned());
                } else if k == "linkpath" {
                    link = Some(v.to_owned());
                }
            }
        }
        i += len;
    }
    (path, link)
}

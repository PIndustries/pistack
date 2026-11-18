// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::net::{Ipv4Addr, Ipv6Addr};

use super::normalize_name::normalize_name;
use super::record_type::RecordType;

/// Check one RDATA string for `kind`; returns its canonical form.
pub fn validate_rdata(kind: RecordType, raw: &str) -> ApiResult<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(ApiError::bad_request("record data must not be empty"));
    }
    let bad = |m: String| ApiError::bad_request(format!("{kind} record {raw:?}: {m}"));
    match kind {
        RecordType::A => s
            .parse::<Ipv4Addr>()
            .map(|a| a.to_string())
            .map_err(|_| bad("not an IPv4 address".into())),
        RecordType::Aaaa => s
            .parse::<Ipv6Addr>()
            .map(|a| a.to_string())
            .map_err(|_| bad("not an IPv6 address".into())),
        RecordType::Cname | RecordType::Ns | RecordType::Ptr => {
            if !s.ends_with('.') {
                return Err(bad(
                    "target must be an absolute name with a trailing dot".into()
                ));
            }
            normalize_name(s).map_err(|e| bad(e.message))
        }
        RecordType::Mx => {
            let (pref, host) = s
                .split_once(char::is_whitespace)
                .ok_or_else(|| bad("expected '<preference> <exchange.>'".into()))?;
            let pref: u16 = pref
                .parse()
                .map_err(|_| bad("preference must be 0-65535".into()))?;
            let host = host.trim();
            if !host.ends_with('.') {
                return Err(bad("exchange must be an absolute name".into()));
            }
            let host = normalize_name(host).map_err(|e| bad(e.message))?;
            Ok(format!("{pref} {host}"))
        }
        RecordType::Srv => {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() != 4 {
                return Err(bad("expected '<priority> <weight> <port> <target.>'".into()));
            }
            let mut nums = [0u16; 3];
            for (i, p) in parts.iter().take(3).enumerate() {
                nums[i] = p.parse().map_err(|_| bad(format!("{p} must be 0-65535")))?;
            }
            if !parts[3].ends_with('.') {
                return Err(bad("target must be an absolute name".into()));
            }
            let target = normalize_name(parts[3]).map_err(|e| bad(e.message))?;
            Ok(format!("{} {} {} {target}", nums[0], nums[1], nums[2]))
        }
        RecordType::Txt => {
            // Accept bare or quoted text; store quoted so the zone-file
            // export is unambiguous. Each character string is 255 bytes max.
            let text = if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                &s[1..s.len() - 1]
            } else {
                s
            };
            if text.len() > 255 {
                return Err(bad("text longer than 255 bytes".into()));
            }
            Ok(format!("\"{}\"", text.replace('"', "\\\"")))
        }
    }
}

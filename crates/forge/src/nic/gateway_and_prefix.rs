// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Subnet;
use pistack_types::{ApiError, ApiResult};
use std::net::Ipv4Addr;

pub(crate) fn gateway_and_prefix(sub: &Subnet) -> ApiResult<(String, u8)> {
    let cidr = sub.spec.cidr.to_string();
    let (addr, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| ApiError::bad_request(format!("subnet {} has no prefix", sub.meta.id)))?;
    if addr.contains(':') {
        return Err(ApiError::bad_request(format!(
            "subnet {} is not ipv4",
            sub.meta.id
        )));
    }
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| ApiError::bad_request(format!("subnet {} has a bad prefix", sub.meta.id)))?;
    let gateway = match sub.spec.gateway {
        Some(ip) if ip.is_ipv4() => ip.to_string(),
        Some(_) => {
            return Err(ApiError::bad_request(format!(
                "subnet {} gateway is not ipv4",
                sub.meta.id
            )))
        }
        None => {
            let parsed: Ipv4Addr = addr.parse().map_err(|_| {
                ApiError::bad_request(format!("subnet {} is not ipv4", sub.meta.id))
            })?;
            Ipv4Addr::from(u32::from(parsed).saturating_add(1)).to_string()
        }
    };
    Ok((gateway, prefix))
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Subnet, Vpc};
use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult};
use std::str::FromStr;

use super::cidr::Cidr;
use super::next_block::next_block;

/// One cidr from a short-road request: a block as it stands, or the next free
/// block of the prefix asked for.
pub(crate) fn carve(vpc: &Vpc, taken: &[Subnet], raw: &str) -> ApiResult<IpNet> {
    let used: Vec<IpNet> = taken.iter().map(|s| s.spec.cidr).collect();
    match Cidr::from_str(raw).map_err(ApiError::bad_request)? {
        Cidr::Block(b) => Ok(b),
        Cidr::Prefix(p) => next_block(&vpc.spec.cidrs, &used, p),
    }
}

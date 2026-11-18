// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::AllowedAddressPair;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;

use super::format_mac::format_mac;
use super::max_allowed_pairs::MAX_ALLOWED_PAIRS;
use super::pair_net::pair_net;
use super::parse_mac::parse_mac;

/// Check a port's allowed address pairs: at most
/// [`MAX_ALLOWED_PAIRS`] of them, each holding an address or a prefix that
/// is not the whole family, and each with a MAC that parses when it names
/// one. Two pairs of the same address and MAC are the same pair, so the
/// second is refused rather than quietly merged.
pub fn validate_pairs(pairs: &[AllowedAddressPair]) -> ApiResult<()> {
    if pairs.len() > MAX_ALLOWED_PAIRS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_ALLOWED_PAIRS} allowed address pairs per port"
        )));
    }
    let mut seen = BTreeSet::new();
    for p in pairs {
        if p.ip_address.prefix_len() == 0 {
            return Err(ApiError::bad_request(format!(
                "{} covers every address of its family",
                p.ip_address
            )));
        }
        let mac = match &p.mac_address {
            Some(m) => format_mac(parse_mac(m)?),
            None => String::new(),
        };
        if !seen.insert((pair_net(p.ip_address), mac.clone())) {
            return Err(ApiError::bad_request(format!(
                "allowed address pair {} with mac {} is listed twice",
                p.ip_address,
                if mac.is_empty() { "the port mac" } else { &mac }
            )));
        }
    }
    Ok(())
}

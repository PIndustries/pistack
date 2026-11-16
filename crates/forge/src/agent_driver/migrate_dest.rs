// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::usable_underlay;
use crate::types::Host;
use pistack_types::{ApiError, ApiResult};
/// `qemu+ssh://<underlay>/system` for a live-migration destination.
/// A missing or non-private underlay is an error. There is no path that
/// migrates without one, even when the machine names no network.
pub fn migrate_dest(host: &Host) -> ApiResult<String> {
    let ip = usable_underlay(host.spec.underlay_ip.as_deref()).ok_or_else(|| {
        ApiError::unprocessable(format!(
            "host {} has no usable private underlay",
            host.meta.name
        ))
    })?;
    Ok(format!("qemu+ssh://{ip}/system"))
}

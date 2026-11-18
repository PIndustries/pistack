// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};

use super::fabric_client::fabric_client;

/// Drop ports plugged into `device`. No port is success.
pub fn release_ports(store: &Store, device: &Id) -> ApiResult<()> {
    fabric_client(store)
        .release_device_ports(device)
        .map(|_| ())
}

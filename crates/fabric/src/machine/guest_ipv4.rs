// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id};

use super::first_ipv4::first_ipv4;
use super::guest_address::GuestAddress;
use super::machine_view::MachineView;

/// First IPv4 in `status.addresses`. A trailing prefix (`10.88.0.187/24`) is stripped.
pub(crate) fn guest_ipv4(store: &Store, machine: &Id) -> ApiResult<GuestAddress> {
    if machine.kind != kind::MACHINE {
        return Err(ApiError::bad_request(format!("{machine} is not a machine")));
    }
    let placed = store.collection::<MachineView>().get(machine)?;
    let ipv4 = first_ipv4(&placed.status.addresses)
        .ok_or_else(|| ApiError::unprocessable(format!("machine {machine} has no ipv4 address")))?;
    Ok(GuestAddress {
        meta: placed.meta,
        ipv4,
    })
}

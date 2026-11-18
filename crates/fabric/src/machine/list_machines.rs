// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::ApiResult;

use super::fip_condition::FIP_CONDITION;
use super::first_ipv4::first_ipv4;
use super::listed_machine::ListedMachine;
use super::machine_view::MachineView;

pub(crate) fn list_machines(store: &Store) -> ApiResult<Vec<ListedMachine>> {
    let rows = store.collection::<MachineView>().list(None)?;
    Ok(rows
        .into_iter()
        .map(|placed| ListedMachine {
            ipv4: first_ipv4(&placed.status.addresses),
            noted: placed
                .status
                .conditions
                .into_iter()
                .find(|c| c.kind == FIP_CONDITION)
                .map(|c| (c.reason, c.message)),
            meta: placed.meta,
        })
        .collect())
}

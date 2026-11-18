// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_store::Collection;

use super::app_state::AppState;

pub(crate) fn machines(s: &AppState) -> Collection<Machine> {
    s.store.collection::<Machine>()
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_store::Collection;

use super::app_state::AppState;

pub(crate) fn hosts(s: &AppState) -> Collection<Host> {
    s.store.collection::<Host>()
}

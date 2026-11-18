// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Size;
use pistack_store::Collection;

use super::app_state::AppState;

pub(crate) fn sizes(s: &AppState) -> Collection<Size> {
    s.store.collection::<Size>()
}

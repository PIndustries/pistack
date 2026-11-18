// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_store::Collection;

use super::reconciler::Reconciler;

impl Reconciler {
    pub(super) fn hosts(&self) -> Collection<Host> {
        self.store.collection::<Host>()
    }
}

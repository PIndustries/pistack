// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::{consts::TERM_KEY, ovn_backend::OvnBackend};

impl OvnBackend {
    pub(in crate::backend::ovn) fn term(&self) -> u64 {
        self.attached
            .get()
            .and_then(|a| a.store.get_meta::<u64>(TERM_KEY).ok().flatten())
            .unwrap_or(0)
    }
}

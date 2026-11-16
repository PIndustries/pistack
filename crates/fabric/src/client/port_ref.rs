// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::Id;
/// What the reconciler remembers about a port after it has gone.
#[derive(Clone, Debug)]
pub(crate) struct PortRef {
    pub subnet: Option<Id>,
    pub groups: Vec<Id>,
}

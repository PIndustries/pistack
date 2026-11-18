// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self};
use pistack_store::Store;
use pistack_types::Id;

use super::mirror::mirror;

/// Record the project's usage after a machine's delete was accepted.
pub fn note_delete(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    mirror(store, project);
}

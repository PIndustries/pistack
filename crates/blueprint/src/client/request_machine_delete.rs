// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::{Store, StoreError};
use pistack_types::{ApiResult, Id};

/// Mark a machine for deletion the same way forge's DELETE does.
pub(crate) fn request_machine_delete(store: &Store, id: &Id) -> ApiResult<()> {
    match store.collection::<forge::Machine>().modify(id, |m| {
        m.meta
            .annotations
            .entry(forge::DELETE_ANNOTATION.into())
            .or_insert_with(|| chrono::Utc::now().to_rfc3339());
        Ok(())
    }) {
        Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

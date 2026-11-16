// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::release_volume::release_volume;
use pistack_types::Id;

pub(in crate::runtime::volfs) async fn rollback(volume: &Id) {
    if let Err(err) = release_volume(volume).await {
        tracing::warn!(
            volume = %volume,
            error = %err,
            "anvil: rolling back a slab mount failed"
        );
    }
}

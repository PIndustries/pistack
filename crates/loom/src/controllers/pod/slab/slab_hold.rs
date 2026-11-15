// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::SlabBind;
pub(in crate::controllers::pod) enum SlabHold {
    /// The pod names no slab volume.
    None,
    /// Do not create or start. The message names the volume.
    Wait(String),
    Ready(SlabBind),
}

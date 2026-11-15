// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::Id;
pub(in crate::controllers::pod) struct PublishedVol {
    pub(in crate::controllers::pod) id: String,
    pub(in crate::controllers::pod) name: String,
    pub(in crate::controllers::pod) path: Option<String>,
    pub(in crate::controllers::pod) device: Option<String>,
    pub(in crate::controllers::pod) node: Option<Id>,
    pub(in crate::controllers::pod) attached_here: bool,
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(crate) struct Guest {
    pub(crate) name: String,
    pub(crate) local: String,
    pub(crate) public: String,
    pub(crate) key: Option<String>,
    pub(crate) user_data: String,
    pub(crate) project: Option<Id>,
    pub(crate) secrets: Vec<String>,
}

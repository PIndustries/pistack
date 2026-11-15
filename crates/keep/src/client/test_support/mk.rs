// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::Id;

pub(in crate::client) fn mk(name: &str, project: &Id) -> Secret {
    Secret::new(name, Some(project.clone()), SecretSpec::default())
}

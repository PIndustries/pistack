// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::permission_denied;
use crate::runtime::youki::bundles::privileged_remove;
use crate::runtime::youki::bundles::remove_err;
use crate::runtime::youki::bundles::remove_unprivileged;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn delete_path(path: &Path, privilege: &Privilege) -> ApiResult<()> {
    match remove_unprivileged(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if permission_denied(&e) && matches!(privilege, Privilege::Sudo { .. }) => {
            privileged_remove(path, privilege, &e)
        }
        Err(e) => Err(remove_err(path, &e)),
    }
}

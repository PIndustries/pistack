// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::remove_bundle;
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::privilege::RunMode;
use std::path::Path;

pub(in crate::runtime::youki) async fn forget_failed_create(
    bin: &Path,
    mode: &RunMode,
    id: &str,
    bundle: Option<&Path>,
) {
    let _ = exec_youki(bin, mode, &["delete", "--force", id], None).await;
    if let Some(bundle) = bundle {
        let _ = remove_bundle(bundle, &mode.privilege).await;
    }
}

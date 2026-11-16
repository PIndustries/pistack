// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::remove_bundle;
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::privilege::RunMode;
use std::path::Path;

pub(in crate::runtime::youki) async fn abort_new_pod(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    sandbox_id: &str,
    created: &[String],
) {
    for cid in created.iter().rev() {
        let _ = exec_youki(bin, mode, &["delete", "--force", cid], None).await;
    }
    let _ = exec_youki(bin, mode, &["delete", "--force", sandbox_id], None).await;
    let _ = remove_bundle(bundle, &mode.privilege).await;
}

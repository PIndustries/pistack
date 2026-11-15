// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Drop containers from a create that did not finish. A first create also
/// removes the bundle. A restart keeps it: the volume records still live there.
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::lifecycle::abort_new_pod;
use crate::runtime::youki::privilege::RunMode;
use std::path::Path;

pub(in crate::runtime::youki) async fn release_failed_create(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    sandbox_id: &str,
    created: &[String],
    prior: bool,
) {
    if prior {
        for cid in created.iter().rev() {
            let _ = exec_youki(bin, mode, &["delete", "--force", cid], None).await;
        }
        let _ = exec_youki(bin, mode, &["delete", "--force", sandbox_id], None).await;
        return;
    }
    abort_new_pod(bin, mode, bundle, sandbox_id, created).await;
}

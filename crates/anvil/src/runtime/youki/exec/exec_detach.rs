// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::run_detach;
use crate::runtime::youki::lifecycle::DetachIo;
use crate::runtime::youki::privilege::RunMode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn exec_detach(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
    io: &DetachIo,
) -> ApiResult<()> {
    run_detach(bin, mode, args, join_user_ns, io)
        .await
        .map(|_| ())
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::run_youki;
use crate::runtime::youki::privilege::RunMode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn exec_youki(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
) -> ApiResult<()> {
    run_youki(bin, mode, args, join_user_ns).await.map(|_| ())
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::canonical_fs::canonical_fs;
use pistack_types::ApiResult;

/// `mkfs.<fs>` and the arguments that keep it quiet and non-interactive.
pub(in crate::runtime::volfs) fn mkfs_command(
    fs: &str,
    device: &str,
) -> ApiResult<(String, Vec<String>)> {
    let fs = canonical_fs(fs)?;
    let program = format!("mkfs.{fs}");
    let mut args = match fs.as_str() {
        "xfs" => vec!["-f".to_owned(), "-q".to_owned()],
        "btrfs" => vec!["-f".to_owned()],
        _ => vec!["-q".to_owned(), "-F".to_owned()],
    };
    args.push(device.to_owned());
    Ok((program, args))
}

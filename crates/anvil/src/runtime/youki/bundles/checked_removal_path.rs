// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Absolute path strictly inside [`state_dir`], with no `..` component.
/// Lexical only: a symlink is not resolved, so the check cannot be pointed
/// at another directory by canonicalizing it.
use crate::runtime::youki::bundles::normalize_lexical;
use crate::runtime::youki::bundles::removal_text_ok;
use crate::runtime::youki::state_dir::state_dir;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn checked_removal_path(path: &Path) -> Result<PathBuf, ()> {
    if !removal_text_ok(path) || !path.is_absolute() {
        return Err(());
    }
    let state = state_dir();
    if !removal_text_ok(&state) || !state.is_absolute() {
        return Err(());
    }
    let path = normalize_lexical(path);
    let state = normalize_lexical(&state);
    if path == state || !path.starts_with(&state) {
        return Err(());
    }
    Ok(path)
}

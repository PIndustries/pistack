// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Tests that point youki at a fake binary share this lock. `PISTACK_YOUKI_BIN`
/// and `PISTACK_YOUKI_STATE` are process-wide.
#[cfg(test)]
pub(crate) fn youki_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn env_lock() -> &'static tokio::sync::Mutex<()> {
    crate::runtime::youki::youki_env_lock()
}

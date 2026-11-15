// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::EnvGuard;
use crate::runtime::youki::consts::CLEANUP_ENV;
use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::consts::SUDO_BIN_ENV;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::consts::TIMEOUT_ENV;
use std::path::Path;

impl EnvGuard {
    /// Points youki at the test binary and forces the root OCI config.
    /// Sudo, netns, and user-namespace overrides from the developer
    /// environment are cleared for the duration of the test.
    pub(in crate::runtime) fn set(bin: &Path, state: &Path) -> Self {
        let keys = [
            "PISTACK_YOUKI_BIN",
            "PISTACK_YOUKI_STATE",
            TIMEOUT_ENV,
            SUDO_ENV,
            SUDO_BIN_ENV,
            CLEANUP_ENV,
            NETNS_ENV,
            "PISTACK_YOUKI_TEST_EUID",
            "PISTACK_YOUKI_TEST_EGID",
            "PISTACK_YOUKI_TEST_USERNS",
            "PISTACK_RBD",
            "PISTACK_RBD_SUDO",
            "PISTACK_RBD_SUDO_BIN",
            "PISTACK_RBD_TEST_EUID",
            "PISTACK_RBD_DEV_ROOT",
            "PISTACK_IP_BIN",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("PISTACK_YOUKI_BIN", bin);
        std::env::set_var("PISTACK_YOUKI_STATE", state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
        for key in [
            TIMEOUT_ENV,
            SUDO_ENV,
            SUDO_BIN_ENV,
            CLEANUP_ENV,
            NETNS_ENV,
            "PISTACK_YOUKI_TEST_EGID",
            "PISTACK_YOUKI_TEST_USERNS",
            "PISTACK_RBD",
            "PISTACK_RBD_SUDO",
            "PISTACK_RBD_SUDO_BIN",
            "PISTACK_RBD_TEST_EUID",
            "PISTACK_RBD_DEV_ROOT",
            "PISTACK_IP_BIN",
        ] {
            std::env::remove_var(key);
        }
        Self { saved }
    }
}

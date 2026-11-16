// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::agent_is_root::agent_is_root;
use super::consts::{SUDOERS_SAMPLE, SUDO_ENV};
use super::env_flag::env_flag;

pub(in crate::runtime::volfs) fn privilege_hint() -> String {
    if agent_is_root() || env_flag(SUDO_ENV) {
        String::new()
    } else {
        format!(" Set {SUDO_ENV}=1 and allow `{SUDOERS_SAMPLE}`.")
    }
}

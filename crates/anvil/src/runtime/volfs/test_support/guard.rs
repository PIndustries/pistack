// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::volfs::consts::SUDO_ENV;
use std::path::Path;

pub(in crate::runtime::volfs) struct EnvGuard {
    pub(in crate::runtime::volfs) saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    pub(in crate::runtime::volfs) fn set(pairs: &[(&'static str, &Path)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in pairs {
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

pub(in crate::runtime::volfs) struct EuidGuard(
    pub(in crate::runtime::volfs) Option<std::ffi::OsString>,
);

impl EuidGuard {
    pub(in crate::runtime::volfs) fn root() -> Self {
        let saved = std::env::var_os("PISTACK_VOLFS_TEST_EUID");
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
        std::env::remove_var(SUDO_ENV);
        Self(saved)
    }
}

impl Drop for EuidGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("PISTACK_VOLFS_TEST_EUID", value),
            None => std::env::remove_var("PISTACK_VOLFS_TEST_EUID"),
        }
    }
}

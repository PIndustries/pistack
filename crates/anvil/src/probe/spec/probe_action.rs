// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeAction {
    Http {
        path: String,
        port: u16,
        host: Option<String>,
        https: bool,
    },
    Tcp {
        port: u16,
    },
    Exec {
        command: Vec<String>,
    },
    Missing,
}

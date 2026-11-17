// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ModuleInfo`: the static description a module reports.

/// Static description of a module.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Workshop name: "gate", "forge", "fabric", ...
    pub name: &'static str,
    /// URL prefix the gateway mounts it on, e.g. "identity", "machines".
    /// Plain nouns here on purpose: `pi machine create` -> POST /machines.
    pub prefix: &'static str,
    /// One line, shown by `GET /` discovery and `pi modules`.
    pub summary: &'static str,
    pub version: &'static str,
}

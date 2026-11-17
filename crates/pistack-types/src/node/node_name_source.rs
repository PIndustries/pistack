// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `NodeNameSource`: where a resolved node name came from.

use std::fmt;

/// Where a resolved node name came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeNameSource {
    Env,
    File,
    Adopted,
    Hostname,
    Minted,
}

impl NodeNameSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeNameSource::Env => "env",
            NodeNameSource::File => "file",
            NodeNameSource::Adopted => "adopted",
            NodeNameSource::Hostname => "hostname",
            NodeNameSource::Minted => "minted",
        }
    }
}

impl fmt::Display for NodeNameSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `PolicyError`: why a policy document was rejected.

use super::super::expr::ParseError;

/// Why a policy document was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("policy document is not valid YAML/JSON: {0}")]
    Document(String),
    #[error("rule '{name}': {source}")]
    RuleSyntax { name: String, source: ParseError },
    #[error("action '{action}': {source}")]
    ActionSyntax { action: String, source: ParseError },
    #[error("{referrer} references unknown rule '{rule}'")]
    UnknownRule { referrer: String, rule: String },
    #[error("rule reference cycle: {}", path.join(" -> "))]
    Cycle { path: Vec<String> },
}

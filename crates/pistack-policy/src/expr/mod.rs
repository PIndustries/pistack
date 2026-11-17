// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The rule expression language: AST, lexer and a small recursive-descent
//! parser. No external parser crate; the grammar is tiny:
//!
//! ```text
//! expr    := or
//! or      := and ( "or" and )*
//! and     := not ( "and" not )*
//! not     := "not" not | primary
//! primary := "(" expr ")"
//!          | "true" | "false"
//!          | "system"          # caller holds a system-scoped token
//!          | "project_member"  # caller.project == target.project
//!          | "owner"           # caller.principal == target.owner
//!          | "role:" NAME      # caller has the named role
//!          | "rule:" NAME      # reference to a named rule in the document
//! NAME    := [A-Za-z0-9_.-]+
//! ```
//!
//! Keywords are case-sensitive and lower-case. Any other bare word is an
//! error; the message names the word and its column so a typo in a policy
//! file is found quickly.

mod ast;
mod parse_error;
mod parser;
#[cfg(test)]
mod tests;

pub use ast::Expr;
pub use parse_error::ParseError;
pub use parser::parse;

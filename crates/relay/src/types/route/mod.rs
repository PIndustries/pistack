// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The HTTP route a listener carries: one host/path rule and how its path
//! is compared with the request's.

mod path_match;
mod rule;

pub use path_match::PathMatch;
pub use rule::Route;

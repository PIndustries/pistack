// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `upstream` argument: which `http://` addresses a door may be told
//! to publish.

mod allowed_upstream;
mod check_upstream;

pub(crate) use check_upstream::check_upstream;

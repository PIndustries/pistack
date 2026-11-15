// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counting a project's rows, refusing one more at its limit, and
//! mirroring the count into the project's usage.

pub(in crate::client) mod admit;
pub(in crate::client) mod count;
pub(in crate::client) mod note_usage;

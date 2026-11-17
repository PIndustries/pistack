// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Creating the `pw` WireGuard interface before `wg set` runs against it.
//! Only creates; never deletes, never names `wg-fleet`.

mod ip_bin;
mod try_create_pw;

pub(crate) use try_create_pw::try_create_pw;

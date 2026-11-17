// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The WireGuard interface name a door may use, and the argv guard that
//! keeps it to that name. One file per rule.

mod device_hex;
mod is_pw;
mod pw_iface;
mod refuse_wg_args;

pub(super) use is_pw::is_pw;
pub(super) use pw_iface::pw_iface;
pub(super) use refuse_wg_args::refuse_wg_args;

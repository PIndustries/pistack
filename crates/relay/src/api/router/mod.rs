// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The routes, and the three kinds that get them from [`crud`]. Members are
//! wired by hand because a member is reached through its pool.

mod by_balancer;
mod routes;

pub use routes::router;

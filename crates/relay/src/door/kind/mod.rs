// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What kind of publisher a listener has, and what an edge door is allowed
//! to read.

mod door_kind;
mod door_side;

pub use door_kind::DoorKind;
pub use door_side::DoorSide;

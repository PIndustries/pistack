// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ACCEPTED_MESSAGE`: status text stored when a stack or kit is created.
/// Status text stored when a stack or kit is created. Apply replaces it
/// with the ids it inserted, updated, or found already present.
pub const ACCEPTED_MESSAGE: &str =
    "accepted; apply creates network, subnet, machine, router, pod, listener, and record set rows";

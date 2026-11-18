// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// No tunnel endpoint.
    NoUnderlay,
    /// No report yet, or the report says the chassis is not working.
    NotReady,
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secondary zone transfer: pull a full AXFR from a primary over TCP and
//! compile it into a [`crate::server::ZoneData`] for authoritative answers.

mod parse_primary;
mod pull_axfr;
mod pull_from_listed_first;
mod pull_from_primaries;
mod pull_ordered;
#[cfg(test)]
mod tests;
mod xfer_timeout;
pub(crate) use pull_from_listed_first::pull_from_listed_first;
pub(crate) use pull_from_primaries::pull_from_primaries;

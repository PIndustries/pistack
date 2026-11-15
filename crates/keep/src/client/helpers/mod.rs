// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Checks and small conversions shared by more than one operation,
//! none of which needs a `Client`.

pub(in crate::client) mod check_payload;
pub(in crate::client) use check_payload::check_payload;
pub(in crate::client) mod check_metadata;
pub(in crate::client) use check_metadata::check_metadata;
pub(crate) mod check_meta_fields;
pub(crate) use check_meta_fields::check_meta_fields;
pub(crate) mod managed_role;
pub(crate) use managed_role::managed_role;
pub mod managed_cert_role;
pub use managed_cert_role::managed_cert_role;
pub mod agent_cert_expiry;
pub use agent_cert_expiry::agent_cert_expiry;
pub(in crate::client) mod sanitize;
pub(in crate::client) use sanitize::sanitize;

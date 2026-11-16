// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `keep::Client`: what the binary and other modules use. Everything the
//! HTTP layer does goes through here too, so the two never disagree.

pub use crate::types::{Certificate, CertificateAuthority, CertificateSpec};

mod authorities;
mod certificates;
mod consts;
mod debug_fmt;
mod handle;
pub(crate) mod helpers;
mod pki;
mod quota_gate;
mod secrets;

pub use consts::{
    MAX_CONTENT_TYPE, MAX_DESCRIPTION, MAX_LABELS, MAX_LABEL_KEY, MAX_LABEL_VALUE, MAX_PAYLOAD,
};
pub use handle::Client;
pub use helpers::{agent_cert_expiry, managed_cert_role};
pub(crate) use helpers::{check_meta_fields, managed_role};

#[cfg(test)]
mod test_support;

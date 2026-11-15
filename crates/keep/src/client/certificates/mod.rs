// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Certificate resources a caller asks for, and the secret that
//! holds each private key.

pub mod certificate_key_pem;
pub mod delete_certificate;
pub mod get_certificate;
pub mod issue_certificate;
pub mod issue_certificate_with;
pub mod list_certificates;
pub(in crate::client) mod persist_certificate;
pub mod revoke_certificate;
mod store_certificate;

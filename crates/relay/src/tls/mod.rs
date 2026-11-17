// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! TLS material for a terminated listener.
//!
//! The certificate and its private key are read from `keep`. This module
//! does not issue, renew, or generate a certificate. A missing, revoked, or
//! unreadable certificate is unavailable.
//!
//! One file per thing: [`material`] is the [`TlsMaterial`] the listener
//! holds, [`load_kept`] reads it from `keep`, [`existing_root`] the root key
//! that unseals its private key, and [`server_config`] turns the material
//! into the rustls config that presents it. Callers see [`TlsMaterial`],
//! [`load_kept`] and [`server_config`] here, as before.

mod existing_root;
mod load_kept;
mod material;
mod server_config;

pub use load_kept::load_kept;
pub use material::TlsMaterial;
pub use server_config::server_config;

#[cfg(test)]
mod tests;

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ca::{pem_to_der, serial_from_der};
use crate::tls::Pki;

pub(in crate::client) fn serial_of(pki: &Pki) -> u64 {
    serial_from_der(&pem_to_der(&pki.cert_pem).unwrap()).unwrap()
}

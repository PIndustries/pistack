// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::CertificateSpec;

pub(in crate::client) fn cert_spec(cn: &str) -> CertificateSpec {
    CertificateSpec {
        common_name: cn.into(),
        ..Default::default()
    }
}

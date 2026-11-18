// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Ingress;
use pistack_store::StoreError;
use pistack_types::{ApiError, ApiResult, Id};
use relay::Client;

/// The keep certificate an ingress's `tls` entries name, if any.
///
/// Each entry's `secret_name` is a keep certificate name (or id) in the
/// ingress's project. One listener presents one certificate, so every
/// entry must name the same one.
pub(crate) fn ingress_cert(client: &Client, ingress: &Ingress) -> ApiResult<Option<Id>> {
    let mut names: Vec<&str> = Vec::new();
    for tls in &ingress.spec.tls {
        let name = tls.secret_name.trim();
        if name.is_empty() {
            return Err(ApiError::unprocessable(
                "ingress tls entry needs a certificate name",
            ));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let name = match names.as_slice() {
        [] => return Ok(None),
        [one] => *one,
        many => {
            return Err(ApiError::unprocessable(format!(
                "ingress tls names {} certificates ({}); one certificate per ingress is supported",
                many.len(),
                many.join(", ")
            )))
        }
    };
    let project = ingress.meta.project.as_ref();
    let missing = || ApiError::unavailable(format!("ingress certificate '{name}' not found"));
    let cert = match client
        .store()
        .collection::<keep::Certificate>()
        .resolve(project, name)
    {
        Ok(cert) => cert,
        Err(StoreError::NotFound(_)) => return Err(missing()),
        Err(e) => return Err(e.into()),
    };
    if cert.meta.project.as_ref() != project {
        return Err(missing());
    }
    Ok(Some(cert.meta.id))
}

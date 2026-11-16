// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::check_addr::check_addr;
use super::check_path_part::check_path_part;
use super::consts::*;
use crate::error::Result;
use crate::vault::VaultTransit;

pub(in crate::backend) fn vault_from(get: &dyn Fn(&str) -> Option<String>) -> Result<VaultTransit> {
    let need =
        |k: &str| get(k).ok_or_else(|| backend_err(format!("{ENV_BACKEND}=vault needs {k}")));
    let addr = need(ENV_VAULT_ADDR)?.trim_end_matches('/').to_owned();
    check_addr(&addr)?;
    let token = match (get(ENV_VAULT_TOKEN), get(ENV_VAULT_TOKEN_FILE)) {
        (Some(_), Some(_)) => {
            return Err(backend_err(format!(
                "set only one of {ENV_VAULT_TOKEN} and {ENV_VAULT_TOKEN_FILE}"
            )))
        }
        (Some(t), None) => t,
        (None, Some(path)) => {
            let t = std::fs::read_to_string(&path)
                .map_err(|e| backend_err(format!("{ENV_VAULT_TOKEN_FILE} {path}: {e}")))?;
            let t = t.trim().to_owned();
            if t.is_empty() {
                return Err(backend_err(format!(
                    "{ENV_VAULT_TOKEN_FILE} {path} is empty"
                )));
            }
            t
        }
        (None, None) => {
            return Err(backend_err(format!(
                "{ENV_BACKEND}=vault needs {ENV_VAULT_TOKEN} or {ENV_VAULT_TOKEN_FILE}"
            )))
        }
    };
    let key = need(ENV_VAULT_KEY)?;
    check_path_part(ENV_VAULT_KEY, &key, false)?;
    let mount = get(ENV_VAULT_MOUNT).unwrap_or_else(|| "transit".into());
    let mount = mount.trim_matches('/').to_owned();
    check_path_part(ENV_VAULT_MOUNT, &mount, true)?;
    let ca_pem = match get(ENV_VAULT_CACERT) {
        Some(path) => Some(
            std::fs::read(&path)
                .map_err(|e| backend_err(format!("{ENV_VAULT_CACERT} {path}: {e}")))?,
        ),
        None => None,
    };
    Ok(VaultTransit {
        addr,
        token,
        mount,
        key,
        namespace: get(ENV_VAULT_NAMESPACE),
        ca_pem,
        timeout: VAULT_TIMEOUT,
    })
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::consts::*;
use crate::backend::test_support::*;
use crate::backend::KeyBackend;

#[test]
fn selection_defaults_to_software_and_refuses_the_rest() {
    assert!(matches!(parse(&[]).unwrap(), KeyBackend::Software));
    assert!(matches!(
        parse(&[(ENV_BACKEND, "software")]).unwrap(),
        KeyBackend::Software
    ));
    assert!(matches!(
        parse(&[(ENV_BACKEND, "  ")]).unwrap(),
        KeyBackend::Software
    ));
    assert!(err(&[(ENV_BACKEND, "pkcs11")]).contains("parked"));
    assert!(err(&[(ENV_BACKEND, "hsm")]).contains("not a root-key backend"));
    // Vault settings without selecting vault are a mistake, not ignored.
    assert!(err(&[(ENV_VAULT_ADDR, "https://vault:8200")]).contains(ENV_VAULT_ADDR));
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::consts::*;
use crate::backend::test_support::*;
use crate::backend::KeyBackend;

#[test]
fn token_file_is_read_and_must_not_be_empty() {
    let dir = std::env::temp_dir().join(format!("keep-vault-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let tok = dir.join("token");
    std::fs::write(&tok, "s.from-file\n").unwrap();
    let empty = dir.join("empty");
    std::fs::write(&empty, "\n").unwrap();
    let pairs = |path: &std::path::Path| {
        parse(&[
            (ENV_BACKEND, "vault"),
            (ENV_VAULT_ADDR, "https://vault:8200"),
            (ENV_VAULT_TOKEN_FILE, path.to_str().unwrap()),
            (ENV_VAULT_KEY, "k"),
        ])
    };
    let KeyBackend::VaultTransit(v) = pairs(&tok).unwrap() else {
        panic!("expected vault");
    };
    assert_eq!(v.token, "s.from-file");
    assert!(pairs(&empty).unwrap_err().to_string().contains("empty"));
    let _ = std::fs::remove_dir_all(&dir);
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#![allow(dead_code)]

use super::test_support::*;
use crate::backend::forget_unwrapped;
use crate::backend::*;
use pistack_store::Store;

/// Admin call for the real-Vault test (mounts, keys, rotation).
fn vault_admin(addr: &str, token: &str, method: &str, path: &str, body: serde_json::Value) {
    let (url, token, method) = (
        format!("{addr}/v1/{path}"),
        token.to_owned(),
        method.to_owned(),
    );
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let c = reqwest::Client::new();
            let req = match method.as_str() {
                "DELETE" => c.delete(&url),
                _ => c.post(&url).json(&body),
            };
            let resp = req.header("X-Vault-Token", token).send().await.unwrap();
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            assert!(status.is_success(), "{method} {url}: {status} {text}");
        })
    })
    .join()
    .unwrap();
}

/// Migration, rewrap and the lock-out path against a real Vault, e.g.
/// `vault server -dev`. Ignored by default and not run in CI:
///
/// ```text
/// PISTACK_TEST_VAULT_ADDR=http://127.0.0.1:8200 PISTACK_TEST_VAULT_TOKEN=root \
///   cargo test -p keep --lib real_vault -- --ignored
/// ```
///
/// It mounts a fresh Transit engine at a random path and removes it after.
#[test]
#[ignore = "needs a real Vault: set PISTACK_TEST_VAULT_ADDR and PISTACK_TEST_VAULT_TOKEN"]
fn real_vault_migrate_rewrap_and_back() {
    let addr = std::env::var("PISTACK_TEST_VAULT_ADDR").expect("PISTACK_TEST_VAULT_ADDR");
    let token = std::env::var("PISTACK_TEST_VAULT_TOKEN").expect("PISTACK_TEST_VAULT_TOKEN");
    let mount = format!(
        "pistack-it-{}",
        &uuid::Uuid::now_v7().simple().to_string()[24..]
    );
    let key = "pistack-root";
    vault_admin(
        &addr,
        &token,
        "POST",
        &format!("sys/mounts/{mount}"),
        serde_json::json!({"type": "transit"}),
    );
    let result = std::panic::catch_unwind(|| {
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}"),
            serde_json::json!({}),
        );
        let backend = parse(&[
            (ENV_BACKEND, "vault"),
            (ENV_VAULT_ADDR, &addr),
            (ENV_VAULT_TOKEN, &token),
            (ENV_VAULT_KEY, key),
            (ENV_VAULT_MOUNT, &mount),
        ])
        .unwrap();
        let KeyBackend::VaultTransit(v) = backend.clone() else {
            panic!()
        };
        let store = Store::in_memory().unwrap();
        let (soft, secret, ca) = software_cloud(&store);

        // Software -> Vault.
        let m = migrate_to_vault(&store, &v).unwrap();
        assert_eq!(m.key_version, Some(1));
        assert!(matches!(state(&store), RootKeyState::Wrapped(_)));
        reopen_and_check(&store, &backend, &secret, &ca);

        // Rotate, rewrap, retire v1: still opens.
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}/rotate"),
            serde_json::json!({}),
        );
        let r = rewrap(&store, &v).unwrap();
        assert_eq!(
            (r.from_version, r.to_version, r.changed),
            (Some(1), Some(2), true)
        );
        assert!(!rewrap(&store, &v).unwrap().changed);
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}/config"),
            serde_json::json!({"min_decryption_version": 2}),
        );
        reopen_and_check(&store, &backend, &secret, &ca);

        // Retire v2 before rewrapping: refused with the way out.
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}/rotate"),
            serde_json::json!({}),
        );
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}/config"),
            serde_json::json!({"min_decryption_version": 3}),
        );
        forget_unwrapped();
        let e = backend.load_existing(&store).unwrap_err().to_string();
        assert!(e.contains("Lower min_decryption_version to 2"), "{e}");
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("{mount}/keys/{key}/config"),
            serde_json::json!({"min_decryption_version": 2}),
        );
        assert!(rewrap(&store, &v).unwrap().changed);

        // Vault -> software gives back the original key.
        migrate_to_software(&store, &v).unwrap();
        let k = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
        assert_eq!(k.bytes(), soft.bytes());
        reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);
    });
    vault_admin(
        &addr,
        &token,
        "DELETE",
        &format!("sys/mounts/{mount}"),
        serde_json::json!({}),
    );
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

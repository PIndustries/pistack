// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::KeepError;
use crate::types::*;
use pistack_store::Store;

#[cfg(test)]
#[test]
fn client_round_trips_secrets_through_vault_transit() {
    use crate::backend::{forget_unwrapped, KeyBackend, META_ROOT_KEY_WRAPPED};
    use crate::crypto::META_ROOT_KEY;
    use crate::vault::mock::{MockVault, KEY, TOKEN};

    let vault = MockVault::start();
    let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
    let store = Store::in_memory().unwrap();
    let c = Client::open_with(&store, &backend).unwrap();
    let s = c
        .create_secret(Secret::new("db", None, SecretSpec::default()), b"pw-vault")
        .unwrap();
    let pki = c
        .issue_server_cert("gw.local", &["gw.local".into()])
        .unwrap();
    assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
    assert!(store
        .get_meta::<serde_json::Value>(META_ROOT_KEY_WRAPPED)
        .unwrap()
        .is_some());

    // Restart: the key comes back from Vault, CA keys and payloads open.
    forget_unwrapped();
    let c2 = Client::open_with(&store, &backend).unwrap();
    assert_eq!(c2.get_secret_payload(&s.meta.id).unwrap(), b"pw-vault");
    assert_eq!(c2.ca_bundle_pem(), pki.ca_pem);
    let cert = c2.list_certificates(None).unwrap().remove(0);
    assert_eq!(c2.certificate_key_pem(&cert).unwrap(), pki.key_pem);

    // The software backend refuses this store rather than starting over.
    assert!(matches!(
        Client::open_with(&store, &KeyBackend::Software),
        Err(KeepError::Backend(_))
    ));
    // A token Vault refuses cannot open it either.
    assert!(matches!(
        Client::open_with(
            &store,
            &KeyBackend::VaultTransit(vault.config("s.revoked", KEY))
        ),
        Err(KeepError::Backend(_))
    ));
}

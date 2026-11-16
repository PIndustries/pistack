// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::consts::*;
use crate::backend::test_support::*;
use crate::backend::KeyBackend;

#[test]
fn vault_settings_are_validated() {
    let base = [
        (ENV_BACKEND, "vault"),
        (ENV_VAULT_ADDR, "https://vault.example:8200/"),
        (ENV_VAULT_TOKEN, "s.abc"),
        (ENV_VAULT_KEY, "pistack-root"),
    ];
    let KeyBackend::VaultTransit(v) = parse(&base).unwrap() else {
        panic!("expected vault");
    };
    assert_eq!(v.addr, "https://vault.example:8200");
    assert_eq!(v.mount, "transit");
    assert_eq!(v.key, "pistack-root");
    assert_eq!(v.token, "s.abc");
    assert!(v.namespace.is_none());

    for missing in [ENV_VAULT_ADDR, ENV_VAULT_KEY] {
        let pairs: Vec<_> = base
            .iter()
            .copied()
            .filter(|(k, _)| *k != missing)
            .collect();
        assert!(err(&pairs).contains(missing));
    }
    let no_token: Vec<_> = base
        .iter()
        .copied()
        .filter(|(k, _)| *k != ENV_VAULT_TOKEN)
        .collect();
    assert!(err(&no_token).contains(ENV_VAULT_TOKEN_FILE));

    let with = |extra: (&'static str, &'static str)| {
        let mut p: Vec<_> = base
            .iter()
            .copied()
            .filter(|(k, _)| *k != extra.0)
            .collect();
        p.push(extra);
        p
    };
    assert!(err(&with((ENV_VAULT_ADDR, "http://vault.example:8200"))).contains("loopback"));
    assert!(parse(&with((ENV_VAULT_ADDR, "http://127.0.0.1:8200"))).is_ok());
    assert!(parse(&with((ENV_VAULT_ADDR, "http://[::1]:8200"))).is_ok());
    assert!(parse(&with((ENV_VAULT_ADDR, "http://localhost:8200"))).is_ok());
    assert!(err(&with((ENV_VAULT_ADDR, "ftp://vault"))).contains("scheme"));
    assert!(err(&with((ENV_VAULT_ADDR, "vault:8200"))).contains(ENV_VAULT_ADDR));
    assert!(err(&with((ENV_VAULT_KEY, "../sys"))).contains(ENV_VAULT_KEY));
    assert!(err(&with((ENV_VAULT_KEY, "a/b"))).contains(ENV_VAULT_KEY));
    assert!(err(&with((ENV_VAULT_MOUNT, "kv/../sys"))).contains(ENV_VAULT_MOUNT));
    let KeyBackend::VaultTransit(v) = parse(&with((ENV_VAULT_MOUNT, "/team/transit/"))).unwrap()
    else {
        panic!("expected vault");
    };
    assert_eq!(v.mount, "team/transit");
    assert!(err(&with((ENV_VAULT_TOKEN_FILE, "/nonexistent"))).contains("only one"));
    assert!(err(&with((ENV_VAULT_CACERT, "/nonexistent/ca.pem"))).contains(ENV_VAULT_CACERT));
}

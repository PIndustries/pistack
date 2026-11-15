// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::consts::TERM_KEY;
    use super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    use pistack_store::Store;
    use pistack_types::Leadership;
    #[tokio::test]
    async fn a_follower_never_writes_and_rows_carry_the_leaders_term() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        store.put_meta(TERM_KEY, &7u64).unwrap();
        let leadership = Leadership::new(false);
        b.attach(&leadership, &store);
        let n = net(NetworkKind::Overlay);
        let err = b.ensure_network(&n).await.unwrap_err();
        assert!(err.message.contains("not leader"), "{}", err.message);
        assert_eq!(db.writes(), 0);
        assert!(db.rows("Logical_Switch").is_empty());

        leadership.set(true);
        b.ensure_network(&n).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ext(&ls[0], "pistack:term").as_deref(), Some("7"));
    }
}

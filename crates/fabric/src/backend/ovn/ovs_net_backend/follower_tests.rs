// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::{Direction, EtherType, Rule};
    use pistack_store::Store;
    use pistack_types::Leadership;
    #[tokio::test]
    async fn a_follower_writes_no_security_groups() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        let leadership = Leadership::new(false);
        b.attach(&leadership, &store);
        let g = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        assert!(b.apply_security_group(&g, &[]).await.is_err());
        assert!(b.delete_security_group(&g.meta.id).await.is_err());
        assert_eq!(db.writes(), 0);
    }
}

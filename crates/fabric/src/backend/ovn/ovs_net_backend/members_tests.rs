// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::{Direction, EtherType, NetworkKind, Rule};
    #[tokio::test]
    async fn applying_a_group_sets_its_members_from_their_switch_ports() {
        let (db, b) = backend();
        let g = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let mut a = port(&n, "fa:16:3e:00:00:21", &["10.20.0.21"]);
        a.spec.security_groups = vec![g.meta.id.clone()];
        b.ensure_port(&n, &a, &[]).await.unwrap();
        let mut c = port(&n, "fa:16:3e:00:00:22", &["10.20.0.22"]);
        c.spec.security_groups = vec![g.meta.id.clone()];
        // `c` has no switch port yet; it joins when it is ensured.
        b.apply_security_group(&g, &[a.clone(), c.clone()])
            .await
            .unwrap();
        let name = format!("pg_{}", g.meta.id.uuid.simple());
        assert_eq!(
            ovsdb::elems(group(&db, &name).unwrap().get("ports")),
            vec![lsp_uuid(&db, &a)]
        );
        b.ensure_port(&n, &c, std::slice::from_ref(&g))
            .await
            .unwrap();
        let mut have = ovsdb::elems(group(&db, &name).unwrap().get("ports"));
        let mut want = vec![lsp_uuid(&db, &a), lsp_uuid(&db, &c)];
        have.sort_by_key(|v| v.to_string());
        want.sort_by_key(|v| v.to_string());
        assert_eq!(have, want);
    }
}

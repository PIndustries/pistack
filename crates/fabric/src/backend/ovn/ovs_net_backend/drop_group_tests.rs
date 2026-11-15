// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::{Direction, EtherType, Rule};
    #[tokio::test]
    async fn a_port_with_security_joins_the_drop_group_and_its_groups() {
        let (db, b) = backend();
        let web = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let db_sg = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let (n, mut p) = ready_port(&b, std::slice::from_ref(&web)).await;
        let u = lsp_uuid(&db, &p);
        let web_name = format!("pg_{}", web.meta.id.uuid.simple());
        let db_name = format!("pg_{}", db_sg.meta.id.uuid.simple());

        let drop = group(&db, "pg_pistack_drop").unwrap();
        assert_eq!(ovsdb::elems(drop.get("ports")), vec![u.clone()]);
        let mut want = vec![
            "from-lport 1001 drop inport == @pg_pistack_drop && ip".to_string(),
            "to-lport 1001 drop outport == @pg_pistack_drop && ip".to_string(),
            "from-lport 1002 allow inport == @pg_pistack_drop && udp && udp.src == 68 && udp.dst == 67".to_string(),
            "from-lport 1002 allow inport == @pg_pistack_drop && udp && udp.src == 546 && udp.dst == 547".to_string(),
            "from-lport 1002 allow-related inport == @pg_pistack_drop && ip4 && ip4.dst == 169.254.169.254 && tcp && tcp.dst == 80".to_string(),
        ];
        want.sort();
        assert_eq!(acl_matches(&db, "pg_pistack_drop"), want);
        let wg = group(&db, &web_name).unwrap();
        assert_eq!(ovsdb::elems(wg.get("ports")), vec![u.clone()]);
        assert_eq!(acl_matches(&db, &web_name).len(), 1);

        let writes = db.writes();
        b.ensure_port(&n, &p, std::slice::from_ref(&web))
            .await
            .unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        // Applying the group with its members agrees with the port.
        b.apply_security_group(&web, std::slice::from_ref(&p))
            .await
            .unwrap();
        assert_eq!(db.writes(), writes);

        // Moving the port to another group takes it out of the first.
        p.spec.security_groups = vec![db_sg.meta.id.clone()];
        b.ensure_port(&n, &p, std::slice::from_ref(&db_sg))
            .await
            .unwrap();
        assert!(ovsdb::elems(group(&db, &web_name).unwrap().get("ports")).is_empty());
        assert_eq!(
            ovsdb::elems(group(&db, &db_name).unwrap().get("ports")),
            vec![u.clone()]
        );

        // Without port security it is in no group at all.
        p.spec.port_security = false;
        p.spec.security_groups.clear();
        b.ensure_port(&n, &p, &[]).await.unwrap();
        for g in db.rows("Port_Group") {
            assert!(ovsdb::elems(g.get("ports")).is_empty(), "{g}");
        }

        // Deleting a port takes it out of its groups.
        p.spec.port_security = true;
        p.spec.security_groups = vec![db_sg.meta.id.clone()];
        b.ensure_port(&n, &p, std::slice::from_ref(&db_sg))
            .await
            .unwrap();
        b.delete_port(&p.meta.id).await.unwrap();
        for g in db.rows("Port_Group") {
            assert!(ovsdb::elems(g.get("ports")).is_empty(), "{g}");
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::{Direction, EtherType, Protocol, Remote, Rule};
    use pistack_types::{kind, Id};
    #[tokio::test]
    async fn rules_become_acls_on_the_groups_port_group() {
        let (db, b) = backend();
        let other = Id::new(kind::SECGROUP);
        let mut g = sg(vec![]);
        let me = g.meta.id.clone();
        g.spec.rules = vec![
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22)
                .remote(Remote::Cidr("10.0.0.0/8".parse().unwrap())),
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Udp)
                .ports(5000, 5100),
            Rule::new(Direction::Ingress, EtherType::IPv4).remote(Remote::Group(me.clone())),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .protocol(Protocol::Icmp)
                .remote(Remote::Group(other.clone())),
            Rule::new(Direction::Egress, EtherType::IPv4),
            Rule::new(Direction::Egress, EtherType::IPv6),
            // A v4 remote on an IPv6 rule matches nothing.
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .remote(Remote::Cidr("192.168.0.0/16".parse().unwrap())),
        ];
        b.apply_security_group(&g, &[]).await.unwrap();
        let name = format!("pg_{}", me.uuid.simple());
        let o = format!("pg_{}", other.uuid.simple());
        let mut want = vec![
            format!("from-lport 1002 allow-related inport == @{name} && ip4"),
            format!("from-lport 1002 allow-related inport == @{name} && ip6"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && ip4.src == ${name}_ip4"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && ip4.src == 10.0.0.0/8 && tcp && tcp.dst == 22"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && udp && udp.dst >= 5000 && udp.dst <= 5100"),
            format!("to-lport 1002 allow-related outport == @{name} && ip6 && ip6.src == ${o}_ip6 && icmp6"),
        ];
        want.sort();
        assert_eq!(acl_matches(&db, &name), want);
        let pg = group(&db, &name).unwrap();
        assert_eq!(ext(&pg, "pistack:id"), Some(me.to_string()));
        assert_eq!(ovsdb::elems(pg.get("acls")).len(), 6);
        // The group the ICMP rule names exists, empty, so its address set does.
        let og = group(&db, &o).unwrap();
        assert!(ovsdb::elems(og.get("acls")).is_empty());
        assert_eq!(ext(&og, "pistack:id"), Some(other.to_string()));

        let writes = db.writes();
        b.apply_security_group(&g, &[]).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        g.spec.rules.truncate(1);
        b.apply_security_group(&g, &[]).await.unwrap();
        assert_eq!(acl_matches(&db, &name).len(), 1);
        assert_eq!(db.rows("ACL").len(), 1, "replaced ACLs are dropped");

        b.delete_security_group(&me).await.unwrap();
        b.delete_security_group(&me).await.unwrap();
        assert!(group(&db, &name).is_none());
        assert!(db.rows("ACL").is_empty());
    }
}

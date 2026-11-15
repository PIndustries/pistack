// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::{HostRoute, NetworkKind};
    #[tokio::test]
    async fn a_dhcp_subnet_gets_options_and_turning_dhcp_off_removes_them() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let mut s = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        s.spec.host_routes = vec![HostRoute {
            destination: "192.168.5.0/24".parse().unwrap(),
            nexthop: "10.20.0.254".parse().unwrap(),
        }];
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s).await.unwrap();
        let d = db.rows("DHCP_Options");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0]["cidr"], "10.20.0.0/24");
        assert_eq!(
            opt(&d[0], "options", "server_id").as_deref(),
            Some("10.20.0.1")
        );
        assert_eq!(
            opt(&d[0], "options", "router").as_deref(),
            Some("10.20.0.1")
        );
        assert_eq!(
            opt(&d[0], "options", "lease_time").as_deref(),
            Some("43200")
        );
        assert_eq!(
            opt(&d[0], "options", "dns_server").as_deref(),
            Some("{10.0.0.53}")
        );
        assert_eq!(
            opt(&d[0], "options", "classless_static_route").as_deref(),
            Some("{169.254.169.254/32,0.0.0.0, 192.168.5.0/24,10.20.0.254, 0.0.0.0/0,10.20.0.1}")
        );
        assert!(opt(&d[0], "options", "server_mac")
            .unwrap()
            .starts_with("02:"));
        assert_eq!(ext(&d[0], "pistack:network"), Some(n.meta.id.to_string()));

        let writes = db.writes();
        b.ensure_subnet(&n, &s).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        let mut off = s.clone();
        off.spec.enable_dhcp = false;
        b.ensure_subnet(&n, &off).await.unwrap();
        assert!(db.rows("DHCP_Options").is_empty());

        b.ensure_subnet(&n, &s).await.unwrap();
        b.delete_subnet(&s.meta.id).await.unwrap();
        b.delete_subnet(&s.meta.id).await.unwrap();
        assert!(db.rows("DHCP_Options").is_empty());
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::metadata_port_name;
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    use pistack_types::Id;
    use serde_json::json;
    #[tokio::test]
    async fn a_port_joins_its_switch_with_addresses_security_and_dhcp() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let s4 = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        let s6 = subnet(&n, "fd00:20::/64", "fd00:20::1");
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s4).await.unwrap();
        b.ensure_subnet(&n, &s6).await.unwrap();
        let p = port(&n, "fa:16:3e:00:00:01", &["10.20.0.5", "fd00:20::5"]);
        let binding = b.ensure_port(&n, &p, &[]).await.unwrap();
        assert_eq!(binding.vif_type, "ovs");
        assert_eq!(binding.vif_details["bridge"], "br-int");
        assert_eq!(binding.vif_details["iface_id"], p.meta.id.uuid.to_string());

        let lsp = guest_ports(&db);
        assert_eq!(lsp.len(), 1);
        let want = "fa:16:3e:00:00:01 10.20.0.5 fd00:20::5";
        // Bare UUID: libvirt refuses any other iface-id.
        assert_eq!(lsp[0]["name"], json!(p.meta.id.uuid.to_string()));
        assert_eq!(ovsdb::canon(&lsp[0]["addresses"]), json!(want));
        assert_eq!(ovsdb::canon(&lsp[0]["port_security"]), json!(want));
        assert_eq!(
            ext(&lsp[0], "pistack:device"),
            p.spec.device.as_ref().map(Id::to_string)
        );
        let dhcp = db.rows("DHCP_Options");
        let uuid_of = |cidr: &str| {
            dhcp.iter()
                .find(|r| r["cidr"] == cidr)
                .map(|r| r["_uuid"].clone())
                .unwrap()
        };
        assert_eq!(
            ovsdb::canon(&lsp[0]["dhcpv4_options"]),
            uuid_of("10.20.0.0/24")
        );
        assert_eq!(
            ovsdb::canon(&lsp[0]["dhcpv6_options"]),
            uuid_of("fd00:20::/64")
        );
        let ls = db.rows("Logical_Switch");
        let meta = named(&db, "Logical_Switch_Port", &metadata_port_name(&n.meta.id)).unwrap();
        let on_switch = ovsdb::elems(ls[0].get("ports"));
        assert_eq!(on_switch.len(), 2);
        assert!(on_switch.contains(&lsp[0]["_uuid"]));
        assert!(on_switch.contains(&meta["_uuid"]));

        let writes = db.writes();
        b.ensure_port(&n, &p, &[]).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        let mut open = p.clone();
        open.spec.port_security = false;
        b.ensure_port(&n, &open, &[]).await.unwrap();
        let lsp = guest_ports(&db);
        assert_eq!(ovsdb::canon(&lsp[0]["port_security"]), json!(["set", []]));

        b.delete_port(&p.meta.id).await.unwrap();
        b.delete_port(&p.meta.id).await.unwrap();
        assert!(guest_ports(&db).is_empty());
        let ls = db.rows("Logical_Switch");
        assert_eq!(
            ovsdb::elems(ls[0].get("ports")),
            vec![meta["_uuid"].clone()]
        );
    }
}

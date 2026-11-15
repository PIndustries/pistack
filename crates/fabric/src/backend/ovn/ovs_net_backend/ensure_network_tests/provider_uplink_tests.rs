// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    use serde_json::json;
    #[tokio::test]
    async fn a_provider_network_gets_a_localnet_uplink_on_its_switch() {
        let (db, b) = backend();
        let n = net(NetworkKind::Provider {
            vlan: Some(42),
            physnet: "physnet1".into(),
        });
        b.ensure_network(&n).await.unwrap();
        b.ensure_network(&n).await.unwrap();
        let lsp = guest_ports(&db);
        assert_eq!(lsp.len(), 1);
        assert_eq!(lsp[0]["name"], json!(format!("provnet-{}", n.meta.id)));
        assert_eq!(lsp[0]["type"], "localnet");
        assert_eq!(
            opt(&lsp[0], "options", "network_name").as_deref(),
            Some("physnet1")
        );
        assert_eq!(ovsdb::canon(&lsp[0]["tag"]), json!(42));
        let ls = db.rows("Logical_Switch");
        let on_switch = ovsdb::elems(ls[0].get("ports"));
        assert_eq!(on_switch.len(), 2, "the uplink and the metadata port");
        assert!(on_switch.contains(&lsp[0]["_uuid"]));
    }
}

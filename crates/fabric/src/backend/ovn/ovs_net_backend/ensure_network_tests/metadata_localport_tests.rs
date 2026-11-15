// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::super::test_support::*;
    use super::super::super::super::{metadata_port_mac, metadata_port_name};
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    use serde_json::json;
    #[tokio::test]
    async fn every_network_has_a_metadata_localport_on_its_switch() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let other = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        b.ensure_network(&other).await.unwrap();
        let writes = db.writes();
        b.ensure_network(&n).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        let name = metadata_port_name(&n.meta.id);
        assert_eq!(name, format!("pimeta-{}", n.meta.id.uuid));
        let port = named(&db, "Logical_Switch_Port", &name).unwrap();
        assert_eq!(port["type"], "localport");
        let mac = metadata_port_mac(&n.meta.id);
        assert!(mac.starts_with("06:") && mac.len() == 17, "{mac}");
        assert_eq!(
            ovsdb::canon(&port["addresses"]),
            json!(format!("{mac} 169.254.169.254"))
        );
        assert_eq!(ext(&port, "pistack:metadata").as_deref(), Some("true"));
        assert_eq!(ext(&port, "pistack:id"), Some(n.meta.id.to_string()));
        // On its own network's switch, and only there.
        let ls = named(&db, "Logical_Switch", &format!("ps-{}", n.meta.id)).unwrap();
        assert_eq!(ovsdb::elems(ls.get("ports")), vec![port["_uuid"].clone()]);
        assert_ne!(mac, metadata_port_mac(&other.meta.id));
        assert_eq!(db.rows("Logical_Switch_Port").len(), 2);

        b.delete_network(&n.meta.id).await.unwrap();
        assert!(named(&db, "Logical_Switch_Port", &name).is_none());
        assert_eq!(db.rows("Logical_Switch_Port").len(), 1);
    }
}

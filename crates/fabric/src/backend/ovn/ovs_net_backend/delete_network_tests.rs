// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    #[tokio::test]
    async fn deleting_a_network_takes_its_ports_and_dhcp_with_it() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let s = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s).await.unwrap();
        b.ensure_port(&n, &port(&n, "fa:16:3e:00:00:03", &["10.20.0.7"]), &[])
            .await
            .unwrap();
        b.delete_network(&n.meta.id).await.unwrap();
        b.delete_network(&n.meta.id).await.unwrap();
        assert!(db.rows("Logical_Switch").is_empty());
        assert!(db.rows("Logical_Switch_Port").is_empty());
        assert!(db.rows("DHCP_Options").is_empty());
    }
}

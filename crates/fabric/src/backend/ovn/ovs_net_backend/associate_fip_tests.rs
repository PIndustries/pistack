// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::FipView;
    use crate::backend::NetBackend;
    use crate::types::{FloatingIp, FloatingIpSpec, FloatingIpStatus, NetworkKind};
    use pistack_types::{kind, Meta};
    use serde_json::json;
    #[tokio::test]
    async fn a_floating_ip_is_one_nat_row_that_follows_its_port() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let (outside, _) = view.gateway.clone().unwrap();
        let tenant_net = {
            let (tsub, _) = &view.interfaces[0];
            let mut n = net(NetworkKind::Overlay);
            n.meta.id = tsub.spec.network.clone();
            n.meta.project = tsub.meta.project.clone();
            n
        };
        let vm = port(&tenant_net, "fa:16:3e:00:00:05", &["10.0.0.5", "10.0.0.6"]);
        b.ensure_port(&tenant_net, &vm, &[]).await.unwrap();
        let fip = FloatingIp {
            meta: Meta::new(kind::FLOATING_IP, "f", tenant_net.meta.project.clone()),
            spec: FloatingIpSpec {
                network: outside.meta.id.clone(),
                address: Some("203.0.113.20".parse().unwrap()),
                port: Some(vm.meta.id.clone()),
            },
            status: FloatingIpStatus::default(),
        };
        let at = |ip: &str| FipView {
            fip: fip.clone(),
            target: Some((vm.clone(), ip.parse().unwrap())),
            router: Some(view.router.clone()),
        };

        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        let dnat = nat_of(&db, "dnat_and_snat");
        assert_eq!(dnat.len(), 1);
        assert_eq!(dnat[0]["external_ip"], "203.0.113.20");
        assert_eq!(dnat[0]["logical_ip"], "10.0.0.5");
        assert_eq!(
            ovsdb::canon(&dnat[0]["logical_port"]),
            json!(vm.meta.id.uuid.to_string())
        );
        assert_eq!(
            ovsdb::canon(&dnat[0]["external_mac"]),
            json!("fa:16:3e:00:00:05")
        );
        let lr = db.rows("Logical_Router").pop().unwrap();
        assert!(ovsdb::elems(lr.get("nat")).contains(&dnat[0]["_uuid"]));

        let writes = db.writes();
        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        b.ensure_router(&view).await.unwrap();
        assert_eq!(db.writes(), writes, "repeats write nothing");
        assert_eq!(
            nat_of(&db, "dnat_and_snat").len(),
            1,
            "router keeps the fip row"
        );

        b.associate_fip(&at("10.0.0.6")).await.unwrap();
        let dnat = nat_of(&db, "dnat_and_snat");
        assert_eq!(dnat.len(), 1);
        assert_eq!(dnat[0]["logical_ip"], "10.0.0.6");

        let off = FipView {
            fip: fip.clone(),
            target: None,
            router: None,
        };
        b.associate_fip(&off).await.unwrap();
        assert!(nat_of(&db, "dnat_and_snat").is_empty());
        assert_eq!(nat_of(&db, "snat").len(), 1, "router snat stays");

        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        b.delete_fip(&fip.meta.id).await.unwrap();
        assert!(nat_of(&db, "dnat_and_snat").is_empty());
        b.delete_fip(&fip.meta.id).await.unwrap();

        let lost = FipView {
            router: None,
            ..at("10.0.0.5")
        };
        let err = b.associate_fip(&lost).await.unwrap_err();
        assert!(
            err.message.contains("no router connects the network")
                && err.message.contains("Attach the port's subnet to a router"),
            "{err}"
        );
    }
}

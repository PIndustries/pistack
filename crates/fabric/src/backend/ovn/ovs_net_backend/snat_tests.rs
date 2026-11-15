// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::switch_name::switch_name;
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use serde_json::json;
    #[tokio::test]
    async fn a_router_gets_ports_peers_gateway_routes_and_snat() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let rid = &view.router.meta.id;
        let lr = named(&db, "Logical_Router", &format!("pr-{rid}")).unwrap();
        assert_eq!(
            opt(&lr, "options", "dynamic_neigh_routers").as_deref(),
            Some("true")
        );

        let (tsub, iface) = &view.interfaces[0];
        let lrp = named(
            &db,
            "Logical_Router_Port",
            &format!("lrp-{}", iface.meta.id),
        )
        .unwrap();
        assert_eq!(lrp["mac"], "fa:16:3e:00:00:02");
        assert_eq!(ovsdb::canon(&lrp["networks"]), json!("10.0.0.1/24"));
        let peer = named(&db, "Logical_Switch_Port", &iface.meta.id.to_string()).unwrap();
        assert_eq!(peer["type"], "router");
        assert_eq!(
            opt(&peer, "options", "router-port"),
            Some(format!("lrp-{}", iface.meta.id))
        );
        let ls = named(&db, "Logical_Switch", &switch_name(&tsub.spec.network)).unwrap();
        assert!(ovsdb::elems(ls.get("ports")).contains(&peer["_uuid"]));

        let (_, gw) = view.gateway.as_ref().unwrap();
        let glrp = named(&db, "Logical_Router_Port", &format!("lrp-{}", gw.meta.id)).unwrap();
        assert_eq!(ovsdb::canon(&glrp["networks"]), json!("203.0.113.10/24"));
        let mut chassis: Vec<(String, i64)> = db
            .rows("Gateway_Chassis")
            .iter()
            .map(|r| {
                (
                    r["chassis_name"].as_str().unwrap().to_string(),
                    r["priority"].as_i64().unwrap(),
                )
            })
            .collect();
        chassis.sort();
        assert_eq!(chassis, vec![("gw-a".into(), 2), ("gw-b".into(), 1)]);
        assert_eq!(ovsdb::elems(glrp.get("gateway_chassis")).len(), 2);

        assert_eq!(
            route_pairs(&db),
            vec![
                ("0.0.0.0/0".into(), "203.0.113.1".into()),
                ("192.168.0.0/16".into(), "10.0.0.254".into()),
            ]
        );
        let snat = nat_of(&db, "snat");
        assert_eq!(snat.len(), 1);
        assert_eq!(snat[0]["logical_ip"], "10.0.0.0/24");
        assert_eq!(snat[0]["external_ip"], "203.0.113.10");

        let writes = db.writes();
        b.ensure_router(&view).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat writes nothing");
    }
}

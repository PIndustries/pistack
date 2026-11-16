// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use super::super::super::{gateway_candidates, HostRecord};
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use pistack_store::Store;
    use pistack_types::{Leadership, Phase};
    #[tokio::test]
    async fn gateway_ports_follow_the_hosts_that_report_an_uplink() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        b.attach(&Leadership::always(), &store);
        let hosts = store.collection::<HostRecord>();
        let uplink = Some((true, "physnet1:br-ex"));
        let a = hosts
            .create(host_record("node-a", Phase::Ready, uplink))
            .unwrap();
        hosts
            .create(host_record("node-c", Phase::Ready, uplink))
            .unwrap();
        // None of these can carry a gateway: no uplink, ovn-controller not
        // connected, the host itself stale, and no report at all.
        hosts
            .create(host_record("node-b", Phase::Ready, Some((true, ""))))
            .unwrap();
        hosts
            .create(host_record(
                "node-d",
                Phase::Ready,
                Some((false, "physnet1:br-ex")),
            ))
            .unwrap();
        hosts
            .create(host_record("node-e", Phase::Error, uplink))
            .unwrap();
        hosts
            .create(host_record("node-f", Phase::Ready, None))
            .unwrap();
        assert_eq!(
            gateway_candidates(&store).unwrap(),
            vec!["node-a".to_string(), "node-c".to_string()]
        );

        let (db, b, view) = routed_on(db, b).await;
        b.ensure_router(&view).await.unwrap();
        assert_eq!(
            scheduled_on(&db),
            vec![("node-a".into(), 2), ("node-c".into(), 1)]
        );
        let writes = db.writes();
        b.refresh().await.unwrap();
        b.ensure_router(&view).await.unwrap();
        assert_eq!(db.writes(), writes, "nothing changed, nothing written");

        // node-a loses its southbound connection.
        hosts
            .modify(&a.meta.id, |h| {
                h.status.ovn.as_mut().unwrap().ready = false;
                Ok(())
            })
            .unwrap();
        b.refresh().await.unwrap();
        assert_eq!(scheduled_on(&db), vec![("node-c".into(), 1)]);

        // No host left: the port keeps no chassis, and gets them back on
        // the first pass after a host returns.
        for h in hosts.list(None).unwrap() {
            hosts.delete(&h.meta.id).unwrap();
        }
        b.refresh().await.unwrap();
        assert!(db.rows("Gateway_Chassis").is_empty());
        let gw = db
            .rows("Logical_Router_Port")
            .into_iter()
            .find(|r| ext(r, "pistack:gateway").is_some())
            .unwrap();
        assert!(ovsdb::elems(gw.get("gateway_chassis")).is_empty());
        hosts
            .create(host_record("node-g", Phase::Ready, uplink))
            .unwrap();
        b.refresh().await.unwrap();
        assert_eq!(scheduled_on(&db), vec![("node-g".into(), 1)]);
    }
}

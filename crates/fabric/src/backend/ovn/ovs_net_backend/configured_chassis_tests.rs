// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use super::super::super::HostRecord;
    use crate::backend::NetBackend;
    use pistack_store::Store;
    use pistack_types::{Leadership, Phase};
    #[tokio::test]
    async fn the_configured_chassis_list_wins_over_the_hosts() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        b.attach(&Leadership::always(), &store);
        store
            .collection::<HostRecord>()
            .create(host_record(
                "node-a",
                Phase::Ready,
                Some((true, "physnet1:br-ex")),
            ))
            .unwrap();
        let b = b.with_gateway_chassis(vec!["gw-a".into(), "gw-b".into()]);
        let (db, b, view) = routed_on(db, b).await;
        b.ensure_router(&view).await.unwrap();
        b.refresh().await.unwrap();
        assert_eq!(
            scheduled_on(&db),
            vec![("gw-a".into(), 2), ("gw-b".into(), 1)]
        );
    }
}

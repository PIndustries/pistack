// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    #[tokio::test]
    async fn deleting_a_router_takes_its_rows_and_switch_peers() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        b.delete_router(&view.router.meta.id).await.unwrap();
        for table in [
            "Logical_Router",
            "Logical_Router_Port",
            "Logical_Router_Static_Route",
            "NAT",
            "Gateway_Chassis",
        ] {
            assert!(db.rows(table).is_empty(), "{table} left behind");
        }
        assert!(db
            .rows("Logical_Switch_Port")
            .iter()
            .all(|r| r["type"] != "router"));
        assert_eq!(db.rows("Logical_Switch").len(), 2);
        b.delete_router(&view.router.meta.id).await.unwrap();
    }
}

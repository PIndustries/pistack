// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    #[tokio::test]
    async fn dropping_an_interface_and_a_route_removes_only_those() {
        let (db, b, mut view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let (_, iface) = view.interfaces.pop().unwrap();
        view.router.spec.interfaces.clear();
        view.router.spec.routes.clear();
        b.ensure_router(&view).await.unwrap();

        assert_eq!(db.rows("Logical_Router_Port").len(), 1);
        assert!(named(&db, "Logical_Switch_Port", &iface.meta.id.to_string()).is_none());
        assert_eq!(
            route_pairs(&db),
            vec![("0.0.0.0/0".into(), "203.0.113.1".into())]
        );
        assert!(nat_of(&db, "snat").is_empty());
        assert_eq!(db.rows("Gateway_Chassis").len(), 2);
    }
}

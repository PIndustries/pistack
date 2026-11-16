// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    use pistack_types::Id;
    use serde_json::json;
    #[tokio::test]
    async fn a_network_becomes_a_switch_and_a_repeat_writes_nothing() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0]["name"], json!(format!("ps-{}", n.meta.id)));
        assert_eq!(opt(&ls[0], "other_config", "mtu").as_deref(), Some("1500"));
        assert_eq!(ext(&ls[0], "pistack:id"), Some(n.meta.id.to_string()));
        assert_eq!(
            ext(&ls[0], "pistack:project"),
            n.meta.project.as_ref().map(Id::to_string)
        );
        assert_eq!(ext(&ls[0], "pistack:term").as_deref(), Some("0"));

        let writes = db.writes();
        b.ensure_network(&n).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");
        assert_eq!(db.rows("Logical_Switch").len(), 1);

        let mut bigger = n.clone();
        bigger.spec.mtu = 9000;
        b.ensure_network(&bigger).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ls.len(), 1);
        assert_eq!(opt(&ls[0], "other_config", "mtu").as_deref(), Some("9000"));
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    use crate::types::NetworkKind;
    #[tokio::test]
    async fn a_port_on_a_missing_switch_fails_instead_of_vanishing() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let p = port(&n, "fa:16:3e:00:00:02", &["10.20.0.6"]);
        assert!(b.ensure_port(&n, &p, &[]).await.is_err());
        assert!(db.rows("Logical_Switch_Port").is_empty());
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::ovsdb;
    use crate::backend::NetBackend;
    use crate::types::{AllowedAddressPair, NetworkKind};
    use serde_json::json;
    #[tokio::test]
    async fn an_allowed_pair_rides_the_port_addresses_and_security() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let mut p = port(&n, "fa:16:3e:00:00:31", &["10.20.0.31"]);
        // A bare address is kept as that address with its host prefix, which
        // is how the API stores it.
        let pair = |ip: &str, mac: Option<&str>| AllowedAddressPair {
            ip_address: ip.parse().unwrap(),
            mac_address: mac.map(str::to_string),
        };
        p.spec.allowed_address_pairs = vec![
            // A virtual address two machines share, kept on the port's mac.
            pair("10.20.0.99/32", None),
            // The same address seen under another machine's mac.
            pair("10.20.0.98/32", Some("FA-16-3E-00-00-98")),
            // A whole prefix, so the machine may answer for anything in it.
            pair("10.20.9.0/28", Some("02:00:00:00:09:00")),
            // The same prefix under the port's own mac.
            pair("10.20.8.0/28", None),
        ];
        b.ensure_port(&n, &p, &[]).await.unwrap();
        let lsp = guest_ports(&db);
        assert_eq!(lsp.len(), 1);
        // addresses takes plain addresses, so a prefix is not listed there:
        // only the foreign mac the switch has nowhere else to learn.
        assert_eq!(
            ovsdb::canon(&lsp[0]["addresses"]),
            json!([
                "set",
                [
                    "02:00:00:00:09:00",
                    "fa:16:3e:00:00:31 10.20.0.31",
                    "fa:16:3e:00:00:31 10.20.0.99",
                    "fa:16:3e:00:00:98 10.20.0.98",
                ]
            ])
        );
        // port_security takes masks, so every pair is there as it was asked.
        assert_eq!(
            ovsdb::canon(&lsp[0]["port_security"]),
            json!([
                "set",
                [
                    "02:00:00:00:09:00 10.20.9.0/28",
                    "fa:16:3e:00:00:31 10.20.0.31",
                    "fa:16:3e:00:00:31 10.20.0.99/32",
                    "fa:16:3e:00:00:31 10.20.8.0/28",
                    "fa:16:3e:00:00:98 10.20.0.98/32",
                ]
            ])
        );

        let writes = db.writes();
        b.ensure_port(&n, &p, &[]).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        // The pair goes away with the spec entry.
        p.spec.allowed_address_pairs.remove(2);
        b.ensure_port(&n, &p, &[]).await.unwrap();
        let lsp = guest_ports(&db);
        assert_eq!(
            ovsdb::canon(&lsp[0]["addresses"]),
            json!([
                "set",
                [
                    "fa:16:3e:00:00:31 10.20.0.31",
                    "fa:16:3e:00:00:31 10.20.0.99",
                    "fa:16:3e:00:00:98 10.20.0.98",
                ]
            ])
        );
        assert_eq!(
            ovsdb::canon(&lsp[0]["port_security"]),
            json!([
                "set",
                [
                    "fa:16:3e:00:00:31 10.20.0.31",
                    "fa:16:3e:00:00:31 10.20.0.99/32",
                    "fa:16:3e:00:00:31 10.20.8.0/28",
                    "fa:16:3e:00:00:98 10.20.0.98/32",
                ]
            ])
        );

        // With port security off the port is filtered by nothing, so the
        // pairs are only listed as addresses.
        p.spec.port_security = false;
        b.ensure_port(&n, &p, &[]).await.unwrap();
        let lsp = guest_ports(&db);
        assert_eq!(ovsdb::canon(&lsp[0]["port_security"]), json!(["set", []]));
        assert_eq!(ovsdb::elems(lsp[0].get("addresses")).len(), 3);

        // A pair whose mac will not parse rides on the port's own mac. The
        // API refuses such a pair; this is the backend meeting one anyway.
        let mut junk = p.clone();
        junk.spec.port_security = true;
        junk.spec
            .allowed_address_pairs
            .push(pair("10.20.0.97/32", Some("not a mac")));
        b.ensure_port(&n, &junk, &[]).await.unwrap();
        let lsp = guest_ports(&db);
        let entries = ovsdb::elems(lsp[0].get("addresses"));
        assert_eq!(entries.len(), 4, "{entries:?}");
        assert!(entries.contains(&json!("fa:16:3e:00:00:31 10.20.0.97")));
    }
}

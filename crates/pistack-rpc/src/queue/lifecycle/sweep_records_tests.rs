// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;
use serde_json::json;
use uuid::Uuid;

use crate::proto::CommandKind;
use crate::queue::record::Record;
use crate::queue::record_key::record_key;
use crate::queue::test_support::*;

#[tokio::test]
async fn a_record_older_than_the_timeout_is_not_used_and_is_swept() {
    let store = Store::in_memory().unwrap();
    let old = Record {
        id: Uuid::now_v7(),
        kind: CommandKind::StartMachine,
        issued_at: Utc::now() - chrono::Duration::seconds(61),
    };
    store
        .put_meta(&record_key("n1", "machine/m1"), &old)
        .unwrap();
    store
        .put_meta(&record_key("n2", "machine/m2"), &old)
        .unwrap();
    let q = queue_on(&store, 60);
    let t = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    assert_ne!(t.id(), old.id);
    // The first operation swept: what nobody issued again is gone, what
    // is pending here stays.
    assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
    store
        .put_meta(&record_key("n2", "machine/m2"), &old)
        .unwrap();
    assert_eq!(q.sweep_records(), 1);
    assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
}

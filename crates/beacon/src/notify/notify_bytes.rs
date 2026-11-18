// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};

/// A `NOTIFY` for `<apex> SOA`. The new serial rides in the answer section as
/// the hint RFC 1996 allows; a secondary acts on the AXFR it pulls, not on
/// this number.
pub(crate) fn notify_bytes(id: u16, apex: &str, serial: u32) -> anyhow::Result<Vec<u8>> {
    let name = Name::from_ascii(apex)?;
    let mut message = Message::new(id, MessageType::Query, OpCode::Notify);
    message.metadata.authoritative = true;
    let mut query = Query::new();
    query
        .set_name(name.clone())
        .set_query_class(DNSClass::IN)
        .set_query_type(RecordType::SOA);
    message.add_query(query);
    message.add_answer(Record::from_rdata(
        name.clone(),
        0,
        RData::SOA(hickory_proto::rr::rdata::SOA::new(
            name.clone(),
            name,
            serial,
            0,
            0,
            0,
            0,
        )),
    ));
    Ok(message.to_vec()?)
}

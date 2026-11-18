// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::Zone;
use hickory_proto::rr::rdata::{NS, SOA};
use hickory_proto::rr::{Name, RData, Record};
use std::collections::HashMap;

use super::proto_type::proto_type;
use super::zone_data::ZoneData;

pub(crate) fn compile_zone(client: &Client, z: &Zone) -> anyhow::Result<ZoneData> {
    let apex = Name::from_ascii(&z.spec.name)?;
    let ttl = z.spec.ttl;
    let ns_names = z.nameservers();
    let soa = Record::from_rdata(
        apex.clone(),
        ttl,
        RData::SOA(SOA::new(
            Name::from_ascii(&ns_names[0])?,
            Name::from_ascii(z.soa_rname())?,
            z.status.serial,
            7200,
            3600,
            1_209_600,
            ttl,
        )),
    );
    let mut records: HashMap<Name, Vec<Record>> = HashMap::new();
    let apex_recs = records.entry(apex.clone()).or_default();
    apex_recs.push(soa.clone());
    for n in &ns_names {
        apex_recs.push(Record::from_rdata(
            apex.clone(),
            ttl,
            RData::NS(NS(Name::from_ascii(n)?)),
        ));
    }
    for rs in client.recordsets_in_zone(&z.meta.id)? {
        let owner = Name::from_ascii(&rs.spec.name)?;
        let rtype = proto_type(rs.spec.kind);
        let ttl = rs.spec.ttl.unwrap_or(ttl);
        for r in &rs.spec.records {
            match RData::try_from_str(rtype, r) {
                Ok(rdata) => records
                    .entry(owner.clone())
                    .or_default()
                    .push(Record::from_rdata(owner.clone(), ttl, rdata)),
                Err(e) => tracing::warn!(
                    set = %rs.meta.id, owner = %rs.spec.name, data = %r, error = %e,
                    "beacon: record skipped"
                ),
            }
        }
    }
    Ok(ZoneData { apex, soa, records })
}

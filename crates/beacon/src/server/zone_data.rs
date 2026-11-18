// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::ResponseCode;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use std::collections::HashMap;

use super::answer::Answer;
use super::max_cname_depth::MAX_CNAME_DEPTH;
use super::synth_owner::synth_owner;

/// One compiled zone: every owner name to its records, SOA and NS included.
#[derive(Clone)]
pub(crate) struct ZoneData {
    pub(crate) apex: Name,
    pub(crate) soa: Record,
    pub(crate) records: HashMap<Name, Vec<Record>>,
}

impl ZoneData {
    /// Build from an AXFR answer (SOA … records … SOA). The trailing SOA is
    /// dropped; the leading one becomes the zone SOA.
    pub(crate) fn from_axfr(apex: Name, records: Vec<Record>) -> anyhow::Result<Self> {
        if records.len() < 2 {
            anyhow::bail!("AXFR needs at least two SOA records");
        }
        let soa = records[0].clone();
        if soa.record_type() != RecordType::SOA {
            anyhow::bail!("AXFR must start with SOA");
        }
        let mut map: HashMap<Name, Vec<Record>> = HashMap::new();
        // Skip the closing SOA; keep the opening one and everything else.
        for r in &records[..records.len() - 1] {
            map.entry(r.name.clone()).or_default().push(r.clone());
        }
        if !map.contains_key(&apex) {
            // Ensure apex key exists even if names differ in FQDN flag.
            map.entry(apex.clone()).or_default().push(soa.clone());
        }
        Ok(Self {
            apex,
            soa,
            records: map,
        })
    }

    pub(crate) fn record_count(&self) -> u32 {
        self.records
            .values()
            .map(|v| {
                v.iter()
                    .filter(|r| r.record_type() != RecordType::SOA)
                    .count() as u32
            })
            .sum()
    }
}

impl ZoneData {
    /// Full zone contents for AXFR: SOA, every non-SOA record, SOA again.
    pub(crate) fn axfr(&self) -> Vec<Record> {
        let mut out = vec![self.soa.clone()];
        let mut names: Vec<&Name> = self.records.keys().collect();
        names.sort();
        for name in names {
            for r in &self.records[name] {
                if r.record_type() == RecordType::SOA {
                    continue;
                }
                out.push(r.clone());
            }
        }
        out.push(self.soa.clone());
        out
    }

    pub(crate) fn lookup(&self, name: &Name, qtype: RecordType) -> Answer {
        let mut answers = Vec::new();
        let mut current = name.clone();
        for _ in 0..MAX_CNAME_DEPTH {
            let recs = match self
                .records
                .get(&current)
                .or_else(|| self.wildcard(&current))
            {
                Some(r) => r,
                None if answers.is_empty() => {
                    return Answer {
                        code: ResponseCode::NXDomain,
                        answers,
                        authorities: vec![self.soa.clone()],
                    };
                }
                // CNAME target not in this zone: the client follows it.
                None => break,
            };
            let cname = recs
                .iter()
                .find(|r| r.record_type() == RecordType::CNAME)
                .cloned();
            match cname {
                Some(c) if qtype != RecordType::CNAME && qtype != RecordType::ANY => {
                    let target = match &c.data {
                        RData::CNAME(t) => t.0.clone(),
                        _ => break,
                    };
                    answers.push(synth_owner(c, &current));
                    if !self.apex.zone_of(&target) {
                        break;
                    }
                    current = target;
                }
                _ => {
                    answers.extend(
                        recs.iter()
                            .filter(|r| qtype == RecordType::ANY || r.record_type() == qtype)
                            .map(|r| synth_owner(r.clone(), &current)),
                    );
                    break;
                }
            }
        }
        if answers.is_empty() {
            // Name exists, type does not: NODATA.
            return Answer {
                code: ResponseCode::NoError,
                answers,
                authorities: vec![self.soa.clone()],
            };
        }
        let authorities = if qtype == RecordType::SOA || qtype == RecordType::NS {
            Vec::new()
        } else {
            self.records
                .get(&self.apex)
                .map(|v| {
                    v.iter()
                        .filter(|r| r.record_type() == RecordType::NS)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        Answer {
            code: ResponseCode::NoError,
            answers,
            authorities,
        }
    }

    /// `*.<parent>` for the closest ancestor inside the zone that has one.
    pub(crate) fn wildcard(&self, name: &Name) -> Option<&Vec<Record>> {
        let mut parent = name.base_name();
        while self.apex.zone_of(&parent) && parent.num_labels() >= self.apex.num_labels() {
            let star = Name::from_ascii("*").ok()?.append_domain(&parent).ok()?;
            if let Some(r) = self.records.get(&star) {
                return Some(r);
            }
            if parent == self.apex {
                break;
            }
            parent = parent.base_name();
        }
        None
    }
}

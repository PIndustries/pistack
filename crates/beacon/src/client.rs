// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client`: the one place zones and record sets are written.
//!
//! HTTP handlers, the auto-record task and other modules (`loom`, `relay`)
//! go through here. Every method validates and writes `spec`; the serial
//! reconciler (`reconcile_zone`) is the only writer of `status`.

use crate::types::*;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};
use std::collections::BTreeMap;

/// Status message for secondary zones until AXFR pull lands.
pub const SECONDARY_PENDING_MSG: &str =
    "secondary zone transfer (AXFR) is not implemented; zone is not served";

#[derive(Clone)]
pub struct Client {
    store: Store,
}

fn store_err(e: StoreError) -> ApiError {
    e.into()
}

impl Client {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn zones(&self) -> Collection<Zone> {
        self.store.collection()
    }
    pub fn recordsets(&self) -> Collection<RecordSet> {
        self.store.collection()
    }

    // ---- lookups ------------------------------------------------------------

    /// Record sets of one zone.
    pub fn recordsets_in_zone(&self, zone: &Id) -> ApiResult<Vec<RecordSet>> {
        Ok(self
            .recordsets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|r| &r.spec.zone == zone)
            .collect())
    }

    /// The zone with the longest apex that contains `name` (absolute,
    /// lower-case), if any. Primary zones only; secondaries are not served.
    pub fn zone_for_name(&self, name: &str) -> ApiResult<Option<Zone>> {
        let mut best: Option<Zone> = None;
        for z in self.zones().list(None).map_err(store_err)? {
            if z.spec.kind != ZoneKind::Primary || !in_zone(&z.spec.name, name) {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|b| z.spec.name.len() > b.spec.name.len())
            {
                best = Some(z);
            }
        }
        Ok(best)
    }

    /// Zone by apex name (any project).
    pub fn zone_by_apex(&self, apex: &str) -> ApiResult<Option<Zone>> {
        let apex = normalize_name(apex)?;
        Ok(self
            .zones()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .find(|z| z.spec.name == apex))
    }

    // ---- zones ------------------------------------------------------------

    pub fn create_zone(&self, mut meta: Meta, mut spec: ZoneSpec) -> ApiResult<Zone> {
        meta.id = Id::new(kind::ZONE);
        validate_zone(&mut spec)?;
        if let Some(other) = self.zone_by_apex(&spec.name)? {
            return Err(ApiError::conflict(format!(
                "zone {} already exists as {}",
                spec.name, other.meta.id
            )));
        }
        self.zones()
            .create(Zone {
                meta,
                spec,
                status: ZoneStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_zone(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Zone) -> ApiResult<()>,
    ) -> ApiResult<Zone> {
        let cur = self.zones().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        validate_zone(&mut next.spec)?;
        if next.spec.name != cur.spec.name {
            return Err(ApiError::bad_request("zone name is immutable"));
        }
        self.zones().update(next, expected).map_err(store_err)
    }

    /// Delete a zone and every record set in it.
    pub fn delete_zone(&self, id: &Id) -> ApiResult<Zone> {
        for rs in self.recordsets_in_zone(id)? {
            self.recordsets().delete(&rs.meta.id).map_err(store_err)?;
        }
        self.zones().delete(id).map_err(store_err)
    }

    // ---- record sets --------------------------------------------------------

    pub fn create_recordset(
        &self,
        mut meta: Meta,
        mut spec: RecordSetSpec,
    ) -> ApiResult<RecordSet> {
        meta.id = Id::new(kind::RECORD);
        let zone = self.zones().get(&spec.zone).map_err(store_err)?;
        check_same_project(&meta, &zone.meta, "zone")?;
        require_primary(&zone)?;
        validate_recordset(&zone, &mut spec)?;
        self.check_exclusive(&zone, &spec, None)?;
        self.recordsets()
            .create(RecordSet {
                meta,
                spec,
                status: RecordSetStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_recordset(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut RecordSet) -> ApiResult<()>,
    ) -> ApiResult<RecordSet> {
        let cur = self.recordsets().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.zone != cur.spec.zone {
            return Err(ApiError::bad_request("record set zone is immutable"));
        }
        let zone = self.zones().get(&cur.spec.zone).map_err(store_err)?;
        require_primary(&zone)?;
        validate_recordset(&zone, &mut next.spec)?;
        self.check_exclusive(&zone, &next.spec, Some(id))?;
        self.recordsets().update(next, expected).map_err(store_err)
    }

    pub fn delete_recordset(&self, id: &Id) -> ApiResult<RecordSet> {
        self.recordsets().delete(id).map_err(store_err)
    }

    /// One set per (owner, type); a CNAME owner holds nothing else and
    /// nothing else holds a CNAME.
    fn check_exclusive(&self, zone: &Zone, spec: &RecordSetSpec, me: Option<&Id>) -> ApiResult<()> {
        for other in self.recordsets_in_zone(&zone.meta.id)? {
            if Some(&other.meta.id) == me || other.spec.name != spec.name {
                continue;
            }
            if other.spec.kind == spec.kind {
                return Err(ApiError::conflict(format!(
                    "{} already has a {} set ({})",
                    spec.name, spec.kind, other.meta.id
                )));
            }
            if spec.kind == RecordType::Cname || other.spec.kind == RecordType::Cname {
                return Err(ApiError::conflict(format!(
                    "{} has a CNAME; it cannot carry other records",
                    spec.name
                )));
            }
        }
        Ok(())
    }

    // ---- for other modules ----------------------------------------------------

    /// Create or replace the `(name, type)` set in the zone that contains
    /// `name` (absolute, or relative to the zone). Idempotent: an identical
    /// set is left untouched so no serial bump happens. The set lives in the
    /// zone's project and is labelled `pistack.io/managed-by=<owner>`.
    pub fn upsert_record(
        &self,
        zone: &Id,
        name: &str,
        kind_: RecordType,
        ttl: Option<u32>,
        records: Vec<String>,
        owner: &str,
    ) -> ApiResult<RecordSet> {
        let z = self.zones().get(zone).map_err(store_err)?;
        require_primary(&z)?;
        let mut spec = RecordSetSpec {
            zone: zone.clone(),
            name: name.to_owned(),
            kind: kind_,
            ttl,
            records,
        };
        validate_recordset(&z, &mut spec)?;
        let existing = self
            .recordsets_in_zone(zone)?
            .into_iter()
            .find(|r| r.spec.name == spec.name && r.spec.kind == kind_);
        match existing {
            Some(cur) if cur.spec == spec => Ok(cur),
            Some(cur) => self.update_recordset(&cur.meta.id, None, |r| {
                r.spec = spec.clone();
                r.meta.labels.insert(MANAGED_LABEL.into(), owner.to_owned());
                Ok(())
            }),
            None => {
                let mut meta = Meta::new(kind::RECORD, set_name(&spec), z.meta.project.clone());
                meta.labels.insert(MANAGED_LABEL.into(), owner.to_owned());
                self.create_recordset(meta, spec)
            }
        }
    }

    /// Remove the `(name, type)` set from the zone if present. Returns the
    /// removed set; `None` when there was nothing to remove.
    pub fn remove_record(
        &self,
        zone: &Id,
        name: &str,
        kind_: RecordType,
    ) -> ApiResult<Option<RecordSet>> {
        let z = self.zones().get(zone).map_err(store_err)?;
        let name = owner_in_zone(&z.spec.name, name)?;
        for r in self.recordsets_in_zone(zone)? {
            if r.spec.name == name && r.spec.kind == kind_ {
                return Ok(Some(self.delete_recordset(&r.meta.id)?));
            }
        }
        Ok(None)
    }

    /// The zone in master-file format.
    pub fn export(&self, zone: &Id) -> ApiResult<String> {
        let z = self.zones().get(zone).map_err(store_err)?;
        let sets = self.recordsets_in_zone(zone)?;
        Ok(export_zone(&z, &sets))
    }

    // ---- reconciler --------------------------------------------------------------

    /// Recompute the zone's content digest; bump the serial when it moved.
    /// Idempotent, so it is safe to run on every event and on a timer.
    pub fn reconcile_zone(&self, id: &Id) -> ApiResult<Option<Zone>> {
        let zone = match self.zones().try_get(id).map_err(store_err)? {
            Some(z) => z,
            None => return Ok(None),
        };
        // Secondaries are accepted on the wire model but not transferred yet.
        // Keep them Pending with an honest message; never Ready, never served.
        if matches!(zone.spec.kind, ZoneKind::Secondary { .. }) {
            if zone.status.phase == Phase::Pending
                && zone.status.message == SECONDARY_PENDING_MSG
                && zone.status.serial == 0
                && zone.status.records == 0
            {
                return Ok(Some(zone));
            }
            let updated = self
                .zones()
                .modify(id, |z| {
                    z.status.phase = Phase::Pending;
                    z.status.serial = 0;
                    z.status.records = 0;
                    z.status.digest.clear();
                    z.status.message = SECONDARY_PENDING_MSG.into();
                    Ok(())
                })
                .map_err(store_err)?;
            return Ok(Some(updated));
        }
        let sets = self.recordsets_in_zone(id)?;
        let digest = content_digest(&zone, &sets);
        if zone.status.digest == digest && zone.status.phase == Phase::Ready {
            return Ok(Some(zone));
        }
        let next_serial = zone.status.serial.wrapping_add(1).max(1);
        let updated = self
            .zones()
            .modify(id, |z| {
                z.status.digest = digest.clone();
                z.status.serial = next_serial;
                z.status.records = sets.len() as u32;
                z.status.phase = Phase::Ready;
                z.status.message.clear();
                Ok(())
            })
            .map_err(store_err)?;
        for rs in sets {
            if rs.status.phase != Phase::Ready {
                let _ = self.recordsets().modify(&rs.meta.id, |r| {
                    r.status.phase = Phase::Ready;
                    r.status.serial = next_serial;
                    Ok(())
                });
            }
        }
        Ok(Some(updated))
    }

    pub fn reconcile_all(&self) -> ApiResult<()> {
        for z in self.zones().list(None).map_err(store_err)? {
            if let Err(e) = self.reconcile_zone(&z.meta.id) {
                tracing::warn!(zone = %z.spec.name, error = %e, "beacon: reconcile failed");
            }
        }
        Ok(())
    }
}

/// Resource name for an auto-created set: `web.example.cloud.-A` is not a
/// valid name (no `/` is the only rule, but keep it readable).
fn set_name(spec: &RecordSetSpec) -> String {
    format!(
        "{}-{}",
        spec.name.trim_end_matches('.'),
        spec.kind.as_str().to_ascii_lowercase()
    )
}

/// `(ttl, sorted records)` of one set, as hashed into the zone digest.
type SetShape = (Option<u32>, Vec<String>);

/// Stable fingerprint of everything that ends up on the wire: zone spec plus
/// every set's owner, type, ttl and records, in a canonical order.
fn content_digest(zone: &Zone, sets: &[RecordSet]) -> String {
    use std::hash::{Hash, Hasher};
    let mut ordered: BTreeMap<(String, &str), SetShape> = BTreeMap::new();
    for s in sets {
        let mut recs = s.spec.records.clone();
        recs.sort();
        ordered.insert(
            (s.spec.name.clone(), s.spec.kind.as_str()),
            (s.spec.ttl, recs),
        );
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    zone.spec.name.hash(&mut h);
    zone.spec.ttl.hash(&mut h);
    zone.spec.email.hash(&mut h);
    zone.nameservers().hash(&mut h);
    ordered.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn require_primary(zone: &Zone) -> ApiResult<()> {
    if matches!(zone.spec.kind, ZoneKind::Primary) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "record sets belong on primary zones; secondaries are transfer-only",
        ))
    }
}

/// A project resource may only reference resources in its own project or
/// cloud-wide (project `None`) ones.
fn check_same_project(mine: &Meta, theirs: &Meta, what: &str) -> ApiResult<()> {
    match (&mine.project, &theirs.project) {
        (_, None) => Ok(()),
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(ApiError::forbidden(format!(
            "{what} {} belongs to another project",
            theirs.id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(Store::in_memory().unwrap())
    }

    fn zone(c: &Client, name: &str) -> Zone {
        c.create_zone(
            Meta::new(kind::ZONE, name.trim_end_matches('.'), None),
            ZoneSpec {
                name: name.into(),
                ..ZoneSpec::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn cname_exclusivity_and_uniqueness() {
        let c = client();
        let z = zone(&c, "example.cloud.");
        let a = |name: &str, k: RecordType, recs: &[&str]| RecordSetSpec {
            zone: z.meta.id.clone(),
            name: name.into(),
            kind: k,
            ttl: None,
            records: recs.iter().map(|s| s.to_string()).collect(),
        };
        c.create_recordset(
            Meta::new(kind::RECORD, "web-a", None),
            a("web", RecordType::A, &["10.0.0.4"]),
        )
        .unwrap();
        // same owner+type twice
        let err = c
            .create_recordset(
                Meta::new(kind::RECORD, "web-a2", None),
                a("web", RecordType::A, &["10.0.0.5"]),
            )
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
        // CNAME next to an A
        let err = c
            .create_recordset(
                Meta::new(kind::RECORD, "web-c", None),
                a("web", RecordType::Cname, &["other.example.cloud."]),
            )
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
        // A next to a CNAME
        c.create_recordset(
            Meta::new(kind::RECORD, "www-c", None),
            a("www", RecordType::Cname, &["web.example.cloud."]),
        )
        .unwrap();
        assert!(c
            .create_recordset(
                Meta::new(kind::RECORD, "www-txt", None),
                a("www", RecordType::Txt, &["x"])
            )
            .is_err());
        // Different type at same owner is fine.
        c.create_recordset(
            Meta::new(kind::RECORD, "web-txt", None),
            a("web", RecordType::Txt, &["x"]),
        )
        .unwrap();
    }

    #[test]
    fn upsert_is_idempotent_and_serial_moves_only_on_change() {
        let c = client();
        let z = zone(&c, "example.cloud.");
        let first = c
            .upsert_record(
                &z.meta.id,
                "web",
                RecordType::A,
                None,
                vec!["10.0.0.4".into()],
                "test",
            )
            .unwrap();
        assert_eq!(first.meta.labels.get(MANAGED_LABEL).unwrap(), "test");
        let again = c
            .upsert_record(
                &z.meta.id,
                "web",
                RecordType::A,
                None,
                vec!["10.0.0.4".into()],
                "test",
            )
            .unwrap();
        assert_eq!(again.meta.revision, first.meta.revision);

        let z1 = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
        assert_eq!(z1.status.serial, 1);
        assert_eq!(z1.status.records, 1);
        let z2 = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
        assert_eq!(z2.status.serial, 1);

        let changed = c
            .upsert_record(
                &z.meta.id,
                "web",
                RecordType::A,
                None,
                vec!["10.0.0.5".into()],
                "test",
            )
            .unwrap();
        assert_eq!(changed.meta.id, first.meta.id);
        assert_eq!(
            c.reconcile_zone(&z.meta.id).unwrap().unwrap().status.serial,
            2
        );

        assert!(c
            .remove_record(&z.meta.id, "web", RecordType::A)
            .unwrap()
            .is_some());
        assert!(c
            .remove_record(&z.meta.id, "web", RecordType::A)
            .unwrap()
            .is_none());
        assert_eq!(
            c.reconcile_zone(&z.meta.id).unwrap().unwrap().status.serial,
            3
        );
    }

    #[test]
    fn secondary_stays_pending_and_rejects_records() {
        let c = client();
        let z = c
            .create_zone(
                Meta::new(kind::ZONE, "sec", None),
                ZoneSpec {
                    name: "sec.cloud.".into(),
                    kind: ZoneKind::Secondary {
                        primaries: vec!["10.0.0.1:53".into()],
                    },
                    ..ZoneSpec::default()
                },
            )
            .unwrap();
        let out = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
        assert_eq!(out.status.phase, Phase::Pending);
        assert_eq!(out.status.message, SECONDARY_PENDING_MSG);
        assert_eq!(out.status.serial, 0);
        assert!(c
            .create_recordset(
                Meta::new(kind::RECORD, "a", None),
                RecordSetSpec {
                    zone: z.meta.id.clone(),
                    name: "web".into(),
                    kind: RecordType::A,
                    ttl: None,
                    records: vec!["10.0.0.4".into()],
                },
            )
            .is_err());
        // Idempotent reconcile.
        let again = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
        assert_eq!(again.status.phase, Phase::Pending);
        assert_eq!(again.meta.revision, out.meta.revision);
    }

    #[test]
    fn zone_lookup_prefers_longest_apex() {
        let c = client();
        let parent = zone(&c, "cloud.");
        let child = zone(&c, "example.cloud.");
        assert_eq!(
            c.zone_for_name("web.example.cloud.")
                .unwrap()
                .unwrap()
                .meta
                .id,
            child.meta.id
        );
        assert_eq!(
            c.zone_for_name("x.cloud.").unwrap().unwrap().meta.id,
            parent.meta.id
        );
        assert!(c.zone_for_name("other.").unwrap().is_none());
        // duplicate apex refused
        assert!(c
            .create_zone(
                Meta::new(kind::ZONE, "dup", None),
                ZoneSpec {
                    name: "Example.Cloud".into(),
                    ..ZoneSpec::default()
                }
            )
            .is_err());
        // delete cascades
        c.upsert_record(
            &child.meta.id,
            "web",
            RecordType::A,
            None,
            vec!["10.0.0.4".into()],
            "t",
        )
        .unwrap();
        c.delete_zone(&child.meta.id).unwrap();
        assert!(c.recordsets().list(None).unwrap().is_empty());
    }
}

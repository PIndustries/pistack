// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_store::{Collection, Store};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};

use super::check_same_project::check_same_project;
use super::content_digest::content_digest;
use super::require_primary::require_primary;
use super::secondary_pending_msg::SECONDARY_PENDING_MSG;
use super::set_name::set_name;
use super::store_err::store_err;

#[derive(Clone)]
pub struct Client {
    pub(crate) store: Store,
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

    /// Zone by apex name (any project). A zone served as primary is preferred
    /// when the apex is held both ways, so callers that mean "the zone we
    /// serve" — lookups, export, an auto record — never land on a secondary
    /// copy of the same name.
    pub fn zone_by_apex(&self, apex: &str) -> ApiResult<Option<Zone>> {
        Ok(self.zones_by_apex(apex)?.into_iter().next())
    }

    /// Every zone held for `apex`, primary first. A notify pass needs the
    /// secondary entries too: they are the list of whom to tell.
    pub fn zones_by_apex(&self, apex: &str) -> ApiResult<Vec<Zone>> {
        let apex = normalize_name(apex)?;
        let mut found: Vec<Zone> = self
            .zones()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|z| z.spec.name == apex)
            .collect();
        found.sort_by_key(|z| !matches!(z.spec.kind, ZoneKind::Primary));
        Ok(found)
    }

    // ---- zones ------------------------------------------------------------

    pub fn create_zone(&self, mut meta: Meta, mut spec: ZoneSpec) -> ApiResult<Zone> {
        meta.id = Id::new(kind::ZONE);
        validate_zone(&mut spec)?;
        // One primary per apex, and at most one secondary: a node holds a zone
        // one way or the other. A secondary next to a primary of the same name
        // is the shape of a node that serves a zone and keeps a pulled copy of
        // someone else's — which is exactly what NOTIFY has to be able to see.
        let held = self.zones_by_apex(&spec.name)?;
        if let Some(other) = held
            .iter()
            .find(|z| std::mem::discriminant(&z.spec.kind) == std::mem::discriminant(&spec.kind))
        {
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
    pub(crate) fn check_exclusive(
        &self,
        zone: &Zone,
        spec: &RecordSetSpec,
        me: Option<&Id>,
    ) -> ApiResult<()> {
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
    ///
    /// Serial movement is what a NOTIFY and an AXFR both compare against, and
    /// what a secondary's pull is told to fetch, so a zone whose stored serial
    /// does not yet describe its stored content is not up to date: the first
    /// pass after any write bumps.
    pub fn reconcile_zone(&self, id: &Id) -> ApiResult<Option<Zone>> {
        let zone = match self.zones().try_get(id).map_err(store_err)? {
            Some(z) => z,
            None => return Ok(None),
        };
        // Secondaries are owned by the AXFR transfer task. Seed Pending once;
        // never clobber Ready/Error that transfer already wrote.
        if matches!(zone.spec.kind, ZoneKind::Secondary { .. }) {
            if zone.status.phase == Phase::Pending
                && zone.status.message.is_empty()
                && zone.status.serial == 0
            {
                let updated = self
                    .zones()
                    .modify(id, |z| {
                        z.status.message = SECONDARY_PENDING_MSG.into();
                        Ok(())
                    })
                    .map_err(store_err)?;
                return Ok(Some(updated));
            }
            return Ok(Some(zone));
        }
        let sets = self.recordsets_in_zone(id)?;
        let digest = content_digest(&zone, &sets);
        // "Ready" means the stored serial describes the stored content. A
        // second write inside the reconciler's coalescing window still has to
        // move the serial, or the peer a NOTIFY names would be told a serial
        // that does not include it.
        if zone.status.digest == digest
            && zone.status.phase == Phase::Ready
            && zone.status.serial > 0
        {
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

    /// Secondary zones known to the store.
    pub fn secondary_zones(&self) -> ApiResult<Vec<Zone>> {
        Ok(self
            .zones()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|z| matches!(z.spec.kind, ZoneKind::Secondary { .. }))
            .collect())
    }

    /// Mark a secondary Ready after a successful AXFR.
    pub fn secondary_ready(
        &self,
        id: &Id,
        serial: u32,
        records: u32,
        digest: String,
        from: &str,
    ) -> ApiResult<Zone> {
        self.zones()
            .modify(id, |z| {
                z.status.phase = Phase::Ready;
                z.status.serial = serial;
                z.status.records = records;
                z.status.digest = digest.clone();
                z.status.message = format!("transferred from {from}");
                Ok(())
            })
            .map_err(store_err)
    }

    /// Mark a secondary Error after every primary failed.
    pub fn secondary_failed(&self, id: &Id, err: &str) -> ApiResult<Zone> {
        self.zones()
            .modify(id, |z| {
                // Keep last good serial/records if we had a prior transfer.
                z.status.phase = if z.status.serial > 0 {
                    Phase::Ready
                } else {
                    Phase::Error
                };
                z.status.message = format!("AXFR failed: {err}");
                Ok(())
            })
            .map_err(store_err)
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

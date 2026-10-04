// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client` is slab's core: every state change goes through it, from the
//! HTTP handlers, the reconciler, and other modules (`forge` attaches
//! volumes through it). It is the only writer of `status`.

use pistack_rpc::{CommandKind, CommandQueue, VolumeAttach, VolumeAttached, VolumeDetach};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{ApiError, ApiResult, Id, Meta, Phase};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{AttachInfo, AttachTarget, BlockBackend, LocalFileBackend};
use crate::place;
use crate::quota::{AllowAll, QuotaCheck, QuotaRequest};
use crate::types::*;

/// Body of `POST /volumes`, `POST /snapshots`, `POST /backups`,
/// `POST /volume-types`: common metadata plus the kind's spec.
#[derive(Clone, Debug, Deserialize)]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create on behalf of a project; others get their own.
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: S,
}

/// Body of `PUT /<kind>/{id}`: metadata only. `spec` changes have their
/// own verbs (`/extend`) because they need backend work.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateMeta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
}

impl UpdateMeta {
    fn apply(&self, m: &mut Meta) {
        if let Some(n) = &self.name {
            m.name = n.clone();
        }
        if let Some(d) = &self.description {
            m.description = d.clone();
        }
        if let Some(l) = &self.labels {
            m.labels = l.clone();
        }
        if let Some(a) = &self.annotations {
            m.annotations = a.clone();
        }
    }
}

#[derive(Clone)]
pub struct Client {
    store: Store,
    backend: Arc<dyn BlockBackend>,
    quota: Arc<dyn QuotaCheck>,
    /// Set on the server so attach/detach hotplug through the agent.
    /// Absent in unit tests, which only record the attachment.
    queue: Option<CommandQueue>,
}

fn local_path(info: &AttachInfo) -> ApiResult<String> {
    match &info.target {
        AttachTarget::Local { path } if !path.is_empty() => Ok(path.clone()),
        _ => Err(ApiError::unprocessable(
            "only a local file volume can be attached to a KVM guest",
        )),
    }
}

fn validate_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 128 {
        return Err(ApiError::bad_request("name must be 1-128 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::bad_request(
            "name may contain only letters, digits, '-', '_' and '.'",
        ));
    }
    Ok(())
}

impl Client {
    pub fn new(store: Store, backend: Arc<dyn BlockBackend>, quota: Arc<dyn QuotaCheck>) -> Self {
        Self {
            store,
            backend,
            quota,
            queue: None,
        }
    }

    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    pub fn has_queue(&self) -> bool {
        self.queue.is_some()
    }

    /// In-memory store, local file backend under `dir`, no quotas. For tests
    /// and the dev loop.
    pub fn local(dir: impl Into<std::path::PathBuf>) -> ApiResult<Self> {
        let store = Store::in_memory()?;
        Ok(Self::new(
            store,
            Arc::new(LocalFileBackend::new(dir)),
            Arc::new(AllowAll),
        ))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn backend(&self) -> &Arc<dyn BlockBackend> {
        &self.backend
    }
    pub fn volume_types(&self) -> Collection<VolumeType> {
        self.store.collection()
    }
    pub fn volumes(&self) -> Collection<Volume> {
        self.store.collection()
    }
    pub fn snapshots(&self) -> Collection<Snapshot> {
        self.store.collection()
    }
    pub fn backups(&self) -> Collection<Backup> {
        self.store.collection()
    }

    /// Create the `standard` volume type on the configured backend if no
    /// volume type exists yet. Idempotent.
    pub fn seed_defaults(&self) -> ApiResult<()> {
        let col = self.volume_types();
        if col.get_by_name(None, DEFAULT_VOLUME_TYPE).is_ok() {
            return Ok(());
        }
        let mut meta = Meta::new(KIND_VOLUME_TYPE, DEFAULT_VOLUME_TYPE, None);
        meta.description = format!("default volume type on the {} backend", self.backend.name());
        match col.create(VolumeType {
            meta,
            spec: VolumeTypeSpec {
                backend: self.backend.name().to_owned(),
                params: BTreeMap::new(),
            },
            status: VolumeTypeStatus::default(),
        }) {
            Ok(_) | Err(StoreError::Conflict(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    // ---- volume types -----------------------------------------------------

    pub fn create_volume_type(&self, req: Create<VolumeTypeSpec>) -> ApiResult<VolumeType> {
        validate_name(&req.name)?;
        if req.spec.backend.is_empty() {
            return Err(ApiError::bad_request("spec.backend is required"));
        }
        let mut meta = Meta::new(KIND_VOLUME_TYPE, req.name, None);
        meta.description = req.description;
        meta.labels = req.labels;
        meta.annotations = req.annotations;
        Ok(self.volume_types().create(VolumeType {
            meta,
            spec: req.spec,
            status: VolumeTypeStatus::default(),
        })?)
    }

    pub fn get_volume_type(&self, name_or_id: &str) -> ApiResult<VolumeType> {
        Ok(self.volume_types().resolve(None, name_or_id)?)
    }

    pub fn list_volume_types(&self) -> ApiResult<Vec<VolumeType>> {
        Ok(self.volume_types().list(None)?)
    }

    pub fn update_volume_type(
        &self,
        id: &Id,
        meta: UpdateMeta,
        spec: Option<VolumeTypeSpec>,
        expected_revision: Option<u64>,
    ) -> ApiResult<VolumeType> {
        if let Some(n) = &meta.name {
            validate_name(n)?;
        }
        let mut cur = self.volume_types().get(id)?;
        meta.apply(&mut cur.meta);
        if let Some(s) = spec {
            cur.spec = s;
        }
        Ok(self.volume_types().update(cur, expected_revision)?)
    }

    pub fn delete_volume_type(&self, id: &Id) -> ApiResult<VolumeType> {
        let vt = self.volume_types().get(id)?;
        let in_use = self.volumes().list(None)?.iter().any(|v| {
            v.spec.volume_type == vt.meta.name || v.spec.volume_type == vt.meta.id.to_string()
        });
        if in_use {
            return Err(ApiError::conflict(format!(
                "volume type '{}' is in use by existing volumes",
                vt.meta.name
            )));
        }
        Ok(self.volume_types().delete(id)?)
    }

    // ---- volumes ----------------------------------------------------------

    pub async fn create_volume(&self, req: Create<VolumeSpec>) -> ApiResult<Volume> {
        validate_name(&req.name)?;
        let mut spec = req.spec;
        if spec.size_gib == 0 {
            return Err(ApiError::bad_request("spec.size_gib must be at least 1"));
        }
        // Normalise the type to its name so renames do not break volumes.
        let vt = self.get_volume_type(&spec.volume_type).map_err(|_| {
            ApiError::bad_request(format!("unknown volume type '{}'", spec.volume_type))
        })?;
        if vt.spec.backend != self.backend.name() {
            return Err(ApiError::unprocessable(format!(
                "volume type '{}' uses backend '{}' but this server runs '{}'",
                vt.meta.name,
                vt.spec.backend,
                self.backend.name()
            )));
        }
        spec.volume_type = vt.meta.name.clone();

        // Source checks: sizes must not shrink and sources must be ready.
        match &spec.source {
            VolumeSource::None | VolumeSource::Image(_) => {}
            VolumeSource::Snapshot(id) => {
                let snap = self.snapshots().get(id)?;
                if snap.status.phase != Phase::Ready {
                    return Err(ApiError::conflict(format!("snapshot {id} is not ready")));
                }
                if spec.size_gib < snap.status.size_gib {
                    return Err(ApiError::unprocessable(format!(
                        "size_gib {} is smaller than snapshot ({} GiB)",
                        spec.size_gib, snap.status.size_gib
                    )));
                }
            }
            VolumeSource::Volume(id) => {
                let src = self.volumes().get(id)?;
                if src.status.phase != Phase::Ready {
                    return Err(ApiError::conflict(format!("volume {id} is not ready")));
                }
                if spec.size_gib < src.spec.size_gib {
                    return Err(ApiError::unprocessable(format!(
                        "size_gib {} is smaller than source volume ({} GiB)",
                        spec.size_gib, src.spec.size_gib
                    )));
                }
            }
        }

        self.quota
            .check(
                req.project.as_ref(),
                &[
                    QuotaRequest::volumes(1),
                    QuotaRequest::gigabytes(spec.size_gib),
                ],
            )
            .await?;
        crate::usage::admit_create(self.store(), req.project.as_ref())?;

        let mut meta = Meta::new(KIND_VOLUME, req.name, req.project);
        meta.description = req.description;
        meta.labels = req.labels;
        meta.annotations = req.annotations;
        let vol = self.volumes().create(Volume {
            meta,
            spec,
            status: VolumeStatus {
                phase: Phase::Pending,
                ..Default::default()
            },
        })?;
        crate::usage::note_create(self.store(), vol.meta.project.as_ref());
        Ok(vol)
    }

    pub fn get_volume(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Volume> {
        Ok(self.volumes().resolve(project, name_or_id)?)
    }

    pub fn list_volumes(&self, project: Option<&Id>) -> ApiResult<Vec<Volume>> {
        Ok(self.volumes().list(project)?)
    }

    pub fn update_volume(
        &self,
        id: &Id,
        meta: UpdateMeta,
        expected_revision: Option<u64>,
    ) -> ApiResult<Volume> {
        if let Some(n) = &meta.name {
            validate_name(n)?;
        }
        let mut cur = self.volumes().get(id)?;
        meta.apply(&mut cur.meta);
        Ok(self.volumes().update(cur, expected_revision)?)
    }

    /// Mark a volume for deletion; the reconciler frees storage and removes
    /// the record. Attached volumes are refused.
    pub fn request_delete_volume(&self, id: &Id) -> ApiResult<Volume> {
        let snaps = self.snapshots().list(None)?;
        if snaps
            .iter()
            .any(|s| &s.spec.volume == id && s.status.phase != Phase::Deleting)
        {
            return Err(ApiError::conflict(format!(
                "volume {id} has snapshots; delete them first"
            )));
        }
        let mut release = false;
        let vol = self.volumes().modify(id, |v| {
            if v.status.attached_to.is_some() {
                return Err(StoreError::Conflict(format!(
                    "volume {id} is attached to {}",
                    v.status
                        .attached_to
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_default()
                )));
            }
            release = crate::usage::claim_release(v);
            Ok(())
        })?;
        if release {
            crate::usage::note_delete(self.store(), vol.meta.project.as_ref());
        }
        Ok(vol)
    }

    /// Record that `machine` holds the volume as `device`. `forge` calls this
    /// once `anvil` reports the attach done.
    pub fn mark_attached(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<Volume> {
        if let Some(d) = &device {
            if d.is_empty() || d.contains('/') || d.len() > 32 {
                return Err(ApiError::bad_request(
                    "device must be a bare name like 'vdb'",
                ));
            }
        }
        Ok(self.volumes().modify(id, |v| {
            if v.status.phase != Phase::Ready {
                return Err(StoreError::Conflict(format!(
                    "volume {id} is {:?}, not ready",
                    v.status.phase
                )));
            }
            if let Some(cur) = &v.status.attached_to {
                if cur != machine {
                    return Err(StoreError::Conflict(format!(
                        "volume {id} is already attached to {cur}"
                    )));
                }
            }
            v.status.attached_to = Some(machine.clone());
            v.status.device = device.clone();
            set_condition(
                &mut v.status.conditions,
                "Attached",
                true,
                "attached",
                machine.to_string(),
            );
            Ok(())
        })?)
    }

    pub fn mark_detached(&self, id: &Id) -> ApiResult<Volume> {
        Ok(self.volumes().modify(id, |v| {
            v.status.attached_to = None;
            v.status.device = None;
            set_condition(&mut v.status.conditions, "Attached", false, "detached", "");
            Ok(())
        })?)
    }

    /// Ask the host that runs `machine` to hotplug this volume. Returns the
    /// guest device name. The volume must be a local file.
    pub async fn plug_volume(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<String> {
        let queue = self.queue.clone().ok_or_else(|| {
            ApiError::not_implemented("volume attach has no host queue on this process")
        })?;
        let vol = self.ready_volume(id).await?;
        let node = place::node_for_machine(&self.store, machine)?;
        let info = self.backend.attach_info(&vol, &node).await?;
        let path = local_path(&info)?;
        let req = VolumeAttach {
            machine: machine.clone(),
            volume: id.clone(),
            path,
            device,
            read_only: info.read_only,
        };
        let payload = serde_json::to_value(&req).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = queue.enqueue(&node, CommandKind::AttachVolume, payload)?;
        let attached: VolumeAttached =
            serde_json::from_value(ticket.wait().await?.into_api_result()?).map_err(|e| {
                ApiError::internal(format!("host returned a bad attach result: {e}"))
            })?;
        Ok(attached.device)
    }

    /// Ask the host to unplug the volume. A disk that is already gone is success.
    pub async fn unplug_volume(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<()> {
        let queue = self.queue.clone().ok_or_else(|| {
            ApiError::not_implemented("volume detach has no host queue on this process")
        })?;
        let vol = self.volumes().get(id)?;
        let node = place::node_for_machine(&self.store, machine)?;
        let info = self.backend.attach_info(&vol, &node).await?;
        let path = local_path(&info)?;
        let req = VolumeDetach {
            machine: machine.clone(),
            volume: id.clone(),
            path,
            device,
        };
        let payload = serde_json::to_value(&req).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = queue.enqueue(&node, CommandKind::DetachVolume, payload)?;
        ticket.wait().await?.into_api_result()?;
        Ok(())
    }

    async fn ready_volume(&self, id: &Id) -> ApiResult<Volume> {
        for _ in 0..40 {
            let vol = self.volumes().get(id)?;
            match vol.status.phase {
                Phase::Ready => return Ok(vol),
                Phase::Pending | Phase::Creating | Phase::Updating => {
                    if let Err(e) = self.reconcile_volume(id).await {
                        tracing::debug!(volume = %id, error = %e, "volume not ready yet");
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => {
                    return Err(ApiError::conflict(format!(
                        "volume {id} is {other:?}, not ready"
                    )))
                }
            }
        }
        Err(ApiError::conflict(format!(
            "volume {id} did not become ready"
        )))
    }

    /// Grow a volume. Sets `spec.size_gib` and phase `Updating`; the
    /// reconciler asks the backend and returns the volume to `Ready`.
    pub async fn extend_volume(&self, id: &Id, size_gib: u64) -> ApiResult<Volume> {
        let cur = self.volumes().get(id)?;
        if size_gib <= cur.spec.size_gib {
            return Err(ApiError::unprocessable(format!(
                "size_gib must be greater than the current {} GiB",
                cur.spec.size_gib
            )));
        }
        if cur.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "volume {id} is {:?}, not ready",
                cur.status.phase
            )));
        }
        self.quota
            .check(
                cur.meta.project.as_ref(),
                &[QuotaRequest::gigabytes(size_gib - cur.spec.size_gib)],
            )
            .await?;
        Ok(self.volumes().modify(id, |v| {
            v.spec.size_gib = size_gib;
            v.status.phase = Phase::Updating;
            Ok(())
        })?)
    }

    /// How `host` should attach the volume. Public API for `forge`.
    pub async fn attach_info(&self, volume: &Id, host: &str) -> ApiResult<AttachInfo> {
        let vol = self.volumes().get(volume)?;
        if vol.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "volume {volume} is {:?}, not ready",
                vol.status.phase
            )));
        }
        self.backend.attach_info(&vol, host).await
    }

    // ---- snapshots --------------------------------------------------------

    pub async fn create_snapshot(&self, req: Create<SnapshotSpec>) -> ApiResult<Snapshot> {
        validate_name(&req.name)?;
        let vol = self.volumes().get(&req.spec.volume)?;
        if vol.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "volume {} is {:?}, not ready",
                vol.meta.id, vol.status.phase
            )));
        }
        // A snapshot lives in its volume's project regardless of the body.
        let project = vol.meta.project.clone();
        self.quota
            .check(project.as_ref(), &[QuotaRequest::snapshots(1)])
            .await?;
        let mut meta = Meta::new(KIND_SNAPSHOT, req.name, project);
        meta.description = req.description;
        meta.labels = req.labels;
        meta.annotations = req.annotations;
        Ok(self.snapshots().create(Snapshot {
            meta,
            spec: req.spec,
            status: SnapshotStatus {
                phase: Phase::Pending,
                size_gib: vol.spec.size_gib,
                ..Default::default()
            },
        })?)
    }

    pub fn get_snapshot(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Snapshot> {
        Ok(self.snapshots().resolve(project, name_or_id)?)
    }

    pub fn list_snapshots(&self, project: Option<&Id>) -> ApiResult<Vec<Snapshot>> {
        Ok(self.snapshots().list(project)?)
    }

    pub fn request_delete_snapshot(&self, id: &Id) -> ApiResult<Snapshot> {
        Ok(self.snapshots().modify(id, |s| {
            s.status.phase = Phase::Deleting;
            Ok(())
        })?)
    }

    // ---- backups ----------------------------------------------------------

    pub async fn create_backup(&self, req: Create<BackupSpec>) -> ApiResult<Backup> {
        validate_name(&req.name)?;
        if req.spec.target.is_empty() {
            return Err(ApiError::bad_request(
                "spec.target (silo bucket) is required",
            ));
        }
        let vol = self.volumes().get(&req.spec.volume)?;
        let project = vol.meta.project.clone();
        self.quota
            .check(project.as_ref(), &[QuotaRequest::backups(1)])
            .await?;
        let mut meta = Meta::new(KIND_BACKUP, req.name, project);
        meta.description = req.description;
        meta.labels = req.labels;
        meta.annotations = req.annotations;
        Ok(self.backups().create(Backup {
            meta,
            spec: req.spec,
            status: BackupStatus {
                phase: Phase::Pending,
                size_gib: vol.spec.size_gib,
                ..Default::default()
            },
        })?)
    }

    pub fn get_backup(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Backup> {
        Ok(self.backups().resolve(project, name_or_id)?)
    }

    pub fn list_backups(&self, project: Option<&Id>) -> ApiResult<Vec<Backup>> {
        Ok(self.backups().list(project)?)
    }

    pub fn request_delete_backup(&self, id: &Id) -> ApiResult<Backup> {
        Ok(self.backups().modify(id, |b| {
            b.status.phase = Phase::Deleting;
            Ok(())
        })?)
    }

    pub fn restore_backup(&self, id: &Id) -> ApiResult<Volume> {
        let _ = self.backups().get(id)?;
        Err(ApiError::not_implemented(
            "backup restore needs silo; not implemented yet",
        ))
    }

    // ---- reconciliation ---------------------------------------------------

    /// Drive one volume towards its desired state. Returns the volume if it
    /// still exists.
    pub async fn reconcile_volume(&self, id: &Id) -> ApiResult<Option<Volume>> {
        let col = self.volumes();
        let Some(vol) = col.try_get(id)? else {
            return Ok(None);
        };
        match vol.status.phase {
            Phase::Pending => {
                let vol = col.modify(id, |v| {
                    v.status.phase = Phase::Creating;
                    Ok(())
                })?;
                match self.backend.create(&vol).await {
                    Ok(r) => Ok(Some(col.modify(id, |v| {
                        v.status.phase = Phase::Ready;
                        v.status.backend_ref = Some(r.to_string());
                        set_condition(&mut v.status.conditions, "Provisioned", true, "created", "");
                        if matches!(v.spec.source, VolumeSource::Image(_)) {
                            set_condition(
                                &mut v.status.conditions,
                                "ImageWritten",
                                false,
                                "pending",
                                "image population happens at first attach",
                            );
                        }
                        Ok(())
                    })?)),
                    Err(e) => Ok(Some(self.fail_volume(id, "create", e)?)),
                }
            }
            Phase::Updating => match self.backend.extend(&vol, vol.spec.size_gib).await {
                Ok(()) => Ok(Some(col.modify(id, |v| {
                    v.status.phase = Phase::Ready;
                    set_condition(&mut v.status.conditions, "Resized", true, "extended", "");
                    Ok(())
                })?)),
                Err(e) => Ok(Some(self.fail_volume(id, "extend", e)?)),
            },
            Phase::Deleting => match self.backend.delete(&vol).await {
                Ok(()) => {
                    col.delete(id)?;
                    Ok(None)
                }
                Err(e) => Ok(Some(self.fail_volume(id, "delete", e)?)),
            },
            Phase::Creating | Phase::Ready | Phase::Error => Ok(Some(vol)),
        }
    }

    fn fail_volume(&self, id: &Id, op: &str, e: ApiError) -> ApiResult<Volume> {
        tracing::warn!(volume = %id, op, error = %e, "volume reconcile failed");
        Ok(self.volumes().modify(id, |v| {
            v.status.phase = Phase::Error;
            set_condition(
                &mut v.status.conditions,
                "Provisioned",
                false,
                op,
                e.to_string(),
            );
            Ok(())
        })?)
    }

    pub async fn reconcile_snapshot(&self, id: &Id) -> ApiResult<Option<Snapshot>> {
        let col = self.snapshots();
        let Some(snap) = col.try_get(id)? else {
            return Ok(None);
        };
        match snap.status.phase {
            Phase::Pending => {
                let snap = col.modify(id, |s| {
                    s.status.phase = Phase::Creating;
                    Ok(())
                })?;
                let vol = self.volumes().get(&snap.spec.volume)?;
                match self.backend.snapshot(&vol, &snap).await {
                    Ok(r) => Ok(Some(col.modify(id, |s| {
                        s.status.phase = Phase::Ready;
                        s.status.backend_ref = Some(r.to_string());
                        Ok(())
                    })?)),
                    Err(e) => Ok(Some(self.fail_snapshot(id, "snapshot", e)?)),
                }
            }
            Phase::Deleting => match self.backend.delete_snapshot(&snap).await {
                Ok(()) => {
                    col.delete(id)?;
                    Ok(None)
                }
                Err(e) => Ok(Some(self.fail_snapshot(id, "delete", e)?)),
            },
            _ => Ok(Some(snap)),
        }
    }

    fn fail_snapshot(&self, id: &Id, op: &str, e: ApiError) -> ApiResult<Snapshot> {
        tracing::warn!(snapshot = %id, op, error = %e, "snapshot reconcile failed");
        Ok(self.snapshots().modify(id, |s| {
            s.status.phase = Phase::Error;
            set_condition(
                &mut s.status.conditions,
                "Provisioned",
                false,
                op,
                e.to_string(),
            );
            Ok(())
        })?)
    }

    /// Backups need `silo`; until it is wired every backup lands in `Error`
    /// with a `not_implemented` condition, and deletes just remove the record.
    pub async fn reconcile_backup(&self, id: &Id) -> ApiResult<Option<Backup>> {
        let col = self.backups();
        let Some(b) = col.try_get(id)? else {
            return Ok(None);
        };
        match b.status.phase {
            Phase::Pending | Phase::Creating => Ok(Some(col.modify(id, |b| {
                b.status.phase = Phase::Error;
                set_condition(
                    &mut b.status.conditions,
                    "Uploaded",
                    false,
                    "not_implemented",
                    "backup upload to silo is not implemented yet",
                );
                Ok(())
            })?)),
            Phase::Deleting => {
                col.delete(id)?;
                Ok(None)
            }
            _ => Ok(Some(b)),
        }
    }

    /// Reconcile everything once. Used at start-up and after a lagged event
    /// stream.
    pub async fn reconcile_all(&self) -> ApiResult<()> {
        for v in self.volumes().list(None)? {
            if let Err(e) = self.reconcile_volume(&v.meta.id).await {
                tracing::warn!(volume = %v.meta.id, error = %e, "resync failed");
            }
        }
        for s in self.snapshots().list(None)? {
            if let Err(e) = self.reconcile_snapshot(&s.meta.id).await {
                tracing::warn!(snapshot = %s.meta.id, error = %e, "resync failed");
            }
        }
        for b in self.backups().list(None)? {
            if let Err(e) = self.reconcile_backup(&b.meta.id).await {
                tracing::warn!(backup = %b.meta.id, error = %e, "resync failed");
            }
        }
        Ok(())
    }
}

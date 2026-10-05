// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client` is slab's core: every state change goes through it, from the
//! HTTP handlers, the reconciler, and other modules (`forge` attaches
//! volumes through it). It is the only writer of `status`.

use mold::ImageStore;
use pistack_rpc::{
    CommandKind, CommandQueue, DiskTarget, VolumeAttach, VolumeAttached, VolumeDetach,
};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::error::ErrorCode;
use pistack_types::{ApiError, ApiResult, Id, Meta, Phase};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::backend::{
    AttachInfo, AttachTarget, BlockBackend, LocalFileBackend, LvmBackend, RbdBackend,
};
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
    /// Directory silo uses for backup object bytes.
    silo_root: PathBuf,
    /// Mold blob store. Volume create reads an image from here.
    images: Arc<dyn ImageStore>,
    /// Temp files for stores that only offer a byte stream.
    spool_dir: PathBuf,
}

/// Host path and optional network target for one attach or detach.
///
/// A local file or an LVM device is a host path. The agent treats a path
/// under `/dev` as a block device. An RBD image is a disk target and does
/// not need a path.
fn attach_fields(info: &AttachInfo) -> ApiResult<(String, Option<DiskTarget>)> {
    match &info.target {
        AttachTarget::Local { path } if !path.is_empty() => Ok((path.clone(), None)),
        AttachTarget::Rbd {
            pool,
            image,
            monitors,
            user,
            key_secret,
        } if !pool.is_empty() && !image.is_empty() => Ok((
            String::new(),
            Some(DiskTarget::Rbd {
                pool: pool.clone(),
                image: image.clone(),
                monitors: monitors.clone(),
                user: user.clone(),
                key_secret: key_secret.clone(),
            }),
        )),
        AttachTarget::Local { .. } => Err(ApiError::unprocessable("volume has no host path")),
        AttachTarget::Rbd { .. } => Err(ApiError::unprocessable(
            "rbd volume is missing a pool or image",
        )),
        AttachTarget::Iscsi { .. } | AttachTarget::Nvme { .. } => Err(ApiError::unprocessable(
            "volume protocol cannot be attached through the host agent",
        )),
    }
}

fn queue_attach(
    machine: Id,
    volume: Id,
    device: Option<String>,
    info: &AttachInfo,
) -> ApiResult<VolumeAttach> {
    let (path, target) = attach_fields(info)?;
    Ok(VolumeAttach {
        machine,
        volume,
        path,
        device,
        read_only: info.read_only,
        target,
    })
}

fn queue_detach(
    machine: Id,
    volume: Id,
    device: Option<String>,
    info: &AttachInfo,
) -> ApiResult<VolumeDetach> {
    let (path, target) = attach_fields(info)?;
    Ok(VolumeDetach {
        machine,
        volume,
        path,
        device,
        target,
    })
}

fn validate_device(device: &str) -> ApiResult<()> {
    if device.is_empty() || device.contains('/') || device.len() > 32 {
        return Err(ApiError::bad_request(
            "device must be a bare name like 'vdb'",
        ));
    }
    Ok(())
}

/// A local file or an LVM device needs a host path. An RBD image can be a
/// VM network disk with no host map, or a pod device the agent records later.
fn backend_needs_host_path(backend: &str) -> bool {
    backend.trim() != "rbd"
}

fn condition_holds(vol: &Volume, kind: &str) -> bool {
    vol.status
        .conditions
        .iter()
        .any(|c| c.kind == kind && c.status)
}

/// `Ok(())` when `path` is an absolute file or device node on this host.
fn host_path_present(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("host path is missing".to_owned());
    }
    let p = Path::new(path);
    if path.contains('\0')
        || !p.is_absolute()
        || p.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!("host path {path} must be an absolute path"));
    }
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_dir() => Err(format!("host path {path} is a directory")),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("host path {path} does not exist"))
        }
        Err(err) => Err(format!("host path {path}: {err}")),
    }
}

fn is_rbd_host_path(path: &str) -> bool {
    let p = Path::new(path);
    path.starts_with("/dev/rbd")
        && !path.contains('\0')
        && p.is_absolute()
        && !p.components().any(|c| matches!(c, Component::ParentDir))
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
            silo_root: silo::Silo::data_root(),
            images: Arc::new(mold::LocalDirStore::new(mold::default_image_dir())),
            spool_dir: crate::default_spool_dir(),
        }
    }

    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Where backup objects are written. Tests point this at a temp directory.
    pub fn with_silo_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.silo_root = root.into();
        self
    }

    /// Blob store shared with mold.
    pub fn with_images(mut self, images: Arc<dyn ImageStore>) -> Self {
        self.images = images;
        self
    }

    /// Directory for a streamed image copy. Removed after the copy.
    pub fn with_spool_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spool_dir = dir.into();
        self
    }

    pub(crate) fn mold(&self) -> mold::Client {
        mold::Client::new(self.store.clone(), self.images.clone())
    }

    pub fn silo_root(&self) -> &Path {
        &self.silo_root
    }

    pub fn has_queue(&self) -> bool {
        self.queue.is_some()
    }

    /// In-memory store, local file backend under `dir`, no quotas. For tests
    /// and the dev loop.
    pub fn local(dir: impl Into<PathBuf>) -> ApiResult<Self> {
        let dir = dir.into();
        let silo_root = dir.join("silo");
        let store = Store::in_memory()?;
        Ok(Self::new(
            store,
            Arc::new(LocalFileBackend::new(dir.clone())),
            Arc::new(AllowAll),
        )
        .with_silo_root(silo_root)
        .with_images(Arc::new(mold::LocalDirStore::new(dir.join("images"))))
        .with_spool_dir(dir.join("spool")))
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

    pub fn create_volume_type(&self, mut req: Create<VolumeTypeSpec>) -> ApiResult<VolumeType> {
        validate_name(&req.name)?;
        req.spec = normalize_backend(req.spec)?;
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
            cur.spec = normalize_backend(s)?;
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
        // Normalise the type to its name so renames do not break volumes.
        let vt = self.get_volume_type(&spec.volume_type).map_err(|_| {
            ApiError::bad_request(format!("unknown volume type '{}'", spec.volume_type))
        })?;
        // Reject a backend this process cannot select. Empty and local stay
        // on the file backend. lvm and rbd are checked again when they run.
        known_backend(&vt.spec.backend)?;
        spec.volume_type = vt.meta.name.clone();

        if let VolumeSource::Image(id) = &spec.source {
            let checked = crate::image::check_image(&self.mold(), id, spec.size_gib).await?;
            spec.size_gib = checked.size_gib;
        }
        if spec.size_gib == 0 {
            return Err(ApiError::bad_request("spec.size_gib must be at least 1"));
        }

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

    /// Record that `machine` is the attach target. Same as [`Self::begin_attach`].
    ///
    /// Does not report the volume attached. [`Self::confirm_attach`] does
    /// that after the host path is real.
    pub fn mark_attached(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<Volume> {
        self.begin_attach(id, machine, device)
    }

    /// Record the attach request.
    ///
    /// Sets `attached_to` and `Attached=false` with reason `Attaching`.
    /// A second attach is rejected while that is set, including another
    /// request for the same machine. An attach that is already confirmed
    /// for `machine` is left as it is.
    pub fn begin_attach(&self, id: &Id, machine: &Id, device: Option<String>) -> ApiResult<Volume> {
        if let Some(d) = &device {
            validate_device(d)?;
        }
        let node = place::node_for_machine(&self.store, machine).ok();
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
                if condition_holds(v, "Attached") {
                    return Ok(());
                }
                return Err(StoreError::Conflict(format!(
                    "volume {id} is attaching to {cur}"
                )));
            }
            v.status.attached_to = Some(machine.clone());
            v.status.device = device.clone();
            v.status.host_path = None;
            v.status.attach_node = node.clone();
            set_condition(
                &mut v.status.conditions,
                "Attached",
                false,
                "Attaching",
                machine.to_string(),
            );
            Ok(())
        })?)
    }

    /// Record a confirmed attach.
    ///
    /// A non-empty `host_path` must exist on this host (a file or a device
    /// node). An empty path is `AttachFailed` for a local or LVM volume.
    /// An RBD volume may confirm with no path: that is a VM network disk.
    /// A missing path clears `attached_to` and returns the error.
    pub fn confirm_attach(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
        host_path: Option<&str>,
    ) -> ApiResult<Volume> {
        self.ensure_claimed(id, machine)?;
        if let Some(path) = host_path.filter(|p| !p.is_empty()) {
            if let Err(msg) = host_path_present(path) {
                self.record_failure(id, machine, &msg)?;
                return Err(ApiError::unprocessable(msg));
            }
            return self.write_attached(id, machine, device, Some(path.to_owned()));
        }
        if self.volume_needs_host_path(id)? {
            let msg = "host path is missing";
            self.record_failure(id, machine, msg)?;
            return Err(ApiError::unprocessable(msg));
        }
        self.write_attached(id, machine, device, None)
    }

    /// Record the path the agent already verified on its own host.
    ///
    /// This process may not be able to see an LVM node or an RBD map, so
    /// the path is stored without a local stat. An empty path is still
    /// `AttachFailed` when the volume needs a host path.
    fn confirm_from_agent(&self, id: &Id, machine: &Id, done: VolumeAttached) -> ApiResult<Volume> {
        self.ensure_claimed(id, machine)?;
        let path = done.host_path.filter(|p| !p.is_empty());
        if path.is_none() && self.volume_needs_host_path(id)? {
            let msg = "host path is missing";
            self.record_failure(id, machine, msg)?;
            return Err(ApiError::unprocessable(msg));
        }
        let device = if done.device.is_empty() {
            None
        } else {
            Some(done.device)
        };
        self.write_attached(id, machine, device, path)
    }

    fn ensure_claimed(&self, id: &Id, machine: &Id) -> ApiResult<Volume> {
        let vol = self.volumes().get(id)?;
        match &vol.status.attached_to {
            Some(cur) if cur == machine => Ok(vol),
            Some(cur) => Err(ApiError::conflict(format!(
                "volume {id} is already attached to {cur}"
            ))),
            None => Err(ApiError::conflict(format!("volume {id} is not attaching"))),
        }
    }

    fn volume_needs_host_path(&self, id: &Id) -> ApiResult<bool> {
        let vol = self.volumes().get(id)?;
        let vt = self.get_volume_type(&vol.spec.volume_type).map_err(|_| {
            ApiError::unprocessable(format!("unknown volume type '{}'", vol.spec.volume_type))
        })?;
        Ok(backend_needs_host_path(&vt.spec.backend))
    }

    fn write_attached(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
        host_path: Option<String>,
    ) -> ApiResult<Volume> {
        if let Some(d) = &device {
            validate_device(d)?;
        }
        Ok(self.volumes().modify(id, |v| {
            match &v.status.attached_to {
                Some(cur) if cur == machine => {}
                Some(cur) => {
                    return Err(StoreError::Conflict(format!(
                        "volume {id} is already attached to {cur}"
                    )))
                }
                None => {
                    return Err(StoreError::Conflict(format!(
                        "volume {id} is not attaching"
                    )))
                }
            }
            v.status.attached_to = Some(machine.clone());
            if device.is_some() {
                v.status.device = device.clone();
            }
            v.status.host_path = host_path.clone();
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

    /// Clear the claim and set `Attached=false` with reason `AttachFailed`.
    fn record_failure(&self, id: &Id, machine: &Id, message: &str) -> ApiResult<()> {
        self.volumes().modify(id, |v| {
            if let Some(cur) = &v.status.attached_to {
                if cur != machine {
                    return Err(StoreError::Conflict(format!(
                        "volume {id} is already attached to {cur}"
                    )));
                }
            }
            v.status.attached_to = None;
            v.status.device = None;
            v.status.host_path = None;
            v.status.attach_node = None;
            set_condition(
                &mut v.status.conditions,
                "Attached",
                false,
                "AttachFailed",
                message,
            );
            Ok(())
        })?;
        Ok(())
    }

    pub fn mark_detached(&self, id: &Id) -> ApiResult<Volume> {
        Ok(self.volumes().modify(id, |v| {
            v.status.attached_to = None;
            v.status.device = None;
            v.status.host_path = None;
            v.status.attach_node = None;
            set_condition(&mut v.status.conditions, "Attached", false, "detached", "");
            Ok(())
        })?)
    }

    /// Ask the host that runs `machine` to hotplug this volume.
    ///
    /// Records `Attaching` first. The returned volume is `Attached` only
    /// after the agent confirms, and only with the host path the agent
    /// checked. A local or LVM volume whose agent result has no path
    /// becomes `AttachFailed` and the error is returned. With no host
    /// queue the volume stays `Attaching`.
    pub async fn attach_volume(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<Volume> {
        let pending = self.begin_attach(id, machine, device.clone())?;
        if condition_holds(&pending, "Attached") || !self.has_queue() {
            return Ok(pending);
        }
        match self.plug_volume(id, machine, device).await {
            Ok(done) => self.confirm_from_agent(id, machine, done),
            Err(err) => {
                self.record_failure(id, machine, &err.message)?;
                Err(err)
            }
        }
    }

    /// Ask the host that runs `machine` to hotplug this volume. Returns the
    /// agent's result. A local file or an LVM device is a host path. An
    /// RBD image is a disk target. Does not write volume status; [`Self::attach_volume`]
    /// does.
    pub async fn plug_volume(
        &self,
        id: &Id,
        machine: &Id,
        device: Option<String>,
    ) -> ApiResult<VolumeAttached> {
        let queue = self.queue.clone().ok_or_else(|| {
            ApiError::not_implemented("volume attach has no host queue on this process")
        })?;
        let vol = self.ready_volume(id).await?;
        let node = place::node_for_machine(&self.store, machine)?;
        let info = self
            .volume_backend(&vol)
            .await?
            .attach_info(&vol, &node)
            .await?;
        let req = queue_attach(machine.clone(), id.clone(), device, &info)?;
        let payload = serde_json::to_value(&req).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = queue.enqueue(&node, CommandKind::AttachVolume, payload)?;
        serde_json::from_value(ticket.wait().await?.into_api_result()?)
            .map_err(|e| ApiError::internal(format!("host returned a bad attach result: {e}")))
    }

    /// Ask the host to unplug the volume. A disk that is already gone is success.
    /// The payload uses the same path or target as attach.
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
        let info = self
            .volume_backend(&vol)
            .await?
            .attach_info(&vol, &node)
            .await?;
        let req = queue_detach(machine.clone(), id.clone(), device, &info)?;
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
        self.volume_backend(&vol)
            .await?
            .attach_info(&vol, host)
            .await
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

    /// Copy a ready backup out of silo into a new volume.
    ///
    /// The new volume is returned only after its bytes match the object.
    /// A backup that is not ready, or whose object is missing, is not restored.
    pub async fn restore_backup(&self, id: &Id) -> ApiResult<Volume> {
        let backup = self.backups().get(id)?;
        if backup.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "backup {id} is {:?}, not ready",
                backup.status.phase
            )));
        }
        let key = backup
            .status
            .object_ref
            .as_deref()
            .ok_or_else(|| ApiError::conflict(format!("backup {id} has no stored object")))?;
        let bucket = self.backup_bucket(&backup)?;
        let (obj, bytes) = self.silo().get_object(&bucket.meta.id, key).await?;
        if bytes.len() as u64 != obj.status.size_bytes || obj.status.sha256.is_empty() {
            return Err(ApiError::conflict(format!(
                "backup {id} bytes are not stored"
            )));
        }
        if bytes.len() > silo::MAX_OBJECT_BYTES {
            return Err(oversize(bytes.len() as u64));
        }
        let src = self.volumes().get(&backup.spec.volume)?;
        let backend_name = self.volume_backend(&src).await?.name();
        if backend_name != "local" {
            return Err(ApiError::unavailable(format!(
                "{backend_name} backup restore is not available"
            )));
        }
        let size_gib = src
            .spec
            .size_gib
            .max(backup.status.size_gib)
            .max(bytes_to_gib(bytes.len() as u64));
        let created = self
            .create_volume(Create {
                name: restore_volume_name(&backup),
                description: format!("restored from {}", backup.meta.name),
                labels: backup.meta.labels.clone(),
                annotations: backup.meta.annotations.clone(),
                project: backup.meta.project.clone(),
                spec: VolumeSpec {
                    size_gib,
                    volume_type: src.spec.volume_type.clone(),
                    source: VolumeSource::None,
                    bootable: src.spec.bootable,
                    encrypted: src.spec.encrypted,
                },
            })
            .await?;
        let created = self
            .reconcile_volume(&created.meta.id)
            .await?
            .ok_or_else(|| ApiError::internal("restored volume disappeared"))?;
        if created.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "restored volume {} is {:?}, not ready",
                created.meta.id, created.status.phase
            )));
        }
        // The blank file is not the restored volume. Stay off Ready until
        // the backup bytes are on the file and read back.
        self.volumes().modify(&created.meta.id, |v| {
            v.status.phase = Phase::Creating;
            Ok(())
        })?;
        let path = self.volume_file(&created).await?;
        if let Err(e) = write_prefix(&path, &bytes).await {
            let _ = self.fail_volume(&created.meta.id, "restore", e.clone());
            return Err(e);
        }
        if let Err(e) = read_prefix(&path, &bytes).await {
            let _ = self.fail_volume(&created.meta.id, "restore", e.clone());
            return Err(e);
        }
        Ok(self.volumes().modify(&created.meta.id, |v| {
            v.status.phase = Phase::Ready;
            set_condition(
                &mut v.status.conditions,
                "Restored",
                true,
                "backup",
                backup.meta.id.to_string(),
            );
            Ok(())
        })?)
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
            Phase::Pending => self.provision_volume(id, &vol, &col).await,
            Phase::Updating => {
                let backend = match self.volume_backend(&vol).await {
                    Ok(backend) => backend,
                    Err(e) => return Ok(Some(self.fail_volume(id, "extend", e)?)),
                };
                match backend.extend(&vol, vol.spec.size_gib).await {
                    Ok(()) => Ok(Some(col.modify(id, |v| {
                        v.status.phase = Phase::Ready;
                        set_condition(&mut v.status.conditions, "Resized", true, "extended", "");
                        Ok(())
                    })?)),
                    Err(e) => Ok(Some(self.fail_volume(id, "extend", e)?)),
                }
            }
            Phase::Deleting => {
                let backend = match self.volume_backend(&vol).await {
                    Ok(backend) => backend,
                    Err(_) if vol.status.backend_ref.is_none() => {
                        col.delete(id)?;
                        return Ok(None);
                    }
                    Err(e) => return Ok(Some(self.fail_volume(id, "delete", e)?)),
                };
                // Nothing was provisioned. Do not run lvremove or rbd rm.
                if vol.status.backend_ref.is_none() && backend.name() != "local" {
                    col.delete(id)?;
                    return Ok(None);
                }
                match backend.delete(&vol).await {
                    Ok(()) => {
                        col.delete(id)?;
                        Ok(None)
                    }
                    Err(e) => Ok(Some(self.fail_volume(id, "delete", e)?)),
                }
            }
            Phase::Creating | Phase::Ready | Phase::Error => Ok(Some(vol)),
        }
    }

    /// Create the blank volume, then copy an image into it while the phase
    /// stays `Creating`. A failed copy removes the blank volume.
    async fn provision_volume(
        &self,
        id: &Id,
        vol: &Volume,
        col: &Collection<Volume>,
    ) -> ApiResult<Option<Volume>> {
        let backend = match self.volume_backend(vol).await {
            Ok(backend) => backend,
            Err(e) => return Ok(Some(self.fail_volume(id, "create", e)?)),
        };
        let vol = col.modify(id, |v| {
            v.status.phase = Phase::Creating;
            Ok(())
        })?;
        let image_id = match &vol.spec.source {
            VolumeSource::Image(image) => Some(image.clone()),
            _ => None,
        };
        let staged = if let Some(image) = &image_id {
            let mold = self.mold();
            let checked = match crate::image::check_image(&mold, image, vol.spec.size_gib).await {
                Ok(checked) => checked,
                Err(e) => return Ok(Some(self.fail_volume(id, "image", e)?)),
            };
            match crate::image::stage_image(&mold, &checked.image, &self.spool_dir).await {
                Ok(staged) => Some(staged),
                Err(e) => return Ok(Some(self.fail_volume(id, "image", e)?)),
            }
        } else {
            None
        };
        let created = match backend.create(&vol).await {
            Ok(created) => created,
            Err(e) => return Ok(Some(self.fail_volume(id, "create", e)?)),
        };
        let vol = col.modify(id, |v| {
            v.status.backend_ref = Some(created.to_string());
            Ok(())
        })?;
        if let (Some(image), Some(staged)) = (&image_id, staged.as_ref()) {
            if let Err(e) = backend
                .write_image(&vol, &staged.path, &staged.format)
                .await
            {
                let e = Self::copy_failed(image, e);
                let clear_ref = match backend.delete(&vol).await {
                    Ok(()) => true,
                    Err(del) => {
                        tracing::warn!(
                            volume = %id,
                            error = %del,
                            "removing a partial volume failed"
                        );
                        false
                    }
                };
                return Ok(Some(self.mark_volume_failed(id, "image", e, clear_ref)?));
            }
        }
        drop(staged);
        Ok(Some(col.modify(id, |v| {
            v.status.phase = Phase::Ready;
            set_condition(&mut v.status.conditions, "Provisioned", true, "created", "");
            if let Some(image) = &image_id {
                set_condition(
                    &mut v.status.conditions,
                    "ImageWritten",
                    true,
                    "copied",
                    image.to_string(),
                );
            }
            Ok(())
        })?))
    }

    fn fail_volume(&self, id: &Id, op: &str, e: ApiError) -> ApiResult<Volume> {
        self.mark_volume_failed(id, op, e, false)
    }

    fn mark_volume_failed(
        &self,
        id: &Id,
        op: &str,
        e: ApiError,
        clear_ref: bool,
    ) -> ApiResult<Volume> {
        tracing::warn!(volume = %id, op, error = %e, "volume reconcile failed");
        Ok(self.volumes().modify(id, |v| {
            v.status.phase = Phase::Error;
            if clear_ref {
                v.status.backend_ref = None;
            }
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

    fn copy_failed(image: &Id, mut err: ApiError) -> ApiError {
        err.message = format!("copying image {image} failed: {}", err.message);
        err
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
                let backend = match self.volume_backend(&vol).await {
                    Ok(backend) => backend,
                    Err(e) => return Ok(Some(self.fail_snapshot(id, "snapshot", e)?)),
                };
                match backend.snapshot(&vol, &snap).await {
                    Ok(r) => Ok(Some(col.modify(id, |s| {
                        s.status.phase = Phase::Ready;
                        s.status.backend_ref = Some(r.to_string());
                        Ok(())
                    })?)),
                    Err(e) => Ok(Some(self.fail_snapshot(id, "snapshot", e)?)),
                }
            }
            Phase::Deleting => {
                let backend = match self.volumes().try_get(&snap.spec.volume)? {
                    Some(vol) => match self.volume_backend(&vol).await {
                        Ok(backend) => backend,
                        Err(_) if snap.status.backend_ref.is_none() => {
                            col.delete(id)?;
                            return Ok(None);
                        }
                        Err(e) => return Ok(Some(self.fail_snapshot(id, "delete", e)?)),
                    },
                    // The volume type left with the volume. The file backend
                    // removes only a path inside its own directory.
                    None => self.local_backend(),
                };
                // Nothing was provisioned. Do not run lvremove or rbd snap rm.
                if snap.status.backend_ref.is_none() && backend.name() != "local" {
                    col.delete(id)?;
                    return Ok(None);
                }
                match backend.delete_snapshot(&snap).await {
                    Ok(()) => {
                        col.delete(id)?;
                        Ok(None)
                    }
                    Err(e) => Ok(Some(self.fail_snapshot(id, "delete", e)?)),
                }
            }
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

    /// Copy the volume file into the backup's silo bucket.
    ///
    /// Ready is set only after the object reads back as the same bytes.
    /// A failed write leaves the backup `Pending` or `Error`. A volume
    /// larger than 1 GiB is unavailable, names the size, and is not truncated.
    pub async fn reconcile_backup(&self, id: &Id) -> ApiResult<Option<Backup>> {
        let col = self.backups();
        let Some(b) = col.try_get(id)? else {
            return Ok(None);
        };
        match b.status.phase {
            Phase::Pending | Phase::Creating => self.upload_backup(id).await,
            Phase::Deleting => self.remove_backup(id).await,
            _ => Ok(Some(b)),
        }
    }

    /// Reconcile everything once. Used at start-up and after a lagged event
    /// stream.
    pub async fn reconcile_all(&self) -> ApiResult<()> {
        if let Err(e) = self.reconcile_stale_attaches().await {
            tracing::warn!(error = %e, "slab: stale attach reconcile failed");
        }
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

    /// Drop attaches whose machine record is gone.
    ///
    /// An RBD host map on a live agent is unmapped first. When the agent or
    /// the machine placement is gone, status is cleared without a host call.
    /// Each clear is logged.
    pub async fn reconcile_stale_attaches(&self) -> ApiResult<()> {
        let volumes = self.volumes().list(None)?;
        for vol in volumes {
            let Some(machine) = vol.status.attached_to.clone() else {
                continue;
            };
            if place::machine_exists(&self.store, &machine)? {
                continue;
            }
            if let Err(e) = self.detach_stale(&vol, &machine).await {
                tracing::warn!(
                    volume = %vol.meta.id,
                    machine = %machine,
                    error = %e,
                    "slab: stale attach reconcile failed"
                );
            }
        }
        Ok(())
    }

    async fn detach_stale(&self, vol: &Volume, machine: &Id) -> ApiResult<()> {
        let host_path = vol.status.host_path.clone().unwrap_or_default();
        let node = vol.status.attach_node.clone();
        let mut unmapped = false;
        if is_rbd_host_path(&host_path) {
            if let (Some(queue), Some(node)) = (self.queue.clone(), node.clone()) {
                if agent_is_live(&queue, &node) {
                    match self.unmap_stale(vol, machine, &node).await {
                        Ok(()) => unmapped = true,
                        Err(err) => {
                            tracing::warn!(
                                volume = %vol.meta.id,
                                machine = %machine,
                                node = %node,
                                error = %err,
                                "slab: stale rbd unmap failed; will retry"
                            );
                            return Ok(());
                        }
                    }
                }
            }
        }
        self.mark_detached(&vol.meta.id)?;
        tracing::info!(
            volume = %vol.meta.id,
            machine = %machine,
            host_path = %host_path,
            unmapped,
            "slab: cleared stale attach"
        );
        Ok(())
    }

    async fn unmap_stale(&self, vol: &Volume, machine: &Id, node: &str) -> ApiResult<()> {
        let queue = self.queue.clone().ok_or_else(|| {
            ApiError::not_implemented("volume detach has no host queue on this process")
        })?;
        let req = self.stale_detach_request(vol, machine, node).await?;
        let payload = serde_json::to_value(&req).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = queue
            .enqueue(node, CommandKind::DetachVolume, payload)
            .map_err(ApiError::from)?;
        ticket
            .wait()
            .await
            .map_err(ApiError::from)?
            .into_api_result()?;
        Ok(())
    }

    async fn stale_detach_request(
        &self,
        vol: &Volume,
        machine: &Id,
        node: &str,
    ) -> ApiResult<VolumeDetach> {
        let backend = self.volume_backend(vol).await?;
        if backend.name() == "rbd" {
            if let Ok(info) = backend.attach_info(vol, node).await {
                return queue_detach(
                    machine.clone(),
                    vol.meta.id.clone(),
                    vol.status.device.clone(),
                    &info,
                );
            }
            tracing::warn!(
                volume = %vol.meta.id,
                "slab: rbd attach target is unavailable; asking the agent to unmap the recorded device"
            );
        }
        Ok(VolumeDetach {
            machine: machine.clone(),
            volume: vol.meta.id.clone(),
            path: vol.status.host_path.clone().unwrap_or_default(),
            device: vol.status.device.clone(),
            target: None,
        })
    }

    fn silo(&self) -> silo::Client {
        silo::Client::new(self.store.clone(), self.silo_root.clone())
    }

    /// Empty or `local` uses the configured file backend. `lvm` and `rbd`
    /// are built from the volume type. They still refuse to run until their
    /// own tools and pool exist.
    async fn volume_backend(&self, vol: &Volume) -> ApiResult<Arc<dyn BlockBackend>> {
        let vt = self.get_volume_type(&vol.spec.volume_type).map_err(|_| {
            ApiError::unprocessable(format!("unknown volume type '{}'", vol.spec.volume_type))
        })?;
        known_backend(&vt.spec.backend)?;
        Ok(match vt.spec.backend.trim() {
            "" | "local" => self.local_backend(),
            "lvm" => Arc::new(LvmBackend::from_params(&vt.spec.params)),
            "rbd" => Arc::new(RbdBackend::from_params(&vt.spec.params)),
            other => {
                return Err(ApiError::unprocessable(format!(
                    "volume type '{}' uses unknown backend '{other}'",
                    vt.meta.name
                )))
            }
        })
    }

    fn local_backend(&self) -> Arc<dyn BlockBackend> {
        if self.backend.name() == "local" {
            self.backend.clone()
        } else {
            Arc::new(LocalFileBackend::new(crate::default_volume_dir()))
        }
    }

    async fn volume_file(&self, vol: &Volume) -> ApiResult<PathBuf> {
        self.volume_backend(vol).await?.bytes_path(vol).await
    }

    async fn upload_backup(&self, id: &Id) -> ApiResult<Option<Backup>> {
        let backup = self.backups().get(id)?;
        let vol = match self.ready_volume(&backup.spec.volume).await {
            Ok(vol) => vol,
            Err(e) => {
                let phase = match self.volumes().try_get(&backup.spec.volume)? {
                    Some(v)
                        if matches!(
                            v.status.phase,
                            Phase::Pending | Phase::Creating | Phase::Updating
                        ) =>
                    {
                        Phase::Pending
                    }
                    _ => Phase::Error,
                };
                return self.mark_backup(id, phase, &e, true);
            }
        };
        let path = match self.volume_file(&vol).await {
            Ok(path) => path,
            Err(e) => return self.mark_backup(id, Phase::Error, &e, true),
        };
        let bytes = match read_capped(&path).await {
            Ok(bytes) => bytes,
            Err(e) => return self.mark_backup(id, Phase::Error, &e, true),
        };
        let key = backup_key(id);
        let bucket = match self.backup_bucket(&backup) {
            Ok(bucket) => bucket,
            Err(e) => return self.mark_backup(id, Phase::Error, &e, true),
        };
        if let Err(e) = self
            .silo()
            .put_object(
                &bucket.meta.id,
                &key,
                &bytes,
                Some("application/octet-stream".into()),
            )
            .await
        {
            return self.mark_backup(id, Phase::Error, &e, true);
        }
        if let Err(e) = self.confirm_object(&bucket, &key, &bytes).await {
            let _ = self.silo().delete_object(&bucket.meta.id, &key).await;
            return self.mark_backup(id, Phase::Error, &e, true);
        }
        Ok(Some(self.backups().modify(id, |b| {
            b.status.phase = Phase::Ready;
            b.status.object_ref = Some(key.clone());
            set_condition(
                &mut b.status.conditions,
                "Uploaded",
                true,
                "stored",
                format!("{} bytes", bytes.len()),
            );
            Ok(())
        })?))
    }

    async fn confirm_object(
        &self,
        bucket: &silo::Bucket,
        key: &str,
        bytes: &[u8],
    ) -> ApiResult<()> {
        let (obj, got) = self.silo().get_object(&bucket.meta.id, key).await?;
        if got != bytes
            || obj.status.size_bytes != bytes.len() as u64
            || obj.status.sha256.is_empty()
        {
            return Err(ApiError::internal("backup bytes were not stored"));
        }
        if got.len() > silo::MAX_OBJECT_BYTES {
            return Err(oversize(got.len() as u64));
        }
        Ok(())
    }

    async fn remove_backup(&self, id: &Id) -> ApiResult<Option<Backup>> {
        let backup = self.backups().get(id)?;
        if let Some(key) = backup.status.object_ref.clone() {
            match self.backup_bucket(&backup) {
                Ok(bucket) => {
                    if let Err(e) = self.silo().delete_object(&bucket.meta.id, &key).await {
                        if e.code != ErrorCode::NotFound {
                            return self.mark_backup(id, Phase::Error, &e, false);
                        }
                    }
                }
                Err(e) if e.code == ErrorCode::NotFound => {}
                Err(e) => return self.mark_backup(id, Phase::Error, &e, false),
            }
        }
        self.backups().delete(id)?;
        Ok(None)
    }

    fn backup_bucket(&self, backup: &Backup) -> ApiResult<silo::Bucket> {
        let target = backup.spec.target.trim();
        if target.is_empty() {
            return Err(ApiError::bad_request(
                "spec.target (silo bucket) is required",
            ));
        }
        let silo = self.silo();
        if let Ok(bucket) = silo.get_bucket(backup.meta.project.as_ref(), target) {
            return Ok(bucket);
        }
        if backup.meta.project.is_some() {
            if let Ok(bucket) = silo.get_bucket(None, target) {
                return Ok(bucket);
            }
        }
        if let Some(bucket) = silo
            .list_buckets(None)?
            .into_iter()
            .find(|bucket| bucket.meta.name == target || bucket.meta.id.to_string() == target)
        {
            return Ok(bucket);
        }
        Err(ApiError::not_found(format!(
            "silo bucket '{target}' not found"
        )))
    }

    /// Record a failed upload. Ready is never set here. Unavailable is
    /// returned so the caller sees the size or the missing tool.
    fn mark_backup(
        &self,
        id: &Id,
        phase: Phase,
        err: &ApiError,
        clear_ref: bool,
    ) -> ApiResult<Option<Backup>> {
        let backup = self.backups().modify(id, |b| {
            b.status.phase = phase;
            if clear_ref {
                b.status.object_ref = None;
            }
            set_condition(
                &mut b.status.conditions,
                "Uploaded",
                false,
                &err.code.to_string(),
                err.message.clone(),
            );
            Ok(())
        })?;
        if err.code == ErrorCode::Unavailable {
            return Err(err.clone());
        }
        Ok(Some(backup))
    }
}

fn agent_is_live(queue: &CommandQueue, node: &str) -> bool {
    queue.nodes().into_iter().any(|info| {
        info.name == node && (info.last_hello.is_some() || info.last_heartbeat.is_some())
    })
}

fn known_backend(name: &str) -> ApiResult<()> {
    match name.trim() {
        "" | "local" | "lvm" | "rbd" => Ok(()),
        other => Err(ApiError::bad_request(format!(
            "unknown volume backend '{other}'"
        ))),
    }
}

fn normalize_backend(mut spec: VolumeTypeSpec) -> ApiResult<VolumeTypeSpec> {
    if spec.backend.trim().is_empty() {
        spec.backend = "local".into();
    }
    known_backend(&spec.backend)?;
    Ok(spec)
}

fn backup_key(id: &Id) -> String {
    format!("backups/{id}")
}

fn restore_volume_name(backup: &Backup) -> String {
    let name = format!("restore-{}", backup.meta.id);
    if name.len() <= 128 {
        name
    } else {
        name[..128].to_owned()
    }
}

fn bytes_to_gib(bytes: u64) -> u64 {
    let gib = 1024 * 1024 * 1024;
    bytes.div_ceil(gib).max(1)
}

fn oversize(len: u64) -> ApiError {
    ApiError::unavailable(format!(
        "backup is {len} bytes, larger than {}",
        silo::format_byte_limit(silo::MAX_OBJECT_BYTES)
    ))
    .with_detail("size_bytes", serde_json::Value::from(len))
}

fn io_err(err: std::io::Error) -> ApiError {
    ApiError::internal(format!("backup: {err}"))
}

/// Read the whole file when it is at most [`silo::MAX_OBJECT_BYTES`]. A
/// larger file returns unavailable with its size and is not returned as a prefix.
async fn read_capped(path: &Path) -> ApiResult<Vec<u8>> {
    let meta = tokio::fs::metadata(path).await.map_err(io_err)?;
    if !meta.is_file() {
        return Err(ApiError::unprocessable("backup source is not a file"));
    }
    let len = meta.len();
    if len > silo::MAX_OBJECT_BYTES as u64 {
        return Err(oversize(len));
    }
    let mut file = tokio::fs::File::open(path).await.map_err(io_err)?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut chunk).await.map_err(io_err)?;
        if n == 0 {
            break;
        }
        if buf.len().saturating_add(n) > silo::MAX_OBJECT_BYTES {
            let grown = tokio::fs::metadata(path)
                .await
                .map(|m| m.len())
                .unwrap_or((buf.len() + n) as u64);
            return Err(oversize(grown));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

async fn write_prefix(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .map_err(io_err)?;
    file.seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(io_err)?;
    file.write_all(bytes).await.map_err(io_err)?;
    let len = file.metadata().await.map_err(io_err)?.len();
    if (bytes.len() as u64) > len {
        file.set_len(bytes.len() as u64).await.map_err(io_err)?;
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(io_err)?;
        file.write_all(bytes).await.map_err(io_err)?;
    }
    file.flush().await.map_err(io_err)?;
    file.sync_all().await.map_err(io_err)?;
    Ok(())
}

async fn read_prefix(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let mut file = tokio::fs::File::open(path).await.map_err(io_err)?;
    let mut got = vec![0u8; bytes.len()];
    file.read_exact(&mut got).await.map_err(io_err)?;
    if got != bytes {
        return Err(ApiError::internal(
            "restored volume bytes do not match the backup",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Id, Id) {
        (Id::new("mach"), Id::new("vol"))
    }

    #[test]
    fn local_file_and_lvm_keep_a_host_path() {
        let (machine, volume) = ids();
        let file = AttachInfo::new(AttachTarget::Local {
            path: "/var/lib/pistack/volumes/data.raw".into(),
        });
        let attach = queue_attach(machine.clone(), volume.clone(), Some("vdb".into()), &file)
            .expect("file attach");
        assert_eq!(attach.path, "/var/lib/pistack/volumes/data.raw");
        assert!(attach.target.is_none());
        assert_eq!(attach.device.as_deref(), Some("vdb"));
        assert!(!attach.read_only);
        let file_wire = serde_json::to_value(&attach).expect("json");
        assert!(file_wire.get("target").is_none());

        // Same shape LVM attach_info reports: /dev/{vg}/{id}.
        let lvm = AttachInfo::new(AttachTarget::Local {
            path: "/dev/vg0/vol-1".into(),
        });
        let attach = queue_attach(machine.clone(), volume.clone(), None, &lvm).expect("lvm attach");
        assert_eq!(attach.path, "/dev/vg0/vol-1");
        assert!(attach.target.is_none());
        let detach = queue_detach(machine, volume, Some("vdc".into()), &lvm).expect("lvm detach");
        assert_eq!(detach.path, attach.path);
        assert!(detach.target.is_none());
        assert_eq!(detach.device.as_deref(), Some("vdc"));
        let wire = serde_json::to_value(&attach).expect("json");
        assert!(wire.get("target").is_none());
        assert_eq!(wire["path"], "/dev/vg0/vol-1");
    }

    #[test]
    fn rbd_target_is_on_attach_and_detach() {
        let (machine, volume) = ids();
        let mut info = AttachInfo::new(AttachTarget::Rbd {
            pool: "volumes".into(),
            image: "vol-1".into(),
            monitors: vec!["10.1.0.5:6789".into()],
            user: "pistack".into(),
            key_secret: Some("/etc/pistack/ceph.keyring".into()),
        });
        info.read_only = true;
        let attach = queue_attach(machine.clone(), volume.clone(), Some("vdd".into()), &info)
            .expect("rbd attach");
        assert!(attach.path.is_empty());
        assert!(attach.read_only);
        assert_eq!(attach.device.as_deref(), Some("vdd"));
        match &attach.target {
            Some(DiskTarget::Rbd {
                pool,
                image,
                monitors,
                user,
                key_secret,
            }) => {
                assert_eq!(pool, "volumes");
                assert_eq!(image, "vol-1");
                assert_eq!(monitors, &vec!["10.1.0.5:6789".to_string()]);
                assert_eq!(user, "pistack");
                assert_eq!(key_secret.as_deref(), Some("/etc/pistack/ceph.keyring"));
            }
            other => panic!("expected rbd target, got {other:?}"),
        }
        let detach = queue_detach(machine, volume, Some("vdd".into()), &info).expect("rbd detach");
        assert!(detach.path.is_empty());
        assert_eq!(detach.target, attach.target);
        let wire = serde_json::to_value(&attach).expect("json");
        assert_eq!(wire["path"], "");
        assert_eq!(wire["target"]["protocol"], "rbd");
        assert_eq!(wire["target"]["pool"], "volumes");
        assert_eq!(wire["target"]["image"], "vol-1");
        assert_eq!(wire["target"]["monitors"][0], "10.1.0.5:6789");
        assert_eq!(wire["target"]["user"], "pistack");
        assert_eq!(wire["target"]["key_secret"], "/etc/pistack/ceph.keyring");
        assert_eq!(wire["read_only"], true);
    }

    #[test]
    fn empty_path_and_other_protocols_are_refused() {
        let (machine, volume) = ids();
        let empty = AttachInfo::new(AttachTarget::Local {
            path: String::new(),
        });
        let err = queue_attach(machine.clone(), volume.clone(), None, &empty).expect_err("empty");
        assert_eq!(err.code, ErrorCode::Unprocessable);

        let iscsi = AttachInfo::new(AttachTarget::Iscsi {
            portal: "10.0.0.1:3260".into(),
            iqn: "iqn.2026-01.local:vol".into(),
            lun: 0,
            auth_secret: None,
        });
        let err = queue_attach(machine.clone(), volume.clone(), None, &iscsi).expect_err("iscsi");
        assert_eq!(err.code, ErrorCode::Unprocessable);

        let nvme = AttachInfo::new(AttachTarget::Nvme {
            transport: "tcp".into(),
            address: "10.0.0.2".into(),
            port: 4420,
            nqn: "nqn.2026-01.local:vol".into(),
        });
        let err = queue_detach(machine, volume, None, &nvme).expect_err("nvme");
        assert_eq!(err.code, ErrorCode::Unprocessable);

        let blank = AttachInfo::new(AttachTarget::Rbd {
            pool: String::new(),
            image: "vol-1".into(),
            monitors: Vec::new(),
            user: String::new(),
            key_secret: None,
        });
        let err = queue_attach(Id::new("mach"), Id::new("vol"), None, &blank).expect_err("rbd");
        assert_eq!(err.code, ErrorCode::Unprocessable);
    }
}

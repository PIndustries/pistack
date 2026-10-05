// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Resource kinds owned by `slab`: volume types, volumes, snapshots, backups.

use chrono::Utc;
use pistack_types::{kind, Condition, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Kind prefixes. `vol` and `snap` are cross-module and come from
/// `pistack_types::kind`; the others are slab-private.
pub const KIND_VOLUME_TYPE: &str = "voltype";
pub const KIND_VOLUME: &str = kind::VOLUME;
pub const KIND_SNAPSHOT: &str = kind::SNAPSHOT;
pub const KIND_BACKUP: &str = "bak";

/// Name of the volume type seeded on first boot.
pub const DEFAULT_VOLUME_TYPE: &str = "standard";

/// `Resource<S, T>` is foreign and so is `Kind`, so the orphan rule stops a
/// module from writing `impl Kind for Resource<MySpec, MyStatus>`. Each kind
/// is therefore its own struct with the exact `Resource` wire shape
/// (flattened meta + spec + status) and conversions both ways.
macro_rules! resource_kind {
    ($(#[$doc:meta])* $name:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $name {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $name {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<$name> for Resource<$spec, $status> {
            fn from(r: $name) -> Self {
                Resource { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<Resource<$spec, $status>> for $name {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

// ---- VolumeType -----------------------------------------------------------

/// A volume type names a backend and the parameters it is created with
/// (pool, replication, QoS). `params` key `fs_type` is the filesystem a pod
/// attach creates when the device has none. Absent or blank means `ext4`.
/// Cloud-scoped: `meta.project` is `None`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeTypeSpec {
    /// Backend name: "local", "lvm", "rbd".
    pub backend: String,
    /// Backend settings. `fs_type` defaults to `ext4` when it is absent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeTypeStatus {}

resource_kind!(
    VolumeType,
    VolumeTypeSpec,
    VolumeTypeStatus,
    KIND_VOLUME_TYPE
);

// ---- Volume ---------------------------------------------------------------

/// Where a new volume's contents come from.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum VolumeSource {
    /// Blank, zero-filled.
    #[default]
    None,
    /// Populated from a ready `mold` image. `size_gib` of 0 uses the image
    /// virtual size rounded up to the next GiB.
    Image(Id),
    /// Restored from a snapshot of a volume with the same type.
    Snapshot(Id),
    /// Cloned from another volume.
    Volume(Id),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeSpec {
    /// Size in GiB. `0` is rejected unless `source` is an image, in which
    /// case the image's virtual size is rounded up to the next GiB.
    #[serde(default)]
    pub size_gib: u64,
    /// Volume type name or id. Defaults to `standard`.
    #[serde(default = "default_volume_type")]
    pub volume_type: String,
    #[serde(default)]
    pub source: VolumeSource,
    #[serde(default)]
    pub bootable: bool,
    #[serde(default)]
    pub encrypted: bool,
}

fn default_volume_type() -> String {
    DEFAULT_VOLUME_TYPE.to_owned()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Machine the volume is attaching to or attached to, if any.
    ///
    /// Set for the whole attach, including `Attaching`, so a second attach
    /// is rejected before the host path exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<Id>,
    /// Guest device name requested at attach, e.g. `vdb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Host path from a confirmed attach.
    ///
    /// A pod records the directory where the volume is mounted. A VM records
    /// a local file or an LVM device node. Absent for a VM network disk,
    /// which has no host map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_path: Option<String>,
    /// Agent node that accepted the attach. Used to unmap an RBD device
    /// after the machine record is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_node: Option<String>,
    /// Opaque backend handle: a file path, an LV path, an RBD image name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

resource_kind!(Volume, VolumeSpec, VolumeStatus, KIND_VOLUME);

impl Volume {
    /// Host path when the attach is confirmed and a path was recorded.
    ///
    /// `None` while the reason is `Attaching` or `AttachFailed`, after
    /// detach, and for a VM network disk that is attached with no host path.
    /// Callers that mount the volume on a pod wait until this is `Some`.
    pub fn is_attached_ready(&self) -> Option<&Path> {
        let attached = self
            .status
            .conditions
            .iter()
            .any(|c| c.kind == "Attached" && c.status);
        if !attached {
            return None;
        }
        let path = self.status.host_path.as_deref().filter(|p| !p.is_empty())?;
        Some(Path::new(path))
    }
}

// ---- Snapshot -------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotSpec {
    pub volume: Id,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Size of the source volume when the snapshot was taken.
    #[serde(default)]
    pub size_gib: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

resource_kind!(Snapshot, SnapshotSpec, SnapshotStatus, KIND_SNAPSHOT);

// ---- Backup ---------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupSpec {
    pub volume: Id,
    /// `silo` bucket the backup is written to.
    pub target: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub size_gib: u64,
    /// Byte length of the stored object. `0` on backups written as a single
    /// object before streaming uploads; restore then uses the object record.
    #[serde(default)]
    pub size_bytes: u64,
    /// Parts in the multipart upload. `0` means a single-object backup.
    #[serde(default)]
    pub part_count: u32,
    /// SHA-256 of the stored bytes, lowercase hex. Absent on older backups;
    /// restore then checks the object record's checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

resource_kind!(Backup, BackupSpec, BackupStatus, KIND_BACKUP);

// ---- helpers --------------------------------------------------------------

/// Replace-or-append a condition by type.
pub fn set_condition(
    conditions: &mut Vec<Condition>,
    kind: &str,
    status: bool,
    reason: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Some(c) = conditions.iter_mut().find(|c| c.kind == kind) {
        if c.status != status {
            c.last_transition = Utc::now();
        }
        c.status = status;
        c.reason = reason.to_owned();
        c.message = message;
        return;
    }
    conditions.push(Condition {
        kind: kind.to_owned(),
        status,
        reason: reason.to_owned(),
        message,
        last_transition: Utc::now(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_backup_status_without_multipart_fields_decodes() {
        let raw = r#"{"phase":"ready","size_gib":1,"object_ref":"backups/bak-old"}"#;
        let status: BackupStatus = serde_json::from_str(raw).unwrap();
        assert_eq!(status.phase, Phase::Ready);
        assert_eq!(status.size_gib, 1);
        assert_eq!(status.object_ref.as_deref(), Some("backups/bak-old"));
        assert_eq!(status.size_bytes, 0);
        assert_eq!(status.part_count, 0);
        assert!(status.sha256.is_none());
        assert!(status.conditions.is_empty());
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Resource kinds owned by `mold`: images and image members.

use chrono::Utc;
use pistack_types::{kind, Condition, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};

pub const KIND_IMAGE: &str = kind::IMAGE;
pub const KIND_IMAGE_MEMBER: &str = "imgmem";

/// See `slab::types` for why each kind is its own struct rather than a
/// `Resource<S, T>` alias: the orphan rule forbids `impl Kind` on the alias.
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Raw,
    Qcow2,
    /// An OCI image for containers (`loom`). Stored as a tarball.
    Oci,
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImageFormat::Raw => "raw",
            ImageFormat::Qcow2 => "qcow2",
            ImageFormat::Oci => "oci",
        })
    }
}

/// Who can see and boot an image.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Owning project only.
    #[default]
    Private,
    /// Owning project plus `ImageMember` projects.
    Project,
    /// Everyone.
    Public,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageSpec {
    pub format: ImageFormat,
    /// Free-form OS hint: "ubuntu-24.04", "rhel-9", "alpine".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default)]
    pub min_disk_gib: u64,
    #[serde(default)]
    pub min_ram_mib: u64,
    #[serde(default)]
    pub visibility: Visibility,
    /// Where `POST /images/{id}/import` fetches from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

fn default_arch() -> String {
    "x86_64".to_owned()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageStatus {
    /// `Pending`: metadata only, awaiting data. `Creating`: upload in
    /// progress. `Ready`: data stored and inspected. `Error`: rejected.
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// `ImageStore` reference, e.g. `sha256:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_ref: Option<String>,
    /// Size of the disk the image expands to (qcow2 virtual size). `None`
    /// when unknown (no `qemu-img` on the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

resource_kind!(Image, ImageSpec, ImageStatus, KIND_IMAGE);

/// Grants `project` access to `image` when its visibility is `Project`.
/// Cloud-scoped record (`meta.project` is the image owner, for listing).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMemberSpec {
    pub image: Id,
    pub project: Id,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMemberStatus {}

resource_kind!(
    ImageMember,
    ImageMemberSpec,
    ImageMemberStatus,
    KIND_IMAGE_MEMBER
);

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

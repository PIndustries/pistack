// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::proto::ImageSource;
use pistack_store::Store;
use pistack_types::Id;
use std::path::Path;
/// Catalog row, read as JSON so forge does not depend on mold.
#[derive(Clone, serde::Deserialize)]
pub(super) struct CatalogImage {
    pub(super) id: String,
    pub(super) name: String,
    /// The owning project. `None` is a cloud-wide image.
    #[serde(default)]
    pub(super) project: Option<String>,
    #[serde(default)]
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) spec: CatalogSpec,
    #[serde(default)]
    pub(super) status: CatalogStatus,
}

#[derive(Clone, serde::Deserialize, Default)]
pub(super) struct CatalogSpec {
    #[serde(default)]
    pub(super) format: String,
    /// `private` (the default), `project` or `public`.
    #[serde(default)]
    pub(super) visibility: String,
    /// Smallest disk the image is known to need.
    #[serde(default)]
    pub(super) min_disk_gib: u64,
}

/// An image member row: `project` may use `image`.
#[derive(serde::Deserialize)]
pub(super) struct CatalogMember {
    pub(super) spec: CatalogMemberSpec,
}

#[derive(serde::Deserialize)]
pub(super) struct CatalogMemberSpec {
    pub(super) image: String,
    pub(super) project: String,
}

#[derive(Clone, serde::Deserialize, Default)]
pub(super) struct CatalogStatus {
    #[serde(default)]
    pub(super) phase: String,
    #[serde(default)]
    pub(super) backing_ref: Option<String>,
    #[serde(default)]
    pub(super) file_path: Option<String>,
    #[serde(default)]
    pub(super) sha256: Option<String>,
    #[serde(default)]
    pub(super) size_bytes: Option<u64>,
    /// Size of the disk the image expands to.
    #[serde(default)]
    pub(super) virtual_size_bytes: Option<u64>,
}

impl CatalogImage {
    /// May a machine in `project` boot this image? This is mold's
    /// `Client::can_see` for a caller in that project, read off the stored
    /// rows: a public image, one the project owns, or one shared with the
    /// project by a member row. A machine with no project is the cloud
    /// admin's and may use any image.
    pub(super) fn usable_by(&self, store: &Store, project: Option<&Id>) -> bool {
        let Some(project) = project.map(Id::to_string) else {
            return true;
        };
        if self.spec.visibility == "public" || self.project.as_deref() == Some(&project) {
            return true;
        }
        self.spec.visibility == "project"
            && store
                .raw_scan("resources", "imgmem/")
                .unwrap_or_default()
                .iter()
                .filter_map(|(_, bytes)| serde_json::from_slice::<CatalogMember>(bytes).ok())
                .any(|m| m.spec.image == self.id && m.spec.project == project)
    }

    /// The image's file on this server, when it has one that exists.
    pub(super) fn local_file(&self) -> Option<String> {
        let path = self
            .status
            .file_path
            .clone()
            .filter(|p| !p.is_empty())
            .or_else(|| self.status.backing_ref.clone())?;
        let file = Path::new(&path);
        (file.is_absolute() && file.is_file()).then_some(path)
    }

    /// What an agent needs to download the image: the id and the digest.
    /// The digest is `status.sha256`, or the hex of a `sha256:` blob id.
    pub(super) fn source(&self) -> Option<ImageSource> {
        let sha256 = self
            .status
            .sha256
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.status
                    .backing_ref
                    .as_deref()
                    .and_then(|r| r.strip_prefix("sha256:"))
                    .map(str::to_owned)
            })?;
        Some(ImageSource {
            id: self.id.clone(),
            sha256,
            size_bytes: self.status.size_bytes,
            format: self.spec.format.clone(),
        })
    }

    /// The smallest disk this image needs: what the catalog recorded, and
    /// at least the disk the image expands to.
    pub(super) fn min_disk_gib(&self) -> u64 {
        let expanded = self
            .status
            .virtual_size_bytes
            .map(|b| b.div_ceil(1 << 30))
            .unwrap_or(0);
        self.spec.min_disk_gib.max(expanded)
    }
}

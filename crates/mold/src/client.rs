// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client` is mold's core and the only writer of image `status`. HTTP
//! handlers call it; `forge` and `slab` use [`Client::open`] and
//! [`Client::resolve`] to boot from and populate volumes with images.

use pistack_store::{Collection, Store, StoreError};
use pistack_types::{ApiError, ApiResult, Caller, Id, Meta, Phase};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::io::StreamReader;

use crate::inspect;
use crate::store::{ImageData, ImageStore, LocalDirStore};
use crate::types::*;

/// Body of `POST /images`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateImage {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create on behalf of a project (or cloud-wide with `None`).
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: ImageSpec,
}

/// Body of `PUT /images/{id}`: metadata and the mutable spec fields.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateImage {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub min_disk_gib: Option<u64>,
    #[serde(default)]
    pub min_ram_mib: Option<u64>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
}

#[derive(Clone)]
pub struct Client {
    store: Store,
    blobs: Arc<dyn ImageStore>,
}

fn validate_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 128 {
        return Err(ApiError::bad_request("name must be 1-128 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(ApiError::bad_request(
            "name may contain only letters, digits, '-', '_', '.' and ':'",
        ));
    }
    Ok(())
}

/// `http(s)://host[:port]/path` with nothing clever.
pub fn validate_source_url(url: &str) -> ApiResult<()> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| ApiError::bad_request("url must start with http:// or https://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(ApiError::bad_request("url has no host"));
    }
    if authority.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad_request(
            "url may not contain credentials or whitespace",
        ));
    }
    // Bracketed IPv6 (`[::1]`, `[::1]:443`) must not be split on ':'.
    // A bare `::1` has more than one colon, so it is the host, not host:port.
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split_once(']').map(|(h, _)| h).unwrap_or(inner)
    } else if authority.matches(':').count() > 1 {
        authority
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    if host.is_empty() {
        return Err(ApiError::bad_request("url has no host"));
    }
    if matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "[::1]" | "169.254.169.254"
    ) {
        return Err(ApiError::bad_request(
            "url may not point at the server itself",
        ));
    }
    Ok(())
}

impl Client {
    pub fn new(store: Store, blobs: Arc<dyn ImageStore>) -> Self {
        Self { store, blobs }
    }

    /// In-memory store with blobs under `dir`. For tests and the dev loop.
    pub fn local(dir: impl Into<std::path::PathBuf>) -> ApiResult<Self> {
        Ok(Self::new(
            Store::in_memory()?,
            Arc::new(LocalDirStore::new(dir)),
        ))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn blobs(&self) -> &Arc<dyn ImageStore> {
        &self.blobs
    }
    pub fn images(&self) -> Collection<Image> {
        self.store.collection()
    }
    pub fn members(&self) -> Collection<ImageMember> {
        self.store.collection()
    }

    // ---- visibility -------------------------------------------------------

    /// Can `caller` see `img`? Admins without a project scope see all.
    pub fn can_see(&self, caller: &Caller, img: &Image) -> ApiResult<bool> {
        if caller.is_admin() && caller.project.is_none() {
            return Ok(true);
        }
        if img.spec.visibility == Visibility::Public {
            return Ok(true);
        }
        let Some(p) = &caller.project else {
            return Ok(caller.is_admin());
        };
        if img.meta.project.as_ref() == Some(p) {
            return Ok(true);
        }
        if img.spec.visibility == Visibility::Project {
            return self.is_member(&img.meta.id, p);
        }
        Ok(false)
    }

    /// May `caller` change `img`? Owner project or admin.
    pub fn can_edit(&self, caller: &Caller, img: &Image) -> bool {
        caller.is_admin() || (caller.project.is_some() && caller.project == img.meta.project)
    }

    fn is_member(&self, image: &Id, project: &Id) -> ApiResult<bool> {
        Ok(self
            .members()
            .list(None)?
            .iter()
            .any(|m| &m.spec.image == image && &m.spec.project == project))
    }

    /// Images visible to `caller`.
    pub fn list_visible(&self, caller: &Caller) -> ApiResult<Vec<Image>> {
        let mut out = Vec::new();
        for img in self.images().list(None)? {
            if self.can_see(caller, &img)? {
                out.push(img);
            }
        }
        Ok(out)
    }

    /// Resolve a name or id for `caller`, enforcing visibility.
    pub fn get_visible(&self, caller: &Caller, name_or_id: &str) -> ApiResult<Image> {
        let img = self.resolve(caller.project.as_ref(), name_or_id)?;
        if !self.can_see(caller, &img)? {
            return Err(ApiError::not_found(format!(
                "image '{name_or_id}' not found"
            )));
        }
        Ok(img)
    }

    // ---- public API for forge and slab ------------------------------------

    /// Resolve `name_or_id` for `project`: an id anywhere, a name in the
    /// project, else a public or cloud-scoped image by that name.
    pub fn resolve(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Image> {
        let col = self.images();
        if let Some(id) = Id::parse(KIND_IMAGE, name_or_id) {
            return Ok(col.get(&id)?);
        }
        if let Some(p) = project {
            if let Some(img) = not_found_to_none(col.get_by_name(Some(p), name_or_id))? {
                return Ok(img);
            }
        }
        if let Some(img) = not_found_to_none(col.get_by_name(None, name_or_id))? {
            return Ok(img);
        }
        let mut publics: Vec<Image> = col
            .list(None)?
            .into_iter()
            .filter(|i| i.meta.name == name_or_id && i.spec.visibility == Visibility::Public)
            .collect();
        publics.sort_by_key(|i| std::cmp::Reverse(i.meta.created_at));
        publics
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::not_found(format!("image '{name_or_id}' not found")))
    }

    /// Stream a ready image's bytes.
    pub async fn open(&self, image: &Id) -> ApiResult<ImageData> {
        let img = self.images().get(image)?;
        self.open_image(&img).await
    }

    pub async fn open_image(&self, img: &Image) -> ApiResult<ImageData> {
        if img.status.phase != Phase::Ready {
            return Err(ApiError::conflict(format!(
                "image {} is {:?}; no data to read",
                img.meta.id, img.status.phase
            )));
        }
        let r = img.status.backing_ref.as_deref().ok_or_else(|| {
            ApiError::internal(format!(
                "image {} is ready but has no data ref",
                img.meta.id
            ))
        })?;
        self.blobs.open(r).await
    }

    // ---- metadata CRUD ----------------------------------------------------

    pub fn create(&self, req: CreateImage) -> ApiResult<Image> {
        validate_name(&req.name)?;
        if req.spec.arch.is_empty() {
            return Err(ApiError::bad_request("spec.arch must not be empty"));
        }
        if let Some(u) = &req.spec.source_url {
            validate_source_url(u)?;
        }
        let mut meta = Meta::new(KIND_IMAGE, req.name, req.project);
        meta.description = req.description;
        meta.labels = req.labels;
        meta.annotations = req.annotations;
        Ok(self.images().create(Image {
            meta,
            spec: req.spec,
            status: ImageStatus {
                phase: Phase::Pending,
                ..Default::default()
            },
        })?)
    }

    pub fn update(
        &self,
        id: &Id,
        req: UpdateImage,
        expected_revision: Option<u64>,
    ) -> ApiResult<Image> {
        if let Some(n) = &req.name {
            validate_name(n)?;
        }
        let mut cur = self.images().get(id)?;
        if let Some(n) = req.name {
            cur.meta.name = n;
        }
        if let Some(d) = req.description {
            cur.meta.description = d;
        }
        if let Some(l) = req.labels {
            cur.meta.labels = l;
        }
        if let Some(a) = req.annotations {
            cur.meta.annotations = a;
        }
        if let Some(os) = req.os {
            cur.spec.os = Some(os);
        }
        if let Some(d) = req.min_disk_gib {
            cur.spec.min_disk_gib = d;
        }
        if let Some(r) = req.min_ram_mib {
            cur.spec.min_ram_mib = r;
        }
        if let Some(v) = req.visibility {
            cur.spec.visibility = v;
        }
        Ok(self.images().update(cur, expected_revision)?)
    }

    /// Delete the record and, unless another image shares the blob, the
    /// bytes.
    pub async fn delete(&self, id: &Id) -> ApiResult<()> {
        let img = self.images().get(id)?;
        if img.status.phase == Phase::Creating {
            return Err(ApiError::conflict(format!("image {id} is being uploaded")));
        }
        for m in self.members().list(None)? {
            if &m.spec.image == id {
                self.members().delete(&m.meta.id)?;
            }
        }
        self.images().delete(id)?;
        if let Some(r) = &img.status.backing_ref {
            self.release_blob(r).await?;
        }
        Ok(())
    }

    async fn release_blob(&self, reference: &str) -> ApiResult<()> {
        let shared = self
            .images()
            .list(None)?
            .iter()
            .any(|i| i.status.backing_ref.as_deref() == Some(reference));
        if !shared {
            self.blobs.delete(reference).await?;
        }
        Ok(())
    }

    // ---- data -------------------------------------------------------------

    /// Store `data` as the image's bytes. The image must be `Pending`
    /// (never uploaded) or `Error` (retry). Hashes while writing, inspects
    /// the result, rejects anything with a backing file.
    pub async fn upload(&self, id: &Id, data: ImageData) -> ApiResult<Image> {
        let col = self.images();
        let img = col.modify(id, |i| match i.status.phase {
            Phase::Pending | Phase::Error => {
                i.status.phase = Phase::Creating;
                Ok(())
            }
            Phase::Creating => Err(StoreError::Conflict(format!(
                "image {id} upload already in progress"
            ))),
            _ => Err(StoreError::Conflict(format!("image {id} already has data"))),
        })?;

        let stored = match self.blobs.put(data).await {
            Ok(s) => s,
            Err(e) => return self.fail(id, "upload", e).await,
        };

        if stored.size == 0 {
            self.release_blob(&stored.reference).await?;
            return self
                .fail(id, "upload", ApiError::bad_request("empty upload"))
                .await;
        }

        let inspection = if img.spec.format == ImageFormat::Oci {
            None
        } else {
            let local = self.blobs.local_path(&stored.reference);
            let head = if local.is_none() {
                Some(self.blobs.open(&stored.reference).await?)
            } else {
                None
            };
            match inspect::inspect(local.as_deref(), head, img.spec.format).await {
                Ok(i) => Some(i),
                Err(e) => {
                    self.release_blob(&stored.reference).await?;
                    return self.fail(id, "inspect", e).await;
                }
            }
        };
        if let Some(i) = &inspection {
            if let Err(e) = i
                .reject_unsafe()
                .and_then(|()| inspect::check_declared(img.spec.format, i))
            {
                self.release_blob(&stored.reference).await?;
                return self.fail(id, "inspect", e).await;
            }
        }

        let file_path = self
            .blobs
            .local_path(&stored.reference)
            .filter(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned());
        Ok(col.modify(id, |i| {
            i.status.phase = Phase::Ready;
            i.status.size_bytes = Some(stored.size);
            i.status.sha256 = Some(stored.sha256.clone());
            i.status.backing_ref = Some(stored.reference.clone());
            i.status.file_path = file_path.clone();
            i.status.virtual_size_bytes = inspection.as_ref().and_then(|x| x.virtual_size);
            set_condition(
                &mut i.status.conditions,
                "Uploaded",
                true,
                "stored",
                self.blobs.name(),
            );
            match &inspection {
                Some(x) => set_condition(
                    &mut i.status.conditions,
                    "Inspected",
                    true,
                    x.source,
                    format!("{:?}", x.format).to_lowercase(),
                ),
                None => set_condition(
                    &mut i.status.conditions,
                    "Inspected",
                    true,
                    "skipped",
                    "oci",
                ),
            }
            Ok(())
        })?)
    }

    async fn fail(&self, id: &Id, op: &str, e: ApiError) -> ApiResult<Image> {
        tracing::warn!(image = %id, op, error = %e, "image upload failed");
        self.images().modify(id, |i| {
            i.status.phase = Phase::Error;
            set_condition(
                &mut i.status.conditions,
                "Uploaded",
                false,
                op,
                e.to_string(),
            );
            Ok(())
        })?;
        Err(e)
    }

    /// `POST /images/{id}/import`. Fetches `url` and streams the body through
    /// [`Client::upload`], so inspect, sha256, and the Ready phase stay the
    /// same as a direct upload.
    pub async fn import(&self, id: &Id, url: &str) -> ApiResult<Image> {
        let img = self.images().get(id)?;
        validate_source_url(url)?;
        if img.status.phase == Phase::Ready {
            return Err(ApiError::conflict(format!("image {id} already has data")));
        }
        let data = fetch_source(url).await?;
        self.upload(id, data).await
    }

    /// Stream `data` through [`Client::upload`] without fetching a URL.
    /// Tests use this so the SSRF check stays on for [`Client::import`].
    pub async fn import_reader<R>(&self, id: &Id, data: R) -> ApiResult<Image>
    where
        R: tokio::io::AsyncRead + Send + 'static,
    {
        let img = self.images().get(id)?;
        if img.status.phase == Phase::Ready {
            return Err(ApiError::conflict(format!("image {id} already has data")));
        }
        self.upload(id, Box::pin(data)).await
    }

    // ---- members ----------------------------------------------------------

    pub fn add_member(&self, image: &Id, project: Id) -> ApiResult<ImageMember> {
        let img = self.images().get(image)?;
        if img.spec.visibility != Visibility::Project {
            return Err(ApiError::conflict(format!(
                "image {image} is {:?}; members only apply to visibility 'project'",
                img.spec.visibility
            )));
        }
        if img.meta.project.as_ref() == Some(&project) {
            return Err(ApiError::bad_request(
                "the owning project is always a member",
            ));
        }
        if self.is_member(image, &project)? {
            return Err(ApiError::conflict(format!(
                "project {project} is already a member"
            )));
        }
        let meta = Meta::new(
            KIND_IMAGE_MEMBER,
            format!("{image}:{project}"),
            img.meta.project.clone(),
        );
        Ok(self.members().create(ImageMember {
            meta,
            spec: ImageMemberSpec {
                image: image.clone(),
                project,
            },
            status: ImageMemberStatus::default(),
        })?)
    }

    pub fn list_members(&self, image: &Id) -> ApiResult<Vec<ImageMember>> {
        Ok(self
            .members()
            .list(None)?
            .into_iter()
            .filter(|m| &m.spec.image == image)
            .collect())
    }

    pub fn remove_member(&self, image: &Id, project: &Id) -> ApiResult<()> {
        let m = self
            .list_members(image)?
            .into_iter()
            .find(|m| &m.spec.project == project)
            .ok_or_else(|| {
                ApiError::not_found(format!("project {project} is not a member of {image}"))
            })?;
        self.members().delete(&m.meta.id)?;
        Ok(())
    }
}

const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

fn redirect_action(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    if attempt.previous().len() >= MAX_REDIRECTS {
        return attempt.error(io::Error::other("image import: too many redirects"));
    }
    match validate_source_url(attempt.url().as_str()) {
        Ok(()) => attempt.follow(),
        Err(e) => attempt.error(io::Error::other(format!("image import: {e}"))),
    }
}

fn fetch_err(e: reqwest::Error) -> ApiError {
    if e.is_timeout() || e.is_connect() {
        ApiError::unavailable(format!("image import: {e}"))
    } else {
        ApiError::bad_request(format!("image import: {e}"))
    }
}

/// GET `url`, following at most five redirects when every hop passes
/// [`validate_source_url`]. The body is capped at 8 GiB.
async fn fetch_source(url: &str) -> ApiResult<ImageData> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(redirect_action))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(120))
        .timeout(Duration::from_secs(2 * 60 * 60))
        .build()
        .map_err(|e| ApiError::internal(format!("image import: {e}")))?;
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(fetch_err)?
        .error_for_status()
        .map_err(fetch_err)?;
    if resp.content_length().is_some_and(|n| n > MAX_IMPORT_BYTES) {
        return Err(ApiError::bad_request("image import exceeds 8 GiB"));
    }
    let stream = futures::stream::unfold((resp, 0u64), |(mut resp, read)| async move {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let next = read.saturating_add(chunk.len() as u64);
                if next > MAX_IMPORT_BYTES {
                    Some((
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "image import exceeds 8 GiB",
                        )),
                        (resp, next),
                    ))
                } else {
                    Some((Ok(chunk), (resp, next)))
                }
            }
            Ok(None) => None,
            Err(e) => Some((Err(io::Error::other(e)), (resp, read))),
        }
    });
    Ok(Box::pin(StreamReader::new(stream)))
}

fn not_found_to_none<T>(r: pistack_store::Result<T>) -> ApiResult<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_source_url;

    #[test]
    fn source_url_allows_public_http_and_rejects_ssrf() {
        assert!(validate_source_url("https://cloud-images.example/x.img").is_ok());
        assert!(validate_source_url("http://example.com:8080/a").is_ok());
        for bad in [
            "ftp://x/y",
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://127.0.0.1:9/latest",
            "http://[::1]/x",
            "http://[::1]:80/x",
            "http://::1/",
            "http://169.254.169.254/latest",
            "http://user:pass@example.com/x",
            "http://example.com/a b",
            "/etc/passwd",
            "https://",
        ] {
            assert!(validate_source_url(bad).is_err(), "{bad}");
        }
    }
}

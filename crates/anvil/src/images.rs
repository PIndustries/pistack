// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Catalog images on this host.
//!
//! The server names a machine's image by a path that exists on the server.
//! On any other host that file is not there, so the agent downloads the
//! image from the server over the agent transport and keeps it here:
//!
//! ```text
//! <data_dir>/images/<sha256>.<format>
//! ```
//!
//! The file name is the digest, so a file that is present is the right
//! bytes: it was written to a `.partial` name, hashed while it was written,
//! compared with the digest the server gave, synced, and only then renamed.
//! A download that fails or does not match leaves nothing behind. Two
//! machines that start from the same image at once share one download.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pistack_rpc::{AgentClient, ImageSource};
use pistack_types::{ApiError, ApiResult};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// A download fails when no bytes arrive for this long. There is no limit
/// on the whole transfer.
const STALL: Duration = Duration::from_secs(60);

/// Where image bytes come from. The agent's client in production.
#[async_trait]
pub trait ImageFetch: Send + Sync {
    async fn open(&self, id: &str) -> ApiResult<Box<dyn ImageBody>>;
}

/// The bytes of one image, a piece at a time. `None` is the end.
#[async_trait]
pub trait ImageBody: Send {
    async fn chunk(&mut self) -> ApiResult<Option<Vec<u8>>>;
}

#[async_trait]
impl ImageFetch for AgentClient {
    async fn open(&self, id: &str) -> ApiResult<Box<dyn ImageBody>> {
        let response = self.open_image(id).await.map_err(ApiError::from)?;
        Ok(Box::new(response))
    }
}

#[async_trait]
impl ImageBody for reqwest::Response {
    async fn chunk(&mut self) -> ApiResult<Option<Vec<u8>>> {
        match reqwest::Response::chunk(self).await {
            Ok(piece) => Ok(piece.map(|bytes| bytes.to_vec())),
            Err(err) => Err(ApiError::unavailable(format!(
                "image download broke off: {err}"
            ))),
        }
    }
}

pub struct ImageCache {
    dir: PathBuf,
    /// One lock per digest, so a second start waits for the first download.
    busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    stall: Duration,
}

impl ImageCache {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("images"),
            busy: Mutex::new(HashMap::new()),
            stall: STALL,
        }
    }

    #[cfg(test)]
    fn with_stall(mut self, stall: Duration) -> Self {
        self.stall = stall;
        self
    }

    /// The local file for `source`, downloading it first when it is not here.
    pub async fn ensure(&self, fetch: &dyn ImageFetch, source: &ImageSource) -> ApiResult<PathBuf> {
        let path = self.path_for(source)?;
        let lock = {
            let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
            busy.entry(source.sha256.clone()).or_default().clone()
        };
        let _held = lock.lock().await;
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(path);
        }
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|err| io("create the image directory", &err))?;
        open_to_readers(&self.dir, 0o755).await;
        tracing::info!(
            image = %source.id,
            sha256 = %source.sha256,
            size_bytes = source.size_bytes,
            "anvil: fetching image from the server"
        );
        let partial = self
            .dir
            .join(format!(".{}.{}.partial", source.sha256, std::process::id()));
        let fetched = self.download(fetch, source, &partial).await;
        if let Err(err) = fetched {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(err);
        }
        open_to_readers(&partial, 0o644).await;
        tokio::fs::rename(&partial, &path)
            .await
            .map_err(|err| io("install the image", &err))?;
        tracing::info!(image = %source.id, path = %path.display(), "anvil: image is on this host");
        Ok(path)
    }

    async fn download(
        &self,
        fetch: &dyn ImageFetch,
        source: &ImageSource,
        partial: &Path,
    ) -> ApiResult<()> {
        let mut body = fetch.open(&source.id).await?;
        let mut file = tokio::fs::File::create(partial)
            .await
            .map_err(|err| io("create the image file", &err))?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        loop {
            let piece = match tokio::time::timeout(self.stall, body.chunk()).await {
                Ok(piece) => piece?,
                Err(_) => {
                    return Err(ApiError::unavailable(format!(
                        "image {} stalled after {written} bytes",
                        source.id
                    )))
                }
            };
            let Some(piece) = piece else { break };
            written += piece.len() as u64;
            if source.size_bytes.is_some_and(|size| written > size) {
                return Err(ApiError::unavailable(format!(
                    "image {} is longer than the catalog says",
                    source.id
                )));
            }
            hasher.update(&piece);
            file.write_all(&piece)
                .await
                .map_err(|err| io("write the image", &err))?;
        }
        file.sync_all()
            .await
            .map_err(|err| io("sync the image", &err))?;
        if source.size_bytes.is_some_and(|size| written != size) {
            return Err(ApiError::unavailable(format!(
                "image {} ended after {written} bytes",
                source.id
            )));
        }
        let got = hex(&hasher.finalize());
        if got != source.sha256 {
            return Err(ApiError::unavailable(format!(
                "image {} does not match its digest: expected {}, got {got}",
                source.id, source.sha256
            )));
        }
        Ok(())
    }

    /// `<dir>/<sha256>.<format>`. Both parts come from the server and end
    /// up in a file name, so both are checked.
    fn path_for(&self, source: &ImageSource) -> ApiResult<PathBuf> {
        let digest_ok = source.sha256.len() == 64
            && source
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !digest_ok {
            return Err(ApiError::bad_request("image digest is not a sha256"));
        }
        let format = if source.format.is_empty() {
            "img"
        } else {
            source.format.as_str()
        };
        let format_ok = format.len() <= 16
            && format
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !format_ok {
            return Err(ApiError::bad_request("image format is not usable"));
        }
        Ok(self.dir.join(format!("{}.{format}", source.sha256)))
    }
}

/// The hypervisor runs as its own user and has to read the image.
async fn open_to_readers(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

fn io(what: &str, err: &std::io::Error) -> ApiError {
    ApiError::internal(format!("could not {what}: {err}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Pieces(std::vec::IntoIter<Vec<u8>>, Option<Duration>);

    #[async_trait]
    impl ImageBody for Pieces {
        async fn chunk(&mut self) -> ApiResult<Option<Vec<u8>>> {
            match self.0.next() {
                Some(piece) => Ok(Some(piece)),
                None => {
                    if let Some(hang) = self.1 {
                        tokio::time::sleep(hang).await;
                    }
                    Ok(None)
                }
            }
        }
    }

    /// Serves `bytes` in small pieces and counts how often it was asked.
    struct Server {
        bytes: Vec<u8>,
        opened: AtomicUsize,
        delay: Duration,
        hang: Option<Duration>,
    }

    impl Server {
        fn new(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                opened: AtomicUsize::new(0),
                delay: Duration::ZERO,
                hang: None,
            }
        }
    }

    #[async_trait]
    impl ImageFetch for Server {
        async fn open(&self, _id: &str) -> ApiResult<Box<dyn ImageBody>> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let pieces: Vec<Vec<u8>> = self.bytes.chunks(7).map(<[u8]>::to_vec).collect();
            Ok(Box::new(Pieces(pieces.into_iter(), self.hang)))
        }
    }

    fn source(bytes: &[u8]) -> ImageSource {
        ImageSource {
            id: "img-1".into(),
            sha256: hex(&Sha256::digest(bytes)),
            size_bytes: Some(bytes.len() as u64),
            format: "qcow2".into(),
        }
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir.join("images")) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[tokio::test]
    async fn an_image_is_fetched_once_and_then_served_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"a disk image that is longer than one piece";
        let server = Server::new(bytes);
        let cache = ImageCache::new(dir.path());
        let src = source(bytes);

        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.qcow2", src.sha256)
        );

        let again = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(again, path);
        assert_eq!(server.opened.load(Ordering::SeqCst), 1);

        // A new process finds the file too.
        let restarted = ImageCache::new(dir.path());
        restarted.ensure(&server, &src).await.unwrap();
        assert_eq!(server.opened.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bytes_that_do_not_match_the_digest_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::new(b"what the server actually sent....");
        let cache = ImageCache::new(dir.path());
        let mut src = source(b"what the catalog says it is......");
        src.size_bytes = Some(server.bytes.len() as u64);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("does not match its digest"), "{err:?}");
        assert!(leftovers(dir.path()).is_empty(), "a bad download was kept");
    }

    #[tokio::test]
    async fn a_short_or_long_body_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"twenty bytes of data";
        let cache = ImageCache::new(dir.path());

        let mut longer = source(bytes);
        longer.size_bytes = Some(bytes.len() as u64 + 5);
        let err = cache
            .ensure(&Server::new(bytes), &longer)
            .await
            .unwrap_err();
        assert!(err.message.contains("ended after"), "{err:?}");

        let mut shorter = source(bytes);
        shorter.size_bytes = Some(4);
        let err = cache
            .ensure(&Server::new(bytes), &shorter)
            .await
            .unwrap_err();
        assert!(err.message.contains("longer than the catalog"), "{err:?}");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn two_starts_from_one_image_share_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"shared by two machines starting together";
        let mut server = Server::new(bytes);
        server.delay = Duration::from_millis(100);
        let cache = ImageCache::new(dir.path());
        let src = source(bytes);

        let (first, second) =
            tokio::join!(cache.ensure(&server, &src), cache.ensure(&server, &src));
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(server.opened.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stalled_download_fails_and_a_retry_works() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"arrives, then the link goes quiet";
        let mut server = Server::new(bytes);
        server.hang = Some(Duration::from_secs(5));
        let cache = ImageCache::new(dir.path()).with_stall(Duration::from_millis(50));
        let src = source(bytes);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("stalled"), "{err:?}");
        assert!(leftovers(dir.path()).is_empty());

        server.hang = None;
        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn a_digest_or_format_that_is_not_a_plain_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(dir.path());
        let server = Server::new(b"x");

        let mut bad = source(b"x");
        bad.sha256 = "../../etc/passwd".into();
        assert!(cache.ensure(&server, &bad).await.is_err());

        let mut bad = source(b"x");
        bad.format = "qcow2/../../x".into();
        assert!(cache.ensure(&server, &bad).await.is_err());
        assert_eq!(server.opened.load(Ordering::SeqCst), 0);
    }
}

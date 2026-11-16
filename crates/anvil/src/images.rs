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
//! A download that does not match leaves nothing behind. Two machines that
//! start from the same image at once share one download.
//!
//! A transfer that breaks off part way is not thrown away either. What
//! arrived stays in the `.partial` file and the next attempt asks the server
//! for the rest of it; a server that cannot start in the middle sends the
//! whole file and that attempt begins from nothing. Which way it went, the
//! digest is taken over the whole image before a byte of it is used.
//!
//! A cached image that nothing on this host uses is not wanted forever, so
//! the agent sweeps the cache from the moment it starts. An image is only ever
//! deleted when nothing can still need it: no machine of this host uses it, no
//! machine disk was built on it, and nobody has asked for it in
//! `PISTACK_IMAGE_KEEP_DAYS` days.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pistack_rpc::{AgentClient, ImageSource};
use pistack_types::{ApiError, ApiResult};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// A download fails when no bytes arrive for this long. There is no limit
/// on the whole transfer.
const STALL: Duration = Duration::from_secs(60);

/// Agent setting: days an image nothing on this host uses is kept. `0` keeps
/// nothing past the sweep that first finds it unused.
pub const KEEP_DAYS_ENV: &str = "PISTACK_IMAGE_KEEP_DAYS";
/// Agent setting: how often the cache is swept, in seconds.
pub const SWEEP_SECONDS_ENV: &str = "PISTACK_IMAGE_SWEEP_SECONDS";
/// The keep time when the operator names no other one.
pub const DEFAULT_KEEP_DAYS: u64 = 7;
/// How often the cache is swept when the operator names no other interval.
pub const DEFAULT_SWEEP: Duration = Duration::from_secs(6 * 60 * 60);
/// Partial bytes nobody has touched in this long are not worth asking a
/// server to continue from.
const PARTIAL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Where image bytes come from. The agent's client in production.
#[async_trait]
pub trait ImageFetch: Send + Sync {
    /// The whole image.
    async fn open(&self, id: &str) -> ApiResult<Box<dyn ImageBody>>;

    /// The image from `from` on when the source can start in the middle of a
    /// file, and the whole of it when it cannot. The caller tells one from
    /// the other by the body's offset: a whole answer starts at `0`.
    async fn open_from(&self, id: &str, from: u64) -> ApiResult<Box<dyn ImageBody>>;
}

/// The bytes of one image, a piece at a time. `None` is the end.
#[async_trait]
pub trait ImageBody: Send {
    async fn chunk(&mut self) -> ApiResult<Option<Vec<u8>>>;

    /// The byte of the image this body starts at. `0` for a whole file.
    fn offset(&self) -> u64 {
        0
    }
}

#[async_trait]
impl ImageFetch for AgentClient {
    async fn open(&self, id: &str) -> ApiResult<Box<dyn ImageBody>> {
        let response = self.open_image(id).await.map_err(ApiError::from)?;
        Ok(Box::new(response))
    }

    async fn open_from(&self, id: &str, from: u64) -> ApiResult<Box<dyn ImageBody>> {
        let response = self
            .open_image_range(id, Some(format!("bytes={from}-")))
            .await
            .map_err(ApiError::from)?;
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

    fn offset(&self) -> u64 {
        // `Content-Range: bytes 512-1023/1024`. The first number says where
        // this body starts. Anything not said in those terms is read as the
        // start of the file.
        let range = self
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let start = range.rsplit('/').next().unwrap_or_default();
        let start = start.split('-').next().unwrap_or_default();
        start.trim().parse().unwrap_or_default()
    }
}

/// What a sweep may not delete, however old its dates say it is.
#[async_trait]
pub trait Pins: Send + Sync {
    /// The digests out of `wanted` that something on this host still needs.
    async fn held(&self, wanted: &[String]) -> HashSet<String>;

    /// The digests of `wanted` still needed, and what this host could not read
    /// while finding out. A sweep that means to delete asks this instead: when
    /// the second list is not empty the first says nothing, because something
    /// that could not be read may be using anything.
    ///
    /// A caller that only wants to know what is in use — a report, a count —
    /// asks `held`.
    async fn read(&self, wanted: &[String]) -> (HashSet<String>, Vec<String>) {
        (self.held(wanted).await, Vec::new())
    }
}

/// Nothing is held back. A sweep of such a cache goes by dates alone.
#[derive(Default)]
pub struct NoPins;

#[async_trait]
impl Pins for NoPins {
    async fn held(&self, _wanted: &[String]) -> HashSet<String> {
        HashSet::new()
    }
}

/// The images named here are held back. For a test of the sweep.
#[cfg(test)]
struct Held(HashSet<String>);

#[cfg(test)]
#[async_trait]
impl Pins for Held {
    async fn held(&self, _wanted: &[String]) -> HashSet<String> {
        self.0.clone()
    }
}

/// A clock a test sets. Everyone shares one so the test can move it forward.
#[cfg(test)]
#[derive(Default)]
struct FixedClock(std::sync::atomic::AtomicU64);

#[cfg(test)]
impl FixedClock {
    /// A clock standing one day after the real `now`.
    ///
    /// A cache reads dates the filesystem stamped with the real clock, and the
    /// dates it is asked to leave behind come from this one, so the two have to
    /// share a starting point: a clock set to an absolute date makes every file
    /// look as though it arrived after the sweep began. The day in hand keeps
    /// the clock's whole seconds ahead of a file dated right now, since a
    /// stored second is a truncated second.
    fn now_ahead() -> Arc<Self> {
        let then = SystemTime::now() + Duration::from_secs(24 * 60 * 60);
        let clock = Arc::new(Self::default());
        let secs = then
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        clock.0.store(secs, Ordering::Relaxed);
        clock
    }
}

#[cfg(test)]
impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.0.load(Ordering::Relaxed))
    }
}

/// What one agent run asks a file about. The host's `qemu-img` in production;
/// a test says what a disk answers instead of running a tool it does not have.
#[async_trait]
trait DiskInfo: Send + Sync {
    /// Every image `disk` was built on: the file behind it, and whatever is
    /// behind that. An error means the disk could not be read at all, which
    /// says nothing about any file and stops a sweep from deleting.
    async fn chain_of(&self, disk: &Path) -> ApiResult<Vec<PathBuf>>;
}

/// The host's `qemu-img`.
struct QemuInfo;

#[async_trait]
impl DiskInfo for QemuInfo {
    async fn chain_of(&self, disk: &Path) -> ApiResult<Vec<PathBuf>> {
        let mut chain = Vec::new();
        let mut next = crate::runtime::libvirt::backing_file(disk).await?;
        // A qcow2 disk can be layered again and again. Follow the whole chain,
        // stopping at a file that names nothing behind it and at one that names
        // a file already walked, which is a cycle rather than a deeper chain.
        while !chain.contains(&next) {
            chain.push(next.clone());
            match crate::runtime::libvirt::backing_file(&next).await {
                Ok(base) => next = base,
                // A chain walks to a file that has nothing behind it. Nothing
                // behind is the end of the chain, not a fault: an image read
                // that far is already held.
                Err(_) => break,
            }
        }
        Ok(chain)
    }
}

/// `Held` plus what a sweep could not read. `held` is only trustworthy when
/// `blind` is empty: a disk that could not be probed, or a directory that
/// could not be listed, means the sweep does not know what is in use.
struct Read {
    held: HashSet<String>,
    /// What could not be read, as one phrase each for the log.
    blind: Vec<String>,
}

/// The test's stand-in: every disk says it was built on `base`.
#[cfg(test)]
struct OneBacking(PathBuf);

#[cfg(test)]
#[async_trait]
impl DiskInfo for OneBacking {
    async fn chain_of(&self, _disk: &Path) -> ApiResult<Vec<PathBuf>> {
        Ok(vec![self.0.clone()])
    }
}

/// The images the machines of this host use, found by looking at the
/// directories its runtimes keep machine files in.
pub struct MachinePins {
    dirs: Vec<PathBuf>,
    /// The image cache's own directory, so a machine file that is a hard link
    /// to a cache image can be recognised as that image.
    cache: PathBuf,
    info: Arc<dyn DiskInfo>,
    /// The machines the agent's records name, read from its state directory.
    machines: Vec<PathBuf>,
    /// The machine directories of the runtimes whose disk names a sweep judges
    /// by, as read when these pins were made.
    runtime: DiskNames,
}

impl Default for MachinePins {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            cache: PathBuf::new(),
            info: Arc::new(QemuInfo),
            machines: Vec::new(),
            runtime: Vec::new(),
        }
    }
}

impl MachinePins {
    /// The pins of exactly these directories, with the cache in `cache`, for a
    /// caller that knows where its machine files are and for a test of the
    /// sweep.
    pub fn new(dirs: impl IntoIterator<Item = PathBuf>, cache: PathBuf) -> Self {
        Self {
            dirs: dirs.into_iter().collect(),
            cache,
            ..Self::default()
        }
    }

    /// The pins of exactly these directories, with `info` saying what a disk
    /// was built on. For a test, which has no `qemu-img` to ask.
    #[cfg(test)]
    fn with_info(
        dirs: impl IntoIterator<Item = PathBuf>,
        cache: PathBuf,
        info: impl DiskInfo + 'static,
    ) -> Self {
        Self {
            dirs: dirs.into_iter().collect(),
            cache,
            info: Arc::new(info),
            ..Self::default()
        }
    }

    /// The pins of these directories and these machine records. For a test.
    #[cfg(test)]
    fn looking_at(
        dirs: impl IntoIterator<Item = PathBuf>,
        cache: PathBuf,
        info: impl DiskInfo + 'static,
        machines: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        Self {
            dirs: dirs.into_iter().collect(),
            cache,
            info: Arc::new(info),
            machines: machines.into_iter().collect(),
            ..Self::default()
        }
    }
}

impl MachinePins {
    /// The machine directories of the runtimes this agent can run, and the
    /// records of the machines the agent itself knows about. The cache is not
    /// one of those directories: its own files are what a sweep judges, and a
    /// disk made by linking or copying one names the image behind it.
    pub fn for_agent(data_dir: &Path) -> Self {
        Self {
            dirs: machine_dirs(data_dir),
            cache: data_dir.join("images"),
            machines: machine_records(data_dir),
            // The runtime directories are read here, once, while a host's
            // environment is in force; the sweep judges its disks by the names
            // this records rather than by an environment read later.
            runtime: machine_disk_names(),
            ..Self::default()
        }
    }
}

#[async_trait]
impl Pins for MachinePins {
    async fn held(&self, wanted: &[String]) -> HashSet<String> {
        self.read(wanted).await.0
    }

    async fn read(&self, wanted: &[String]) -> (HashSet<String>, Vec<String>) {
        let mut read = held_in(
            &self.dirs,
            &self.cache,
            wanted,
            self.info.as_ref(),
            &self.runtime,
        )
        .await;
        // What a machine is to be built from is wanted before its disk exists,
        // so the record holds the image a disk would have held.
        for record in &self.machines {
            match tokio::fs::read(record).await {
                Ok(bytes) => match machine_image(&bytes) {
                    Ok(Some(image)) => {
                        let image = image.to_string_lossy().into_owned();
                        if let Some(digest) =
                            wanted.iter().find(|digest| names_image(&image, digest))
                        {
                            read.held.insert(digest.clone());
                        }
                    }
                    // A record that names no image asks for no image.
                    Ok(None) => {}
                    Err(err) => read.blind.push(format!("{}: {err}", record.display())),
                },
                // A machine whose record cannot be read is a machine this sweep
                // cannot see.
                Err(err) => read.blind.push(format!("{}: {err}", record.display())),
            }
        }
        (read.held, read.blind)
    }
}

/// The digests out of `wanted` that are in use under `dirs`: lying there under
/// their own name, hard-linked there from the cache, or named by a machine disk
/// built on them.
///
/// All three routes matter. A qcow2 disk names the image it is layered on; a
/// disk made by hard-linking a cache file is that image under another name and
/// shares its inode; a copied file is the image under its digest name.
///
/// A disk this host cannot ask about, or a directory it cannot list, is named
/// in `blind` and holds nothing: not knowing what a machine uses is not evidence
/// that it uses nothing. A caller that means to delete has to leave everything
/// alone when `blind` is not empty.
async fn held_in(
    dirs: &[PathBuf],
    cache: &Path,
    wanted: &[String],
    info: &dyn DiskInfo,
    given: &DiskNames,
) -> Read {
    let mut held = HashSet::new();
    let mut blind = Vec::new();
    if wanted.is_empty() {
        return Read { held, blind };
    }
    // The cache images a hard link in a machine directory could be pointing at,
    // by file identity so a second name for the same bytes is recognised.
    let links = IdentityMap::new(cache, wanted).await;
    let mut walk: Vec<Walk> = dirs
        .iter()
        .map(|dir| walk_dir(dir.to_path_buf(), given))
        .collect();
    let mut visited = HashSet::new();
    while let Some(step) = walk.pop() {
        let dir = &step.path;
        if !visited.insert(dir.clone()) {
            continue;
        }
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            // A machine directory that is not there at all is a runtime with no
            // machines on this host, and a directory named only because a
            // machine record was expected under it says nothing either way:
            // either is walked into nothing. A directory that is there and
            // cannot be opened may hold any number of disks.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                blind.push(format!("{}: {err}", dir.display()));
                continue;
            }
        };
        let mut named = Vec::new();
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(err) => {
                    blind.push(format!("{}: {err}", dir.display()));
                    break;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(err) => {
                    blind.push(format!("{}: {err}", path.display()));
                    continue;
                }
            };
            if file_type.is_dir() {
                let mut nested = walk_dir(path, given);
                // A machine directory's own subdirectory belongs to the same
                // machine, whose disk is named by the same runtime.
                if step.disks.is_some() {
                    nested.probes_any = false;
                }
                walk.push(nested);
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let disk = is_disk_name(&step, &name);
            if disk {
                named.push(name.clone());
            }
            // Both the bare digest and the digest with its extension name
            // the same image, so either of the two holds the other.
            if let Some(digest) = wanted.iter().find(|digest| names_image(&name, digest)) {
                held.insert(digest.clone());
                continue;
            }
            if let Some(digest) = links.digest_of(&path).await {
                held.insert(digest);
                continue;
            }
            // Everything a cache might want out of this file — its own name, a
            // hard link to a cached image — has been checked. What is left to
            // learn is what the file is built on, and a runtime's machine
            // directory holds a disk only under the name that runtime writes: a
            // seed, a domain definition, a console log or a cloud-init file is
            // not a disk, and asking after one answers "nothing behind it" and
            // blinds a sweep for no reason.
            if !disk {
                continue;
            }
            match info.chain_of(&path).await {
                Ok(chain) => {
                    for base in chain {
                        let base = base.to_string_lossy().into_owned();
                        if let Some(digest) =
                            wanted.iter().find(|digest| names_image(&base, digest))
                        {
                            held.insert(digest.clone());
                        }
                    }
                }
                // A disk that stands alone is built on no cached image. That is
                // an answer about the cache, not a fault.
                Err(err) if is_flat_disk(&err.message) => {}
                Err(err) => blind.push(format!("{}: {}", path.display(), err.message)),
            }
        }
        // A machine definition sitting in a runtime's own machine directory with
        // no disk file beside it: the disk is under a name this sweep does not
        // know, or somewhere it is not looking. Not evidence that the machine
        // builds on nothing.
        if step.disks.is_some() && named.is_empty() && defines_machine(dir).await {
            blind.push(format!(
                "{}: a machine definition with no disk file of its runtime's name",
                dir.display()
            ));
        }
    }
    Read { held, blind }
}

/// One directory of the walk: where it is, the file names that are this
/// directory's runtime's disks, and whether every file in it may be one.
struct Walk {
    path: PathBuf,
    /// The disk file names of the runtime that writes here, or `None` for a
    /// directory no runtime names.
    disks: Option<&'static [&'static str]>,
    /// Whether a file this walk cannot name as a disk is still probed. Only a
    /// directory no runtime names is read that way: a host that put its machine
    /// files somewhere the sweep was told to look has its disks read there, and
    /// there is no list of names that host's runtimes would have written.
    probes_any: bool,
}

/// A directory of the walk and the disk rule that applies to it.
fn walk_dir(path: PathBuf, given: &DiskNames) -> Walk {
    let disks = disk_names_of(&path, given);
    Walk {
        path,
        disks,
        probes_any: disks.is_none(),
    }
}

/// The file names that are a disk under `dir`, taken from the runtime whose
/// machine directory this is.
fn disk_names_of(dir: &Path, given: &DiskNames) -> Option<&'static [&'static str]> {
    given
        .iter()
        .find(|(root, _)| dir.starts_with(root))
        .map(|(_, names)| *names)
}

/// Whether this file is a disk of the directory being walked.
fn is_disk_name(step: &Walk, name: &str) -> bool {
    match step.disks {
        Some(names) => names.contains(&name),
        None => step.probes_any,
    }
}

/// Each machine directory a sweep is walked from, with the disk file names of
/// the runtime that writes there.
type DiskNames = Vec<(PathBuf, &'static [&'static str])>;

/// The machine directories of this host, read from the environment its runtimes
/// read them from.
fn machine_disk_names() -> DiskNames {
    let mut named = vec![
        (
            crate::runtime::libvirt::images_dir().join("machines"),
            crate::runtime::libvirt::runtime_disk_names(),
        ),
        (
            crate::runtime::cloud::state_dir(),
            crate::runtime::cloud::runtime_disk_names(),
        ),
        (
            crate::runtime::youki::state_dir(),
            crate::runtime::youki::runtime_disk_names(),
        ),
        (
            crate::runtime::metal::images_dir(),
            crate::runtime::metal::runtime_disk_names(),
        ),
    ];
    named.sort_by_key(|(root, _)| std::cmp::Reverse(root.components().count()));
    named
}

/// Whether a directory holds a machine's own definition: a domain definition,
/// or a record of the machine to build. A directory holding only a stray file
/// is not a machine whose disk went missing.
async fn defines_machine(dir: &Path) -> bool {
    for name in ["domain.xml", "machine.json"] {
        if tokio::fs::try_exists(dir.join(name)).await.unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Whether an error from a disk probe says the disk has nothing behind it. A
/// flat disk is a whole answer about the cache: it is built on no image.
/// Anything else — no tool, a failed probe, an answer that means nothing — is
/// a disk the sweep could not read.
fn is_flat_disk(message: &str) -> bool {
    message.contains(crate::runtime::libvirt::FLAT_DISK)
}

/// The wanted cache images by file identity, so a machine file that shares a
/// cache image's inode is read as that image. An identity a filesystem will not
/// name (`(dev, ino)` for a file it cannot stat) matches nothing.
struct IdentityMap {
    by_id: HashMap<(u64, u64), String>,
}

impl IdentityMap {
    async fn new(cache: &Path, wanted: &[String]) -> Self {
        let mut by_id = HashMap::new();
        for digest in wanted {
            for ext in ["", ".qcow2", ".raw", ".img"] {
                let path = cache.join(format!("{digest}{ext}"));
                if let Some(id) = file_id(&path).await {
                    by_id.insert(id, digest.clone());
                }
            }
        }
        Self { by_id }
    }

    async fn digest_of(&self, path: &Path) -> Option<String> {
        let id = file_id(path).await?;
        self.by_id.get(&id).cloned()
    }
}

/// A file's identity: the device and inode number behind any of its names.
#[cfg(unix)]
async fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
async fn file_id(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Whether the file `name` is the image with this `digest`: the digest on its
/// own, or the digest with the extension the catalog gives its format.
fn names_image(name: &str, digest: &str) -> bool {
    let file = Path::new(name)
        .file_name()
        .map(|file| file.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = Path::new(&file)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    file == digest || stem == digest
}

/// Why a sweep left a file where it lies. The class is what a sweep reports:
/// a held image is somebody's working disk, a recent one is only resting, and
/// a waiting one is this sweep's inability to judge, which is the next
/// sweep's to settle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    /// A machine of this host uses the image or a disk of it was built on it.
    Held,
    /// Used, or only just arrived, inside the keep time.
    Recent,
    /// This sweep could not judge it yet.
    Waiting,
    /// This sweep could not read whether anything uses it, so it deleted
    /// nothing. Not a wait for the next sweep's turn: a fault to be fixed.
    Blind,
    /// Something else the sweep is glad to be able to name: a download in
    /// progress, a file it could not delete.
    Other,
}

/// What one sweep did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Cache names deleted.
    pub deleted: Vec<String>,
    /// Bytes they held.
    pub bytes: u64,
    /// Cache files the sweep looked at. A name the cache did not make is not a
    /// cache file and is not counted.
    pub seen: usize,
    /// Names left alone and why. For the log and the tests: a sweep is
    /// judged by what it deleted.
    pub kept: Vec<(String, &'static str)>,
    /// What could not be read, named once whatever number of files it hid. Empty
    /// unless this sweep deleted nothing because it could not tell what was in
    /// use.
    pub blind: Vec<String>,
}

impl Sweep {
    /// The class of one kept name, and so what a sweep says about it.
    fn class(why: &'static str) -> Keep {
        match why {
            "a machine on this host uses it" => Keep::Held,
            WAITING => Keep::Waiting,
            CANNOT_TELL => Keep::Blind,
            "it has been used since the keep time" => Keep::Recent,
            _ => Keep::Other,
        }
    }

    /// How many kept names fall in `want`.
    fn count(&self, want: Keep) -> usize {
        self.kept
            .iter()
            .filter(|(_, why)| Self::class(why) == want)
            .count()
    }

    /// Whether the sweep did anything a host operator would want to hear
    /// about: deleted something, found something it could not yet decide, or
    /// could not tell what was in use at all. A cache of nothing but working
    /// disks, or of nothing at all, has no such news in it twice running.
    fn spoke(&self) -> bool {
        self.count(Keep::Waiting) > 0 || self.count(Keep::Blind) > 0 || !self.deleted.is_empty()
    }
}

/// Kept because this sweep could not yet judge it: it is the next sweep's to
/// decide. Counted as something found, not as something kept for its date.
const WAITING: &str = "it was first seen by this sweep";

/// Kept because this sweep could not find out whether anything uses it. A sweep
/// that has to say this deletes nothing at all.
const CANNOT_TELL: &str = "its use could not be read";

/// When an image was last asked for, and how many times the cache has seen it
/// asked for. The count, not the time, is what tells a sweep that a request
/// arrived after it took its snapshot, because a test's clock can stand still.
#[derive(Clone, Copy)]
struct Used {
    at: SystemTime,
    count: u64,
}

/// A clock a test sets, so a sweep is told what day it is.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub struct ImageCache {
    dir: PathBuf,
    /// One lock per digest, so a second start waits for the first download.
    busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    stall: Duration,
    pins: Arc<dyn Pins>,
    clock: Arc<dyn Clock>,
    /// Digests being downloaded right now.
    downloading: Mutex<HashSet<String>>,
    /// Images a sweep found, with the sweep that found them. An image is
    /// deleted on the sweep after the one that saw it, so a disk written
    /// while a sweep was reading is not lost to that race.
    seen: Mutex<HashMap<String, u64>>,
    /// The last time each image was asked for, and a counter of how many
    /// requests have been seen. A request during a sweep raises the counter past
    /// the value the sweep snapshotted, which is how the sweep knows its own
    /// reading of the file is out of date. The cache keeps this itself: a
    /// filesystem is under no obligation to record reads at all, so the access
    /// date an image file carries is not something a sweep can rely on.
    used: Mutex<HashMap<String, Used>>,
    sweeps: std::sync::atomic::AtomicU64,
    /// A sweep in this process, so one cache is never swept twice at once.
    sweeping: AtomicBool,
}

impl ImageCache {
    pub fn new(data_dir: &Path) -> Self {
        Self::pinned(data_dir, Arc::new(MachinePins::for_agent(data_dir)))
    }

    /// A cache whose sweep asks `pins` what its machines use.
    pub fn pinned(data_dir: &Path, pins: Arc<dyn Pins>) -> Self {
        Self {
            dir: data_dir.join("images"),
            busy: Mutex::new(HashMap::new()),
            stall: STALL,
            pins,
            clock: Arc::new(WallClock),
            downloading: Mutex::new(HashSet::new()),
            sweeping: AtomicBool::new(false),
            seen: Mutex::new(HashMap::new()),
            used: Mutex::new(HashMap::new()),
            sweeps: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn with_stall(mut self, stall: Duration) -> Self {
        self.stall = stall;
        self
    }

    /// A cache with no machines to keep back and no pins to consult: for a
    /// test that only cares about the bytes of an image.
    #[cfg(test)]
    fn alone(data_dir: &Path) -> Self {
        Self::pinned(data_dir, Arc::new(NoPins))
    }

    /// A cache that keeps back nothing and whose sweeps are told what day it
    /// is. For a test of the sweep.
    #[cfg(test)]
    fn dated(data_dir: &Path, clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            ..Self::pinned(data_dir, Arc::new(NoPins))
        }
    }

    /// A cache that keeps back the images in `held`, whatever its machine
    /// directories say. For a test of the sweep.
    #[cfg(test)]
    fn holding(data_dir: &Path, clock: Arc<dyn Clock>, held: &[&str]) -> Self {
        let held: HashSet<String> = held.iter().map(|digest| (*digest).to_owned()).collect();
        Self {
            clock,
            ..Self::pinned(data_dir, Arc::new(Held(held)))
        }
    }

    /// The local file for `source`, downloading it first when it is not here.
    pub async fn ensure(&self, fetch: &dyn ImageFetch, source: &ImageSource) -> ApiResult<PathBuf> {
        let path = self.path_for(source)?;
        self.wanted(&source.sha256);
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
        let partial = self.partial_for(&source.sha256);
        let held = self.hold(&source.sha256);
        let fetched = self.download(fetch, source, &partial).await;
        drop(held);
        if let Err(err) = fetched {
            // Bytes that cannot be a front of this image go now: the front of
            // a whole file that did not hash out is no use for the next
            // attempt. Bytes that can be finished later are what the next
            // attempt continues from, and a file not there is nothing to
            // remove.
            if err.code == pistack_types::error::ErrorCode::Unavailable {
                let _ = tokio::fs::remove_file(&partial).await;
            }
            return Err(err);
        }
        open_to_readers(&partial, 0o644).await;
        tokio::fs::rename(&partial, &path)
            .await
            .map_err(|err| io("install the image", &err))?;
        tracing::info!(image = %source.id, path = %path.display(), "anvil: image is on this host");
        Ok(path)
    }

    /// One attempt at `source`. When an earlier attempt left bytes in
    /// `partial` the server is asked for the rest of them; when it cannot
    /// oblige, this attempt starts the file again from nothing. Either way
    /// the digest is taken over the whole image.
    async fn download(
        &self,
        fetch: &dyn ImageFetch,
        source: &ImageSource,
        partial: &Path,
    ) -> ApiResult<()> {
        // Bytes on disk are worth a range request only when there is a
        // catalog size to check the answer against: without one there is no
        // way to know where the rest of a continued transfer belongs.
        let have = match source.size_bytes {
            Some(_) => resume_point(partial, source, &self.clock).await?,
            None => 0,
        };
        if have == 0 {
            return self.transfer(fetch, source, partial, true).await;
        }
        let body = fetch.open_from(&source.id, have).await?;
        // Where the answer began. A server that cannot start in the middle of
        // a file answers with the whole of it, from byte `0`.
        let offset = body.offset();
        if offset > have || source.size_bytes.is_some_and(|size| offset > size) {
            // An answer that does not line up with the file on disk is not a
            // continuation of it. The bytes already fetched are dropped and
            // the file is taken again from nothing.
            tracing::warn!(
                image = %source.id,
                asked = have,
                answered = offset,
                "anvil: the server did not resume where the bytes on disk end; taking the whole image"
            );
            return self.transfer(fetch, source, partial, true).await;
        }
        // What is on disk is the front of this answer, and this attempt writes
        // on the end of it.
        self.write(source, partial, body, have, offset, true).await
    }

    /// A transfer of the whole image, written over whatever is in `partial`.
    async fn transfer(
        &self,
        fetch: &dyn ImageFetch,
        source: &ImageSource,
        partial: &Path,
        // Whether the bytes this transfer writes are a front of the image at a
        // known offset, and so worth finishing after a break.
        resumable: bool,
    ) -> ApiResult<()> {
        let body = fetch.open(&source.id).await?;
        self.write(source, partial, body, 0, 0, resumable).await
    }

    /// Write `body`, which starts at image byte `offset`, into `partial`,
    /// which already holds the first `have` bytes of the image. The digest is
    /// taken over the whole image.
    async fn write(
        &self,
        source: &ImageSource,
        partial: &Path,
        mut body: Box<dyn ImageBody>,
        have: u64,
        offset: u64,
        // Whether the bytes this transfer writes sit at a known offset in the
        // image, and so are worth finishing after a break. Bytes written from
        // byte `0`, or onto a front an earlier attempt left, qualify; bytes
        // written over an unknown front do not.
        resumable: bool,
    ) -> ApiResult<()> {
        // The answer starts at image byte `offset`, where `partial` holds the
        // first `have` bytes. When the answer starts at or before the end of
        // the file the two can be joined: the file keeps its first `offset`
        // bytes, which the answer does not cover, and the answer supplies the
        // rest from byte `offset`. The file is cut back to `offset` and the
        // answer written on the end. An answer that starts past what is on disk
        // is not a continuation, so the file is taken again from nothing.
        let continues = have > 0 && offset <= have;
        // Bytes the file keeps: its front up to where the answer begins.
        let mut written = match continues {
            true => offset,
            false => 0,
        };

        // The digest covers the whole image, so the bytes the file keeps are
        // hashed before the file is cut back. They are known to belong here:
        // nothing but this code writes a partial, and bytes that did not match
        // were never installed.
        let mut hasher = Sha256::new();
        let head = match continues {
            true => tokio::fs::read(partial)
                .await
                .map_err(|err| io("read the bytes already downloaded", &err))?,
            false => Vec::new(),
        };
        hasher.update(&head[..written as usize]);
        // Cut the file back to where this answer writes, dropping both what the
        // answer repeats and — when the answer does not line up — everything.
        // The kept bytes are already hashed above, before this trim.
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            // The length is set by hand just below, so `create` alone is the
            // whole story: nothing is truncated first and nothing kept past it.
            .truncate(false)
            .open(partial)
            .and_then(|file| file.set_len(written))
            .map_err(|err| io("trim the image file", &err))?;
        // Reopen past the trimmed end so writes land where the answer begins.
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(partial)
            .await
            .map_err(|err| io("open the image file", &err))?;
        if have > 0 {
            tracing::info!(
                image = %source.id,
                kept = written,
                resumed = continues,
                "anvil: continuing an image download"
            );
        }
        loop {
            let piece = match tokio::time::timeout(self.stall, body.chunk()).await {
                Ok(piece) => piece?,
                Err(_) => {
                    // The link went quiet. With the whole image accounted for,
                    // nothing more is asked of the server and the bytes are the
                    // image; short of it, what has been written is a front kept
                    // for the next attempt to finish.
                    if source.size_bytes.is_some_and(|size| written >= size) {
                        break;
                    }
                    return Err(ended_short(source, written, resumable));
                }
            };
            let Some(piece) = piece else {
                // The server has sent all it has. An answer that ends
                // short of the catalog's size without breaking is a broken
                // pipe, not a whole image, and the bytes are a front worth
                // finishing.
                let short = source.size_bytes.is_some_and(|size| written < size);
                if short {
                    return Err(ended_short(source, written, resumable));
                }
                break;
            };
            written += piece.len() as u64;
            if source.size_bytes.is_some_and(|size| written > size) {
                return Err(too_long(source, written));
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
            // The answer ended with the file short of the image. Bytes this
            // attempt wrote onto a known front are still a front of it.
            return Err(ended_short(source, written, resumable));
        }
        let got = hex(&hasher.finalize());
        if got != source.sha256 {
            // Every byte the attempt wrote is inside this digest, so a
            // mismatch says the file is not this image and the partial is no
            // use to the next attempt. What was on disk before a resume is
            // wrong by the same evidence.
            let mut err = ApiError::internal(format!(
                "image {} does not match its digest: expected {}, got {got}",
                source.id, source.sha256
            ));
            if continues {
                err.message = format!(
                    "image {} was resumed onto bytes that do not match its digest: expected {}, got {got}",
                    source.id, source.sha256
                );
            } // Everything in the partial is inside this digest, so all of it is
              // wrong for this image and the caller throws it away.
            err.code = pistack_types::error::ErrorCode::Unavailable;
            return Err(err);
        }
        Ok(())
    }

    /// The cache path for an image file that is already on this host under
    /// another name: a hard link, or a copy when the file is on another
    /// filesystem. The server's blob store keeps an image under its bare
    /// digest, and libvirt's AppArmor helper only lets a guest open a disk
    /// whose name ends in an image extension.
    pub async fn adopt(&self, local: &Path, source: &ImageSource) -> ApiResult<PathBuf> {
        let path = self.path_for(source)?;
        self.wanted(&source.sha256);
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
        if tokio::fs::hard_link(local, &path).await.is_err() {
            let partial = self.partial_for(&source.sha256);
            if let Err(err) = tokio::fs::copy(local, &partial).await {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(io("copy the image", &err));
            }
            tokio::fs::rename(&partial, &path)
                .await
                .map_err(|err| io("install the image", &err))?;
        }
        open_to_readers(&path, 0o644).await;
        Ok(path)
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

    /// Delete every cached image this host has stopped using.
    ///
    /// `keep_days` is the quiet time an image has to have earned. `0` asks for
    /// nothing to be kept past the sweep that first finds it unused. A name
    /// that is not the cache's own, a download in progress, an image a
    /// machine uses or a machine disk is built on, and a file whose dates
    /// cannot be read are all left alone.
    ///
    /// An image is only deleted once a sweep has seen it and found nothing
    /// using it, so an image that arrives between the question and the
    /// answer, or one a runtime simply failed to name, is not deleted.
    ///
    /// Nothing at all is deleted by a sweep that could not read what is in
    /// use. A machine disk that cannot be probed — a running guest holds a
    /// write lock on its own disk — a machine directory that cannot be opened,
    /// a tool that is missing, and an answer that does not parse all mean the
    /// same thing: this sweep does not know what is in use, so it does not
    /// know what is safe to delete. Such a sweep says what it could not read,
    /// removes nothing, and leaves the question to the next one.
    pub async fn sweep(&self, keep_days: u64) -> ApiResult<Sweep> {
        if self
            .sweeping
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return Ok(Sweep::default());
        }
        let out = self.sweep_once(keep_days).await;
        self.sweeping.store(false, Ordering::Release);
        out
    }

    async fn sweep_once(&self, keep_days: u64) -> ApiResult<Sweep> {
        let mut out = Sweep::default();
        let sweep = self.sweeps.fetch_add(1, Ordering::Relaxed);
        let keep = Duration::from_secs(keep_days * 24 * 60 * 60);
        let now = self.clock.now();
        let mut files = Vec::new();
        match tokio::fs::read_dir(&self.dir).await {
            Ok(mut entries) => {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let Some(digest) = digest_of(&name) else {
                        continue;
                    };
                    files.push((name, digest));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(io("read the image directory", &err)),
        }
        out.seen = files.len();
        let mut digests: Vec<String> = files.iter().map(|(_, digest)| digest.clone()).collect();
        digests.sort();
        digests.dedup();
        // What the cache has recorded as the last request for each image,
        // taken before anything else is read. A request that arrives while the
        // sweep is working leaves a date newer than this, which is the sweep's
        // cue to leave that image to the next one.
        let mut asked = self.used.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // A file the cache has no record for was last asked for as lately as
        // its own date: the date is the only evidence there is. Adopting it
        // dates an old stray from when it really sat untouched, so a later
        // sweep can call it old, and lets the next sweep see a request that
        // arrives from now on. Held here, rather than in the shared record, so
        // the date the sweep judged by is the date written back at the end.
        let mut adopted: Vec<(String, SystemTime)> = Vec::new();
        for digest in &digests {
            if asked.contains_key(digest) {
                continue;
            }
            let date = self.file_date(digest).await;
            asked.insert(digest.clone(), Used { at: date, count: 0 });
            adopted.push((digest.clone(), date));
        }
        let (held, blind_reasons) = self.pins.read(&digests).await;
        // A sweep that could not read everything does not know what is in use,
        // so nothing it read about the machines is evidence of anything: no
        // image is counted as held and none is deleted on such a reading.
        let blind = if blind_reasons.is_empty() {
            false
        } else {
            tracing::warn!(
                seen = out.seen,
                could_not_read = blind_reasons.len(),
                what = blind_reasons.join("; "),
                "anvil: image cache not swept, could not tell what is in use; nothing deleted"
            );
            true
        };
        let downloading = self
            .downloading
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect::<HashMap<String, u64>>();
        for name in digests.iter() {
            seen.entry(name.clone()).or_insert(sweep);
        }
        let mut blind_reported = false;
        for (name, digest) in files {
            let path = self.dir.join(&name);
            let keep_it = |out: &mut Sweep, why| out.kept.push((name.clone(), why));
            if downloading.contains(&digest) {
                keep_it(&mut out, "a download is in progress");
                continue;
            }
            // What a sweep could not read about the machines says nothing about
            // any file: an image is not counted as held on a reading that missed
            // a disk, and is re-armed so that a sweep which did not know is
            // never the one that deletes.
            if held.contains(&digest) && !blind {
                seen.insert(digest, sweep);
                keep_it(&mut out, "a machine on this host uses it");
                continue;
            }
            let Some(first) = seen.get(&digest).copied() else {
                keep_it(&mut out, "no sweep has seen it before");
                continue;
            };
            if blind {
                seen.insert(digest, sweep);
                keep_it(&mut out, CANNOT_TELL);
                // Named once whatever number of files the same blind reading
                // hides, so the log and the test see the fault, not its echo.
                if !blind_reported {
                    blind_reported = true;
                    out.blind = blind_reasons.clone();
                }
                continue;
            }
            if first == sweep {
                // Seen now, and named by nothing now. The next sweep decides, by
                // which time the machine directories have been read once more:
                // an image written between this read and now is not deleted by
                // this sweep.
                keep_it(&mut out, WAITING);
                continue;
            }
            let Ok(meta) = tokio::fs::metadata(&path).await else {
                seen.remove(&digest);
                continue;
            };
            if !meta.is_file() {
                keep_it(&mut out, "it is not a file");
                continue;
            }
            // The cache's record of the last request, read again after the
            // machines were asked what they use. A count past the snapshot's
            // means a request arrived while this sweep was working.
            let asked_now = self
                .used
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&digest)
                .copied();
            if asked_now
                .is_some_and(|now| asked.get(&digest).is_none_or(|was| now.count > was.count))
            {
                // Wanted after the machines were asked what they use. Its age
                // cannot be trusted for this sweep.
                seen.insert(digest.clone(), sweep);
                keep_it(&mut out, "it was asked for during this sweep");
                continue;
            }
            // A filesystem need not record reads, so a request may leave no
            // mark on the file. The cache's own record of the request is what
            // dates an image as used when the file's dates say otherwise. A
            // file with no record was dated from its own mtime above, so an
            // old stray is old here too.
            let age = match asked_now.or_else(|| asked.get(&digest).copied()) {
                Some(used) => now
                    .duration_since(used.at)
                    .unwrap_or_else(|_| age_of(&meta, now)),
                None => age_of(&meta, now),
            };
            if age < keep {
                keep_it(&mut out, "it has been used since the keep time");
                continue;
            }
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    seen.remove(&digest);
                    tracing::info!(
                        file = %name,
                        bytes = meta.len(),
                        age_days = age.as_secs() / (24 * 60 * 60),
                        "anvil: unused image deleted from the cache"
                    );
                    out.deleted.push(name);
                    out.bytes += meta.len();
                }
                Err(err) => {
                    tracing::warn!(file = %name, error = %err, "anvil: could not delete an unused image");
                    keep_it(&mut out, "it could not be deleted");
                }
            }
        }
        // A record is only worth keeping for an image that is still here. It
        // goes back under the lock at the end, never held across a read of
        // the disk or of a machine directory.
        let mut back = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let still_here: HashMap<&str, Option<SystemTime>> = adopted
            .iter()
            .map(|(digest, at)| (digest.as_str(), Some(*at)))
            .chain(digests.iter().map(|digest| (digest.as_str(), None)))
            .collect();
        for (digest, at) in seen {
            if still_here.contains_key(digest.as_str()) {
                back.insert(digest, at);
            }
        }
        used.retain(|digest, used| match still_here.get(digest.as_str()) {
            // A record that was here before this sweep is left exactly as it
            // was: its count is what tells a later sweep that a request
            // arrived while that sweep was working.
            Some(Some(_)) => true,
            // A file this sweep adopted is kept with the date it was given,
            // at no count of its own — it has never been asked for.
            Some(None) => {
                used.at = adopted
                    .iter()
                    .find_map(|(d, at)| (d == digest).then_some(*at))
                    .unwrap_or(used.at);
                used.count = 0;
                true
            }
            None => false,
        });
        Ok(out)
    }

    /// The date a cache file was last touched, as the filesystem sees it, so a
    /// file nobody recorded is dated by its own record rather than by the
    /// sweep that found it. A file that is not there, or carries no date at
    /// all, is dated from nothing — the reading that lets a sweep consider it
    /// old.
    async fn file_date(&self, digest: &str) -> SystemTime {
        for ext in ["", ".qcow2", ".raw", ".img"] {
            let path = self.dir.join(format!("{digest}{ext}"));
            if let Ok(meta) = tokio::fs::metadata(&path).await {
                if meta.is_file() {
                    if let Ok(at) = meta.modified() {
                        return at.max(UNIX_EPOCH);
                    }
                }
            }
        }
        UNIX_EPOCH
    }

    /// Say that the image with this digest was asked for now.
    fn wanted(&self, digest: &str) {
        let now = self.clock.now();
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let entry = used
            .entry(digest.to_owned())
            .or_insert(Used { at: now, count: 0 });
        entry.at = now;
        entry.count += 1;
    }

    fn partial_for(&self, digest: &str) -> PathBuf {
        self.dir.join(format!(".{digest}.partial"))
    }

    /// Say that `digest` has bytes on disk right now, so a sweep going on
    /// elsewhere in this process leaves it alone. The returned value says so
    /// no longer when it is dropped.
    fn hold(&self, digest: &str) -> Downloading<'_> {
        self.downloading
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(digest.to_owned());
        Downloading {
            downloading: &self.downloading,
            digest: digest.to_owned(),
        }
    }
}

/// The digest a cache file carries: `<sha256>` or `<sha256>.<format>`. A
/// dotfile — a download in progress — and any name the cache did not make
/// are not the sweep's to delete.
fn digest_of(name: &str) -> Option<String> {
    if name.starts_with('.') {
        return None;
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hex = stem.len() == 64
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    hex.then_some(stem)
}

/// Bytes on disk the next attempt can continue from: `0` when there are
/// none, or when keeping them would cost more than starting again. Bytes
/// that cannot be this image at all are an error, which the caller reads as
/// "throw these away" rather than "write on the end of them".
async fn resume_point(
    partial: &Path,
    source: &ImageSource,
    clock: &Arc<dyn Clock>,
) -> ApiResult<u64> {
    let Ok(meta) = tokio::fs::metadata(partial).await else {
        return Ok(0);
    };
    if !meta.is_file() {
        return Err(ApiError::internal(format!(
            "the bytes left of image {} are not a file",
            source.sha256
        )));
    }
    let size = match source.size_bytes {
        // With no size in the catalog there is no way to know where the rest
        // of a continued transfer belongs, so nothing is continued.
        Some(size) => size,
        None => return Ok(0),
    };
    if meta.len() >= size {
        // Bytes as long as the image that never installed are not a front of
        // it — a right-length file hashes and installs or is discarded. Start
        // the file again rather than append past its end.
        return Ok(0);
    }
    // Half an image nobody has wanted for a day is not worth another
    // request, and bytes that would take longer to finish than the whole
    // image would take are not worth continuing either.
    let stale = age_of(&meta, clock.now()) > PARTIAL_TTL;
    if stale || meta.len().saturating_mul(2) > size {
        return Ok(0);
    }
    Ok(meta.len())
}

/// How long since a file last changed, as the filesystem sees it. A filesystem
/// that records no date for a file leaves it read as ancient, which is the safe
/// reading: the sweep only deletes when nothing else holds the image.
fn age_of(meta: &std::fs::Metadata, now: SystemTime) -> Duration {
    let then = meta.modified().unwrap_or(UNIX_EPOCH).max(UNIX_EPOCH);
    now.duration_since(then).unwrap_or_default()
}

/// The directories a host's machine files live in, for a sweep that has to
/// know which images are in use: every path a runtime puts a machine's files
/// in, read where the runtime reads it, plus the metal runtime's image
/// directory, where a machine boots from a file nothing else points at.
///
/// A directory that is not there is a runtime this host does not run and holds
/// no disk; a directory that is there and cannot be opened is named as a blind
/// spot by the sweep that walks it.
fn machine_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        crate::runtime::libvirt::images_dir().join("machines"),
        crate::runtime::cloud::state_dir(),
        crate::runtime::youki::state_dir(),
        crate::runtime::metal::images_dir(),
    ];
    dirs.extend(machine_dirs_under(data_dir));
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Machine files a host keeps under its own data directory as well. The
/// paths a runtime uses by default are compiled in, not read from here, so a
/// host that put them somewhere else says so in its environment and gets a
/// sweep that cannot see them; these are the places it may also have put
/// them.
fn machine_dirs_under(data_dir: &Path) -> impl Iterator<Item = PathBuf> + use<'_> {
    ["machines", "disks"]
        .into_iter()
        .map(|part| data_dir.join(part))
}

/// The records this agent keeps of the machines it was told about, one per
/// machine. A machine being provisioned has no disk yet, so its record is the
/// only thing naming the image it is being built from.
fn machine_records(data_dir: &Path) -> Vec<PathBuf> {
    let mut records = Vec::new();
    for dir in machine_records_under(data_dir) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // A directory that is not here is a host with no machine records.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            // A directory this sweep cannot list may name a machine whose image
            // nothing else holds. Named as a blind spot rather than skipped: a
            // read that cannot be made is not a machine that does not exist.
            Err(_) => {
                records.push(dir);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                records.push(path.join("machine.json"));
            }
        }
    }
    records.sort();
    records.dedup();
    records
}

/// Where an agent's machine records live. The state of the runtimes that keep
/// one per machine is under the data directory the agent was given; a host that
/// pointed one somewhere else by name has its disks under a directory the sweep
/// already walks, so a disk names the image its record would have named.
fn machine_records_under(data_dir: &Path) -> impl Iterator<Item = PathBuf> + use<'_> {
    ["machines", "disks"]
        .into_iter()
        .map(|part| data_dir.join(part))
}

/// The image a machine record says the machine is built from, or `None` for a
/// record that names no image. An error is a record that could not be read or
/// understood, which is not the same as a record naming nothing.
fn machine_image(bytes: &[u8]) -> Result<Option<PathBuf>, String> {
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| format!("record is not json: {err}"))?;
    let Some(image) = json.get("image") else {
        return Ok(None);
    };
    let Some(image) = image.as_str() else {
        return Err("the record's image is not text".to_owned());
    };
    if image.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(image)))
}

/// Days an unused image is kept, out of `PISTACK_IMAGE_KEEP_DAYS`. `0` is a
/// real answer and means nothing is kept once nothing uses it. A value that is
/// not a number of days is refused: an agent must not start sweeping on a
/// keep time it invented, so the caller stops the agent with this message.
pub fn keep_days_from(raw: Option<&str>) -> Result<u64, String> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_KEEP_DAYS);
    };
    match raw.trim().parse::<u64>() {
        Ok(days) => Ok(days),
        Err(_) => Err(format!(
            "{KEEP_DAYS_ENV}={raw} is not a whole number of days; set it to a number, or to 0 to keep an unused image for nothing beyond the sweep that finds it unused"
        )),
    }
}

/// How often the cache is swept, out of `PISTACK_IMAGE_SWEEP_SECONDS`. Refused
/// for the same reason as the keep time.
pub fn sweep_every_from(raw: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_SWEEP);
    };
    match raw.trim().parse::<u64>() {
        Ok(secs) if secs > 0 => Ok(Duration::from_secs(secs)),
        Ok(_) => Err(format!(
            "{SWEEP_SECONDS_ENV}=0 would sweep forever; set it to a number of seconds above 0"
        )),
        Err(_) => Err(format!(
            "{SWEEP_SECONDS_ENV}={raw} is not a number of seconds; set it to a number above 0"
        )),
    }
}

/// The sweep settings of this host's environment, or the message saying which
/// one the operator has to fix. Read once, before the agent starts looping.
pub fn sweep_settings() -> Result<SweepSettings, String> {
    Ok(SweepSettings {
        keep_days: keep_days_from(std::env::var(KEEP_DAYS_ENV).ok().as_deref())?,
        every: sweep_every_from(std::env::var(SWEEP_SECONDS_ENV).ok().as_deref())?,
    })
}

/// How the cache of this host is swept. Read once, out of the environment,
/// before the agent starts: a keep time or interval the agent cannot honour is
/// refused at startup rather than invented at the first sweep.
#[derive(Clone, Copy, Debug)]
pub struct SweepSettings {
    pub keep_days: u64,
    pub every: Duration,
}

/// Sweep the cache until `shutdown`, missing a sweep rather than queueing one
/// up. The interval and the keep time are passed in rather than read here: an
/// agent that was given a keep time it could not parse must fail at startup,
/// and must not find out on its hundredth sweep.
///
/// One line per sweep that did something or found something waiting: how many
/// files the sweep looked at, how many it kept for each reason, how many it
/// removed and the bytes those held. A sweep with nothing to report says
/// nothing above debug.
pub async fn run(
    cache: Arc<ImageCache>,
    keep_days: u64,
    every: Duration,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // A `tokio` interval is ready the moment it is made. Emptying the first
    // one keeps the wait below honest: without it a host that starts with its
    // control plane down would sit through one whole interval before its
    // first sweep, which is exactly the gap this task is here to close.
    tick.reset();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tick.tick() => {}
        }
        match cache.sweep(keep_days).await {
            Ok(sweep) => log_sweep(&sweep, keep_days),
            Err(err) => tracing::warn!(error = %err.message, "anvil: image cache sweep failed"),
        }
    }
}

/// The one line a sweep earns. See `run`.
fn log_sweep(sweep: &Sweep, keep_days: u64) {
    let held = sweep.count(Keep::Held);
    let recent = sweep.count(Keep::Recent);
    let waiting = sweep.count(Keep::Waiting);
    if !sweep.spoke() {
        tracing::debug!(
            seen = sweep.seen,
            held,
            recent,
            removed = 0,
            bytes = 0,
            "anvil: image cache swept, nothing to delete and nothing waiting"
        );
        return;
    }
    // The count a sweep reports is of files, so `held` says nothing on the line
    // of a sweep that could not read what was in use: what it could not read is
    // named instead.
    let blind = !sweep.blind.is_empty();
    tracing::info!(
        seen = sweep.seen,
        held = if blind { 0 } else { held },
        recent,
        waiting,
        removed = sweep.deleted.len(),
        bytes = sweep.bytes,
        keep_days,
        blind,
        could_not_read = sweep.blind.join("; "),
        "anvil: image cache swept"
    );
}

/// A digest this cache is downloading into right now.
struct Downloading<'a> {
    downloading: &'a Mutex<HashSet<String>>,
    digest: String,
}

impl Drop for Downloading<'_> {
    fn drop(&mut self) {
        self.downloading
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.digest);
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

/// The transfer ended, or went quiet, before the image was whole. What is on
/// disk decides what the next attempt does: bytes that are a known front of
/// the image are kept, and the answer is `Internal` because
/// `Unavailable` is how this code says "these bytes are of no use, throw
/// them away".
fn ended_short(source: &ImageSource, written: u64, keepable: bool) -> ApiError {
    match keepable {
        true => ApiError::internal(format!(
            "image {} broke off at {written} bytes and can be resumed",
            source.id
        )),
        false => ApiError::unavailable(format!(
            "image {} broke off after {written} bytes and cannot be resumed",
            source.id
        )),
    }
}

fn too_long(source: &ImageSource, written: u64) -> ApiError {
    ApiError::unavailable(format!(
        "image {} is longer than the catalog says: {written} bytes in",
        source.id
    ))
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
    use std::sync::atomic::AtomicUsize;

    /// A body of `pieces`, that goes quiet forever after `hang` says to.
    struct Body {
        pieces: std::vec::IntoIter<Vec<u8>>,
        offset: u64,
        hang: Option<Duration>,
    }

    #[async_trait]
    impl ImageBody for Body {
        async fn chunk(&mut self) -> ApiResult<Option<Vec<u8>>> {
            match self.pieces.next() {
                Some(piece) => Ok(Some(piece)),
                None => {
                    if let Some(hang) = self.hang {
                        tokio::time::sleep(hang).await;
                    }
                    Ok(None)
                }
            }
        }

        fn offset(&self) -> u64 {
            self.offset
        }
    }

    /// Serves `bytes` in pieces of seven, and counts what it was asked for.
    /// `cut` is how many bytes one answer sends before the link goes quiet;
    /// `hang` is when it goes quiet forever; `ranged` says whether this server
    /// knows how to start a transfer in the middle of a file; `overlap` is how
    /// far before the byte it was asked for an answer begins; `corrupt` sends
    /// the last byte of an answer wrong.
    struct Server {
        bytes: Vec<u8>,
        whole: AtomicUsize,
        ranges: AtomicUsize,
        delay: Duration,
        cut: Option<usize>,
        hang: Option<Duration>,
        ranged: bool,
        corrupt: bool,
        overlap: u64,
        /// Whether a fresh answer carries the fault. A test turns this off with
        /// `cut = None` and `hang = None` to say the connection is fixed; while
        /// either is set, every answer it gives is broken the same way.
        faults: AtomicBool,
    }

    impl Server {
        fn new(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                whole: AtomicUsize::new(0),
                ranges: AtomicUsize::new(0),
                delay: Duration::ZERO,
                cut: None,
                hang: None,
                ranged: true,
                corrupt: false,
                overlap: 0,
                faults: AtomicBool::new(true),
            }
        }
    }

    #[async_trait]
    impl ImageFetch for Server {
        async fn open(&self, _id: &str) -> ApiResult<Box<dyn ImageBody>> {
            self.whole.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(Box::new(whole(self)))
        }

        async fn open_from(&self, _id: &str, from: u64) -> ApiResult<Box<dyn ImageBody>> {
            if !self.ranged {
                // A server that does not know the request answers with the
                // whole file, exactly as one asked for it from the start.
                self.whole.fetch_add(1, Ordering::SeqCst);
                return Ok(Box::new(whole(self)));
            }
            self.ranges.fetch_add(1, Ordering::SeqCst);
            let start = (from as usize).saturating_sub(self.overlap as usize);
            Ok(Box::new(answer(self, start)))
        }
    }

    /// Whether a fresh answer carries the fault.
    fn armed(server: &Server) -> bool {
        server.faults.load(Ordering::SeqCst)
    }

    /// A whole-file answer.
    fn whole(server: &Server) -> Body {
        answer_at(server, 0)
    }

    /// The answer that starts at `start`.
    fn answer(server: &Server, start: usize) -> Body {
        answer_at(server, start)
    }

    fn answer_at(server: &Server, start: usize) -> Body {
        let faulty = armed(server);
        let mut bytes = server.bytes[start.min(server.bytes.len())..].to_vec();
        if server.corrupt {
            if let Some(last) = bytes.last_mut() {
                *last ^= 0xff;
            }
        }
        // The connection dies after the image's `cut`th byte, wherever the
        // answer happens to start, as a range request that got part way
        // through the rest of the file would.
        if faulty {
            if let Some(cut) = server.cut {
                bytes.truncate(cut.saturating_sub(start));
            }
        }
        Body {
            pieces: pieces(&bytes),
            offset: start as u64,
            hang: faulty.then_some(server.hang).flatten(),
        }
    }

    /// The pieces of `bytes`, seven at a time.
    fn pieces(bytes: &[u8]) -> std::vec::IntoIter<Vec<u8>> {
        bytes
            .chunks(7)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
            .into_iter()
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
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Both of a file's dates to `days` ago, as the filesystem sees them.
    fn aged(path: &Path, days: u64) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - days * 24 * 60 * 60;
        let times = [
            libc::timeval {
                tv_sec: seconds as i64,
                tv_usec: 0,
            },
            libc::timeval {
                tv_sec: seconds as i64,
                tv_usec: 0,
            },
        ];
        let raw = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a two-element array of plain times and a NUL-terminated
        // path, both alive for the call.
        let rc = unsafe { libc::utimes(raw.as_ptr(), times.as_ptr()) };
        assert_eq!(rc, 0, "could not date {}", path.display());
    }

    #[tokio::test]
    async fn an_image_is_fetched_once_and_then_served_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"a disk image that is longer than one piece";
        let server = Server::new(bytes);
        let cache = ImageCache::alone(dir.path());
        let src = source(bytes);

        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.qcow2", src.sha256)
        );

        let again = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(again, path);
        assert_eq!(server.whole.load(Ordering::SeqCst), 1);

        // A new process finds the file too.
        let restarted = ImageCache::alone(dir.path());
        restarted.ensure(&server, &src).await.unwrap();
        assert_eq!(server.whole.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_file_already_here_is_given_a_name_with_its_format() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"the server's blob";
        let src = source(bytes);
        let blob = dir.path().join(&src.sha256);
        std::fs::write(&blob, bytes).unwrap();
        let cache = ImageCache::alone(dir.path());

        let path = cache.adopt(&blob, &src).await.unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.qcow2", src.sha256)
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(cache.adopt(&blob, &src).await.unwrap(), path);
        assert_eq!(leftovers(dir.path()), vec![format!("{}.qcow2", src.sha256)]);

        // Nothing to adopt is an error, and leaves nothing behind.
        let other = source(b"another image");
        assert!(cache
            .adopt(&dir.path().join("missing"), &other)
            .await
            .is_err());
        assert_eq!(leftovers(dir.path()).len(), 1);
    }

    #[tokio::test]
    async fn bytes_that_do_not_match_the_digest_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::new(b"what the server actually sent....");
        let cache = ImageCache::alone(dir.path());
        let mut src = source(b"what the catalog says it is......");
        src.size_bytes = Some(server.bytes.len() as u64);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("does not match its digest"), "{err:?}");
        // The whole image arrived and hashed wrong, so there is no front worth
        // continuing and nothing is kept for the next attempt.
        assert!(leftovers(dir.path()).is_empty(), "a bad download was kept");
        // The image is not installed under its digest either.
        assert!(!dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", src.sha256))
            .exists());
    }

    #[tokio::test]
    async fn a_short_or_long_body_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"twenty bytes of data";
        let cache = ImageCache::alone(dir.path());

        let mut longer = source(bytes);
        longer.size_bytes = Some(bytes.len() as u64 + 5);
        let err = cache
            .ensure(&Server::new(bytes), &longer)
            .await
            .unwrap_err();
        // A body short of the catalog's size without breaking is treated as a
        // connection that died part way: the bytes kept are a front the next
        // attempt continues from.
        assert!(err.message.contains("broke off"), "{err:?}");

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
        let cache = ImageCache::alone(dir.path());
        let src = source(bytes);

        let (first, second) =
            tokio::join!(cache.ensure(&server, &src), cache.ensure(&server, &src));
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(server.whole.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stalled_download_fails_and_a_retry_works() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"arrives, then the link goes quiet";
        let mut server = Server::new(bytes);
        // The link dies after the first piece, well short of the image.
        server.cut = Some(7);
        server.hang = Some(Duration::from_secs(5));
        let cache = ImageCache::alone(dir.path()).with_stall(Duration::from_millis(50));
        let src = source(bytes);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("broke off"), "{err:?}");
        // The bytes written before the break are a front of the image and wait
        // in the partial for the next attempt.
        assert_eq!(
            leftovers(dir.path()),
            vec![format!(".{}.partial", src.sha256)]
        );

        server.cut = None;
        server.hang = None;
        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn a_digest_or_format_that_is_not_a_plain_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::alone(dir.path());
        let server = Server::new(b"x");

        let mut bad = source(b"x");
        bad.sha256 = "../../etc/passwd".into();
        assert!(cache.ensure(&server, &bad).await.is_err());

        let mut bad = source(b"x");
        bad.format = "qcow2/../../x".into();
        assert!(cache.ensure(&server, &bad).await.is_err());
        assert_eq!(server.whole.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_cut_download_is_finished_from_the_bytes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"an image that breaks off halfway and is picked up again";
        let mut server = Server::new(bytes);
        server.cut = Some(12);
        // The connection dies after those bytes rather than the stream
        // simply ending: nothing more ever arrives.
        server.hang = Some(Duration::from_secs(60));
        let cache = ImageCache::alone(dir.path()).with_stall(Duration::from_millis(20));
        let src = source(bytes);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("broke off"), "{err:?}");
        // The bytes that got here wait in the partial, not in the cache.
        assert_eq!(
            leftovers(dir.path()),
            vec![format!(".{}.partial", src.sha256)]
        );
        let partial = dir
            .path()
            .join("images")
            .join(format!(".{}.partial", src.sha256));
        assert_eq!(std::fs::read(&partial).unwrap().len(), 12);

        server.cut = None;
        server.hang = None;
        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // The second attempt asked for the rest, and only the rest: the whole
        // file was asked for once.
        assert_eq!(server.ranges.load(Ordering::SeqCst), 1);
        assert_eq!(server.whole.load(Ordering::SeqCst), 1);
        assert_eq!(leftovers(dir.path()), vec![format!("{}.qcow2", src.sha256)]);
    }

    #[tokio::test]
    async fn a_server_without_ranges_still_delivers_the_image() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"an image from a server that only sends whole files";
        let mut server = Server::new(bytes);
        server.cut = Some(9);
        // The connection dies after those bytes rather than the stream
        // simply ending: nothing more ever arrives.
        server.hang = Some(Duration::from_secs(60));
        let cache = ImageCache::alone(dir.path()).with_stall(Duration::from_millis(20));
        let src = source(bytes);

        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("broke off"), "{err:?}");

        // The server knows nothing about starting in the middle. It sends the
        // whole file and what was on disk is dropped for it.
        server.ranged = false;
        server.cut = None;
        server.hang = None;
        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(server.whole.load(Ordering::SeqCst), 2);
        assert_eq!(leftovers(dir.path()), vec![format!("{}.qcow2", src.sha256)]);
    }

    #[tokio::test]
    async fn an_answer_overlapping_the_bytes_on_disk_is_placed_right() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"an image whose rest arrives with some of it twice";
        let mut server = Server::new(bytes);
        server.cut = Some(15);
        // The connection dies after those bytes rather than the stream
        // simply ending: nothing more ever arrives.
        server.hang = Some(Duration::from_secs(60));
        let cache = ImageCache::alone(dir.path()).with_stall(Duration::from_millis(20));
        let src = source(bytes);
        assert!(cache.ensure(&server, &src).await.is_err());

        // A server that answers with bytes from a little before the byte it
        // was asked for. The repeated front is thrown away, not written twice.
        server.cut = None;
        server.hang = None;
        server.overlap = 4;
        let path = cache.ensure(&server, &src).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn a_resumed_file_that_does_not_match_is_refused_and_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"an image resumed from bytes the server cannot match";
        let mut server = Server::new(bytes);
        server.cut = Some(18);
        // The connection dies after those bytes rather than the stream
        // simply ending: nothing more ever arrives.
        server.hang = Some(Duration::from_secs(60));
        let cache = ImageCache::alone(dir.path()).with_stall(Duration::from_millis(20));
        let src = source(bytes);
        assert!(cache.ensure(&server, &src).await.is_err());

        // The rest arrives with one byte of it wrong. The digest covers the
        // whole file, so this is caught and nothing is kept.
        server.corrupt = true;
        server.cut = None;
        server.hang = None;
        let err = cache.ensure(&server, &src).await.unwrap_err();
        assert!(err.message.contains("match its digest"), "{err:?}");
        assert!(leftovers(dir.path()).is_empty(), "bytes were kept");
    }

    #[tokio::test]
    async fn an_image_nobody_uses_and_nobody_wants_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let old = source(b"an image nobody has started a machine from");
        let path = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", old.sha256));
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        std::fs::write(&path, b"old bytes").unwrap();
        aged(&path, 30);

        // The first sweep only learns the image is here.
        let first = cache.sweep(7).await.unwrap();
        assert!(first.deleted.is_empty(), "{first:?}");
        assert!(path.exists());

        let second = cache.sweep(7).await.unwrap();
        assert_eq!(second.deleted, vec![format!("{}.qcow2", old.sha256)]);
        assert_eq!(second.bytes, 9);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn an_image_used_since_the_keep_time_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let bytes = b"an image that was downloaded long ago and used again";
        let src = source(bytes);
        let server = Server::new(bytes);
        let path = cache.ensure(&server, &src).await.unwrap();
        // The file arrived long ago; asking for it again is what dates it as
        // used, and the cache records that whatever the file's own date says.
        aged(&path, 30);
        cache.ensure(&server, &src).await.unwrap();

        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();
        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(path.exists(), "an image in use was deleted");
        assert!(second
            .kept
            .iter()
            .any(|(_, why)| *why == "it has been used since the keep time"));
    }

    #[tokio::test]
    async fn an_image_a_machine_uses_is_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let used = source(b"an image a machine is running from");
        let cache = ImageCache::holding(dir.path(), clock.clone(), &[&used.sha256]);
        let unneeded = source(b"an image no machine has");
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        for src in [&used, &unneeded] {
            let path = dir
                .path()
                .join("images")
                .join(format!("{}.qcow2", src.sha256));
            std::fs::write(&path, b"bytes").unwrap();
            aged(&path, 30);
        }

        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();
        assert_eq!(second.deleted, vec![format!("{}.qcow2", unneeded.sha256)]);
        assert!(dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", used.sha256))
            .exists());
    }

    #[tokio::test]
    async fn an_image_behind_a_machines_disk_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let bytes = b"the base image a qcow2 disk is layered on";
        let src = source(bytes);
        let disks = dir.path().join("disks");
        std::fs::create_dir_all(disks.join("machines").join("pistack-machine")).unwrap();
        let cache_dir = dir.path().join("data").join("images");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached = cache_dir.join(format!("{}.qcow2", src.sha256));
        std::fs::write(&cached, bytes).unwrap();
        aged(&cached, 30);
        // A disk built on the image, named by the machine directory's disk.
        let disk = disks
            .join("machines")
            .join("pistack-machine")
            .join("disk.qcow2");
        std::fs::write(&disk, b"overlay").unwrap();
        let pins = Arc::new(MachinePins::with_info(
            [disks.clone()],
            cache_dir.clone(),
            OneBacking(cached.clone()),
        ));
        let cache = ImageCache {
            clock: clock.clone(),
            ..ImageCache::pinned(&dir.path().join("data"), pins)
        };
        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(cached.exists(), "an image behind a disk was deleted");
        assert!(second
            .kept
            .iter()
            .any(|(_, why)| *why == "a machine on this host uses it"));
    }

    #[tokio::test]
    async fn an_image_linked_as_a_machines_disk_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        // A machine disk made by linking a cached image is that image under a
        // second name, in a directory the sweep walks.
        let bytes = b"an image kept as a machine's disk under another name";
        let src = source(bytes);
        let disks = dir.path().join("disks");
        std::fs::create_dir_all(disks.join("machines").join("pistack-machine")).unwrap();
        let cache_dir = dir.path().join("data").join("images");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached = cache_dir.join(format!("{}.qcow2", src.sha256));
        std::fs::write(&cached, bytes).unwrap();
        aged(&cached, 30);
        let disk = disks
            .join("machines")
            .join("pistack-machine")
            .join("disk.qcow2");
        std::fs::hard_link(&cached, &disk).unwrap();

        let cache = ImageCache {
            clock: clock.clone(),
            ..ImageCache::pinned(
                &dir.path().join("data"),
                Arc::new(MachinePins::new([disks], cache_dir)),
            )
        };
        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            cached.exists(),
            "an image a machine's disk holds was deleted"
        );
    }

    #[tokio::test]
    async fn an_image_asked_for_during_a_sweep_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let bytes = b"an image a machine starts while the sweep is reading";
        let src = source(bytes);
        let server = Server::new(bytes);
        // Real pins, so an image no machine directory names is a candidate.
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let path = cache.ensure(&server, &src).await.unwrap();
        aged(&path, 30);

        // A download started now, so the cache is holding the image and a
        // download is in progress while a first sweep reads the directory.
        let downloading = cache.hold(&src.sha256);
        let first = cache.sweep(7).await.unwrap();
        assert!(first.deleted.is_empty(), "{first:?}");
        assert!(first
            .kept
            .iter()
            .any(|(_, why)| *why == "a download is in progress"));
        drop(downloading);

        // The download finishes: the request is recorded after the first sweep
        // saw the file. A second sweep reads that record and leaves the image.
        cache.ensure(&server, &src).await.unwrap();
        let second = cache.sweep(7).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(path.exists(), "an image wanted mid-sweep was deleted");
        assert!(second
            .kept
            .iter()
            .any(|(_, why)| *why == "it has been used since the keep time"));
    }

    #[tokio::test]
    async fn a_download_in_progress_is_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let bytes = b"an image arriving while the sweep is walking the cache";
        let src = source(bytes);
        let cache = ImageCache::dated(dir.path(), clock.clone());
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        let cached = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", src.sha256));
        std::fs::write(&cached, b"stale").unwrap();
        aged(&cached, 30);
        let partial = dir
            .path()
            .join("images")
            .join(format!(".{}.partial", src.sha256));
        std::fs::write(&partial, b"partial").unwrap();
        aged(&partial, 30);

        let held = cache.hold(&src.sha256);
        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();
        drop(held);

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(cached.exists(), "an image being downloaded was deleted");
        assert!(partial.exists(), "a partial file was deleted");
        assert!(second
            .kept
            .iter()
            .any(|(_, why)| *why == "a download is in progress"));
    }

    #[test]
    fn only_names_the_cache_made_are_ever_candidates() {
        assert_eq!(digest_of("ab".repeat(32).as_str()), Some("ab".repeat(32)));
        assert_eq!(
            digest_of(&format!("{}.qcow2", "cd".repeat(32))).as_deref(),
            Some("cd".repeat(32).as_str())
        );
        // A download in progress, a name with no digest in it, and someone
        // else's file are all left alone.
        assert_eq!(digest_of(&format!(".{}.partial", "ef".repeat(32))), None);
        assert_eq!(digest_of("console.log"), None);
        assert_eq!(digest_of("not-an-image.qcow2"), None);
        assert_eq!(digest_of(&"ab".repeat(31)), None);
    }

    #[test]
    fn a_keep_time_of_zero_is_a_real_answer() {
        assert_eq!(keep_days_from(None), Ok(DEFAULT_KEEP_DAYS));
        assert_eq!(keep_days_from(Some("30")), Ok(30));
        assert_eq!(keep_days_from(Some(" 7 ")), Ok(7));
        // Zero is not "unset" and not "keep everything": it says an unused
        // image is kept for nothing beyond the sweep that finds it unused.
        assert_eq!(keep_days_from(Some("0")), Ok(0));
    }

    #[test]
    fn a_keep_time_that_is_not_a_number_is_refused() {
        for bad in [
            "seven",
            "",
            "  ",
            "7 days",
            "-1",
            "1.5",
            "99999999999999999999999",
        ] {
            let err = keep_days_from(Some(bad)).unwrap_err();
            assert!(err.contains(KEEP_DAYS_ENV), "{bad:?} gave {err}");
            // The message has to say what to do, not only that it failed.
            assert!(err.contains("number"), "{bad:?} gave {err}");
        }
        // A bad value is refused, never quietly turned into the default.
        assert_ne!(keep_days_from(Some("seven")).ok(), Some(DEFAULT_KEEP_DAYS));
    }

    #[test]
    fn a_sweep_interval_that_is_not_seconds_is_refused() {
        assert_eq!(sweep_every_from(None), Ok(DEFAULT_SWEEP));
        assert_eq!(sweep_every_from(Some("60")), Ok(Duration::from_secs(60)));
        assert_eq!(sweep_every_from(Some("30")), Ok(Duration::from_secs(30)));
        // An interval of no seconds would sweep without pausing.
        let err = sweep_every_from(Some("0")).unwrap_err();
        assert!(err.contains(SWEEP_SECONDS_ENV), "{err}");
        for bad in ["soon", "", "-5", "30s"] {
            let err = sweep_every_from(Some(bad)).unwrap_err();
            assert!(err.contains(SWEEP_SECONDS_ENV), "{bad:?} gave {err}");
        }
    }

    #[tokio::test]
    async fn a_sweep_of_nothing_is_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::alone(dir.path());
        assert_eq!(cache.sweep(7).await.unwrap(), Sweep::default());
        // A cache with nothing in it has nothing to say either way.
        assert_eq!(cache.sweep(0).await.unwrap(), Sweep::default());
    }

    /// A stray the cache never made: a real qcow2 header under a name the
    /// cache would have made, left there by some other hand.
    fn stray(dir: &std::path::Path, days: u64) -> (ImageSource, PathBuf) {
        let src = source(qcow2_header());
        std::fs::create_dir_all(dir.join("images")).unwrap();
        let path = dir.join("images").join(format!("{}.qcow2", src.sha256));
        std::fs::write(&path, qcow2_header()).unwrap();
        aged(&path, days);
        (src, path)
    }

    fn qcow2_header() -> &'static [u8] {
        b"QFI\xfb\x00\x00\x00\x03 an image planted in the cache directory"
    }

    #[tokio::test]
    async fn a_stray_is_dated_from_its_own_file_not_from_the_sweep_that_found_it() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let (src, path) = stray(dir.path(), 10);
        let name = format!("{}.qcow2", src.sha256);

        // The first sweep only learns the file is here. It cannot delete what
        // it has just met, whatever the file's date says.
        let first = cache.sweep(7).await.unwrap();
        assert_eq!(first.seen, 1, "{first:?}");
        assert!(first.deleted.is_empty(), "{first:?}");
        assert!(first
            .kept
            .iter()
            .any(|(kept, why)| *kept == name && *why == WAITING));
        assert!(path.exists());

        // The second sweep has ten days of quiet to judge by, because the
        // adoption dated the stray from its own modification time and not from
        // the sweep that found it.
        let second = cache.sweep(7).await.unwrap();
        assert_eq!(second.deleted, vec![name.clone()], "{second:?}");
        assert!(!path.exists(), "an old stray was kept");
    }

    #[tokio::test]
    async fn a_stray_that_just_arrived_waits_out_the_keep_time() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let (src, path) = stray(dir.path(), 0);
        let name = format!("{}.qcow2", src.sha256);

        cache.sweep(7).await.unwrap();
        let second = cache.sweep(7).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(path.exists(), "a stray that just arrived was deleted");
        assert!(second
            .kept
            .iter()
            .any(|(kept, why)| *kept == name && *why == "it has been used since the keep time"));
    }

    #[tokio::test]
    async fn a_keep_time_of_zero_removes_a_stray_the_sweep_after_it_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let cache = ImageCache::dated(dir.path(), clock.clone());
        let (src, path) = stray(dir.path(), 0);
        let name = format!("{}.qcow2", src.sha256);

        // Zero is not the old "keep everything": it is a quiet time of nothing.
        // The two-sweep rule still holds, so the first sweep only finds it.
        let first = cache.sweep(0).await.unwrap();
        assert!(first.deleted.is_empty(), "{first:?}");
        assert!(path.exists(), "a file was deleted by the sweep that met it");

        let second = cache.sweep(0).await.unwrap();
        assert_eq!(second.deleted, vec![name], "{second:?}");
        assert!(!path.exists(), "keep_days=0 kept an unused image");
    }

    #[tokio::test]
    async fn a_keep_time_of_zero_still_keeps_what_a_machine_uses() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let used = source(b"an image a running machine is backed by");
        let cache = ImageCache::holding(dir.path(), clock.clone(), &[&used.sha256]);
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        let held = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", used.sha256));
        std::fs::write(&held, b"bytes").unwrap();

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(held.exists(), "keep_days=0 deleted a working image");
        assert_eq!(second.count(Keep::Held), 1, "{second:?}");
    }

    /// A sweep is judged by what it deletes, and reported by what it counted.
    #[tokio::test]
    async fn a_sweep_counts_what_it_saw_kept_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let used = source(b"an image a running machine is backed by");
        let gone = source(qcow2_header());
        let pins: Arc<dyn Pins> = Arc::new(Held([used.sha256.clone()].into()));
        let cache = ImageCache {
            clock: clock.clone(),
            ..ImageCache::pinned(dir.path(), pins)
        };
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        // Someone else's file, dated long ago, is not the cache's to count.
        let not_ours = dir.path().join("images").join("no-place.log");
        std::fs::write(&not_ours, b"x").unwrap();
        aged(&not_ours, 30);
        for (src, bytes) in [
            (&used, b"in use".to_vec()),
            (&gone, qcow2_header().to_vec()),
        ] {
            let path = dir
                .path()
                .join("images")
                .join(format!("{}.qcow2", src.sha256));
            std::fs::write(&path, bytes).unwrap();
            aged(&path, 30);
        }

        // A sweep that decides nothing still counts what it looked at. A held
        // image is re-armed by every sweep, so it waits in the first one too.
        let first = cache.sweep(7).await.unwrap();
        assert_eq!(first.seen, 2, "{first:?}");
        assert_eq!(first.count(Keep::Waiting), 1, "{first:?}");
        assert_eq!(first.count(Keep::Held), 1, "{first:?}");
        assert!(first.deleted.is_empty(), "{first:?}");
        assert!(first.spoke(), "{first:?}");

        let second = cache.sweep(7).await.unwrap();
        assert_eq!(second.seen, 2, "{second:?}");
        assert_eq!(second.count(Keep::Held), 1, "{second:?}");
        assert_eq!(second.count(Keep::Waiting), 0, "{second:?}");
        assert_eq!(second.deleted, vec![format!("{}.qcow2", gone.sha256)]);
        assert_eq!(second.bytes, qcow2_header().len() as u64, "{second:?}");

        // Nothing left but a working disk and someone else's file: the sweep
        // has no news, and says nothing above debug.
        let third = cache.sweep(7).await.unwrap();
        assert_eq!(third.seen, 1, "{third:?}");
        assert_eq!(third.count(Keep::Held), 1, "{third:?}");
        assert!(!third.spoke(), "{third:?}");
        assert!(second.spoke(), "a sweep that deleted says nothing");
        assert!(
            not_ours.exists(),
            "a file the cache did not make was touched"
        );
    }

    /// The test's stand-in for a host's `qemu-img`: the chain of each disk is
    /// read out of `chains`, whose name is the disk's own. A disk with no chain
    /// written for it is a disk the host could not open.
    #[derive(Default)]
    struct Chains(std::sync::Mutex<HashMap<String, Vec<PathBuf>>>);

    #[cfg(test)]
    impl Chains {
        fn set(&self, disk: &Path, chain: &[PathBuf]) {
            let name = disk
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name, chain.to_vec());
        }
    }

    #[cfg(test)]
    #[async_trait]
    impl DiskInfo for Arc<Chains> {
        async fn chain_of(&self, disk: &Path) -> ApiResult<Vec<PathBuf>> {
            let name = disk
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&name)
                .cloned()
                .ok_or_else(|| {
                    ApiError::internal(format!(
                        "Could not open '{}': Failed to get \"write\" lock. Is another process using the image?",
                        disk.display()
                    ))
                })
        }
    }

    /// The test's stand-in for a `qemu-img` that is not installed at all.
    #[cfg(test)]
    struct NoTool;

    #[cfg(test)]
    #[async_trait]
    impl DiskInfo for NoTool {
        async fn chain_of(&self, disk: &Path) -> ApiResult<Vec<PathBuf>> {
            Err(ApiError::unavailable(format!(
                "qemu-img is not installed (No such file or directory): {}",
                disk.display()
            )))
        }
    }

    /// A machine's disk in the libvirt runtime's own layout: the domain's
    /// directory, and the disk inside it.
    fn disk_of(machines: &Path, domain: &str) -> PathBuf {
        let disk = machines.join(domain).join("disk.qcow2");
        std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
        std::fs::write(&disk, b"overlay").unwrap();
        disk
    }

    /// The cache and pins of a test's host: machine files under `disks`, the
    /// cache under `data/images`, the records of known machines under
    /// `data/machines`, and `info` answering for the host's tool.
    fn host(dir: &Path, clock: Arc<FixedClock>, info: impl DiskInfo + 'static) -> ImageCache {
        let records: Vec<PathBuf> = std::fs::read_dir(dir.join("data").join("machines"))
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path().join("machine.json"))
                    .collect()
            })
            .unwrap_or_default();
        let cache_dir = dir.join("data").join("images");
        std::fs::create_dir_all(&cache_dir).unwrap();
        ImageCache {
            clock,
            ..ImageCache::pinned(
                &dir.join("data"),
                Arc::new(MachinePins::looking_at(
                    [dir.join("disks")],
                    cache_dir,
                    info,
                    records,
                )),
            )
        }
    }

    #[tokio::test]
    async fn a_disk_that_cannot_be_read_stops_the_sweep_deleting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let wanted = source(b"an image behind a disk the host cannot open");
        let stray = source(qcow2_header());
        let info = Arc::new(Chains::default());
        let cache = host(dir.path(), clock.clone(), info.clone());
        let cached = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", wanted.sha256));
        let other = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        for path in [&cached, &other] {
            std::fs::write(path, b"bytes").unwrap();
            aged(path, 30);
        }
        // The disk of a machine that is running: the host cannot open it at all.
        let disk = disk_of(&dir.path().join("disks"), "pistack-live");

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            cached.exists(),
            "a base image went with a disk that could not be read"
        );
        assert!(
            other.exists(),
            "an image went on a sweep that could not read the machines"
        );
        assert_eq!(second.count(Keep::Blind), 2, "{second:?}");
        assert_eq!(second.count(Keep::Held), 0, "{second:?}");
        assert_eq!(second.count(Keep::Waiting), 0, "{second:?}");
        assert_eq!(second.blind.len(), 1, "{second:?}");
        assert!(second.blind[0].contains("disk.qcow2"), "{:?}", second.blind);
        assert!(second.spoke(), "a sweep that could not read says nothing");

        // The disk can be read again: the next sweep reads everything, and the
        // image nothing is behind is old enough to go.
        info.set(&disk, std::slice::from_ref(&cached));
        let third = cache.sweep(0).await.unwrap();
        assert_eq!(
            third.deleted,
            vec![format!("{}.qcow2", stray.sha256)],
            "{third:?}"
        );
        assert_eq!(third.count(Keep::Held), 1, "{third:?}");
        assert!(
            cached.exists(),
            "an image named by a readable disk was deleted"
        );
        assert!(third.blind.is_empty(), "{third:?}");
    }

    #[tokio::test]
    async fn a_machine_directory_that_cannot_be_opened_stops_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let stray = source(qcow2_header());
        let cache = host(dir.path(), clock.clone(), NoTool);
        let other = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&other, b"bytes").unwrap();
        aged(&other, 30);
        // A machine directory the agent's own user cannot read: it may hold any
        // number of disks layered on images this cache holds.
        let shut = dir.path().join("disks").join("shut");
        std::fs::create_dir_all(&shut).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            other.exists(),
            "an image went on a sweep that could not list a machine directory"
        );
        assert!(
            second.blind.iter().any(|why| why.contains("shut")),
            "{:?}",
            second.blind
        );
    }

    #[tokio::test]
    async fn a_missing_tool_stops_the_sweep_deleting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let stray = source(qcow2_header());
        let cache = host(dir.path(), clock.clone(), NoTool);
        let other = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&other, b"bytes").unwrap();
        aged(&other, 30);
        disk_of(&dir.path().join("disks"), "pistack-live");

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            other.exists(),
            "an image went where the tool to ask was missing"
        );
        assert!(
            second.blind[0].contains("not installed"),
            "{:?}",
            second.blind
        );
    }

    #[tokio::test]
    async fn the_whole_chain_behind_a_disk_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let near = source(b"the image the machine's disk is layered on");
        let far = source(b"the image behind that one");
        let info = Arc::new(Chains::default());
        let cache = host(dir.path(), clock.clone(), info.clone());
        let cache_dir = dir.path().join("data").join("images");
        let near_path = cache_dir.join(format!("{}.qcow2", near.sha256));
        let far_path = cache_dir.join(format!("{}.qcow2", far.sha256));
        for path in [&near_path, &far_path] {
            std::fs::write(path, b"bytes").unwrap();
            aged(path, 30);
        }
        let disk = disk_of(&dir.path().join("disks"), "pistack-chain");
        info.set(&disk, &[near_path.clone(), far_path.clone()]);

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(near_path.exists(), "the image behind a disk was deleted");
        assert!(far_path.exists(), "the image behind that image was deleted");
        assert_eq!(second.count(Keep::Held), 2, "{second:?}");
    }

    /// Every runtime's own machine directory is walked, and a host that moved
    /// one by name is walked there too: a sweep that could not see a runtime's
    /// disks is a sweep that deletes nothing.
    #[tokio::test]
    async fn a_sweep_walks_every_runtime_it_knows() {
        let _lock = crate::runtime::youki::youki_env_lock().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let moved = dir.path().join("libvirt-images");
        let keys = [
            "PISTACK_LIBVIRT_DIR",
            "PISTACK_CLOUD_STATE",
            "PISTACK_YOUKI_STATE",
            "PISTACK_METAL_IMAGES",
        ];
        let prev: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(*k))).collect();
        std::env::set_var("PISTACK_LIBVIRT_DIR", &moved);
        std::env::set_var("PISTACK_CLOUD_STATE", dir.path().join("cloud"));
        std::env::set_var("PISTACK_YOUKI_STATE", dir.path().join("youki"));
        std::env::set_var("PISTACK_METAL_IMAGES", dir.path().join("metal-images"));
        for part in ["cloud", "youki", "metal-images"] {
            std::fs::create_dir_all(dir.path().join(part)).unwrap();
        }
        // A machine of the libvirt runtime, under the path this host named for it.
        std::fs::create_dir_all(moved.join("machines").join("pistack-m1")).unwrap();

        let dirs = machine_dirs(&data);
        for (k, v) in prev {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }

        assert!(
            dirs.contains(&moved.join("machines")),
            "the libvirt runtime's machines are not walked: {dirs:?}"
        );
        assert!(
            dirs.contains(&dir.path().join("cloud")),
            "cloud-hypervisor's machine directory is not walked: {dirs:?}"
        );
        assert!(
            dirs.contains(&dir.path().join("youki")),
            "the container runtime's state directory is not walked: {dirs:?}"
        );
        assert!(
            dirs.contains(&dir.path().join("metal-images")),
            "the metal runtime's image directory is not walked: {dirs:?}"
        );
    }

    /// A runtime this host does not run keeps no machine files at all, so a
    /// directory that is not there is walked into nothing rather than read as a
    /// blind spot. The fault is a directory that is there and cannot be opened,
    /// which is its own test.
    #[tokio::test]
    async fn an_absent_runtime_directory_blinds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = host(dir.path(), FixedClock::now_ahead(), NoTool);
        let stray = source(qcow2_header());
        let other = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&other, b"bytes").unwrap();
        aged(&other, 30);

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        // No disk exists anywhere, so the missing tool never mattered and this
        // sweep read everything it had to.
        assert_eq!(
            second.deleted,
            vec![format!("{}.qcow2", stray.sha256)],
            "{second:?}"
        );
        assert!(second.blind.is_empty(), "{second:?}");
    }

    #[tokio::test]
    async fn an_image_a_machine_is_being_built_from_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let wanted = source(b"an image a machine is being provisioned from");
        std::fs::create_dir_all(dir.path().join("data").join("machines").join("mach-1")).unwrap();
        let cache = host(dir.path(), clock.clone(), NoTool);
        // A record naming the image, written after the pins were made so its
        // path is the one they read.
        let record = dir
            .path()
            .join("data")
            .join("machines")
            .join("mach-1")
            .join("machine.json");
        let cached = dir
            .path()
            .join("data")
            .join("images")
            .join(format!("{}.qcow2", wanted.sha256));
        std::fs::write(&cached, b"bytes").unwrap();
        aged(&cached, 30);
        std::fs::write(
            &record,
            format!(r#"{{"id":"mach-1","image":"{}"}}"#, cached.display()).as_bytes(),
        )
        .unwrap();

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            cached.exists(),
            "an image a machine was being built from was deleted"
        );
        assert_eq!(second.count(Keep::Held), 1, "{second:?}");

        // A record that cannot be read is as unknown as a disk that cannot be
        // probed.
        std::fs::remove_file(&record).unwrap();
        let third = cache.sweep(0).await.unwrap();
        assert!(third.deleted.is_empty(), "{third:?}");
        assert!(
            cached.exists(),
            "an image went with a machine record unread"
        );
        assert!(
            third.blind.iter().any(|why| why.contains("machine.json")),
            "{:?}",
            third.blind
        );
    }

    /// The file set one libvirt machine directory holds on a real host, beside
    /// its disk: the domain definition, the seed, the cloud-init files, and the
    /// console the guest writes to. None of them is a disk.
    const LIBVIRT_MACHINE_FILES: [&str; 6] = [
        "domain.xml",
        "seed.iso",
        "user-data",
        "network-config",
        "meta-data",
        "console.log",
    ];

    /// Writes one machine directory the way the libvirt runtime fills it.
    fn libvirt_machine(machines: &Path, domain: &str) -> PathBuf {
        let vm = machines.join(domain);
        std::fs::create_dir_all(&vm).unwrap();
        for name in LIBVIRT_MACHINE_FILES {
            std::fs::write(vm.join(name), b"not a disk").unwrap();
        }
        vm
    }

    /// A host laid out like a running libvirt machine: the runtime's machine
    /// directories under `dir/libvirt-images/machines`, the cache at
    /// `dir/images`, and no machine records directory at all — a host whose
    /// agent keeps no records of its own, which is the common case and the one
    /// that must not be read as a fault.
    ///
    /// Holds the environment lock until the returned guard is dropped, which is
    /// what keeps this host's `qemu-img` stand-in named as the probe's tool
    /// while its cache is swept: the probe reads the tool's name when it asks,
    /// not when the pins are made.
    async fn real_layout(dir: &Path, clock: Arc<FixedClock>) -> (ImageCache, EnvGuard) {
        let hold = crate::runtime::youki::youki_env_lock().lock().await;
        let libvirt = dir.join("libvirt-images");
        std::fs::create_dir_all(libvirt.join("machines")).unwrap();
        let tool = dir.join("qemu-img");
        write_exec(&tool, &qemu_img_stand_in());
        let keys = [
            "PISTACK_LIBVIRT_DIR",
            "PISTACK_QEMU_IMG",
            "PISTACK_DATA_DIR",
            "PISTACK_CLOUD_STATE",
            "PISTACK_YOUKI_STATE",
            "PISTACK_METAL_IMAGES",
        ];
        let prev: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(*k))).collect();
        std::env::set_var("PISTACK_LIBVIRT_DIR", &libvirt);
        std::env::set_var("PISTACK_QEMU_IMG", &tool);
        std::env::set_var("PISTACK_DATA_DIR", dir);
        // A host running no other runtime keeps no machine directory for it, and
        // says so by pointing each at a directory that does not exist.
        std::env::set_var("PISTACK_CLOUD_STATE", dir.join("no-cloud-hypervisor"));
        std::env::set_var("PISTACK_YOUKI_STATE", dir.join("no-containers"));
        std::env::set_var("PISTACK_METAL_IMAGES", dir.join("no-metal"));
        let pins = MachinePins::for_agent(dir);
        let cache = ImageCache {
            clock,
            ..ImageCache::pinned(dir, Arc::new(pins))
        };
        (cache, EnvGuard { _hold: hold, prev })
    }

    /// A test's hold on the process's runtime environment. The host it names
    /// stays this host until the guard is dropped.
    struct EnvGuard {
        _hold: tokio::sync::MutexGuard<'static, ()>,
        prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.prev.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// The tool of a host laid out by `real_layout`, hidden from a sweep. A
    /// probe against a hidden tool says it is not installed, which is the fault
    /// a missing dependency makes. `show_tool` puts it back.
    fn hide_tool(dir: &Path) {
        let tool = dir.join("qemu-img");
        std::fs::rename(&tool, tool.with_extension("hidden")).unwrap();
        // Named at a path that is not there: an empty variable would be read as
        // "use the `qemu-img` on PATH", which is a different host.
        std::env::set_var("PISTACK_QEMU_IMG", &tool);
    }

    /// Puts a hidden tool back where the host named it.
    fn show_tool(dir: &Path) {
        let tool = dir.join("qemu-img");
        std::fs::rename(tool.with_extension("hidden"), &tool).unwrap();
        std::env::set_var("PISTACK_QEMU_IMG", &tool);
    }

    /// The script a `qemu-img` stand-in is written from. A machine's
    /// `disk.qcow2` answers flat when its directory holds a `flat` marker, with
    /// the base named in `backing` when one is there, and is a disk the tool
    /// refuses to open when the test has said nothing about it at all.
    /// Anything else answers as a whole image with nothing behind it, which is
    /// how a chain ends on a real host — and what a sweep must not read as a
    /// fault when it asks after a file that is not a disk at all.
    fn qemu_img_stand_in() -> String {
        r#"#!/bin/sh
disk=""
prev=""
for a in "$@"; do
  case "$prev" in
    -b|-F) prev=$a; continue ;;
    *) : ;;
  esac
  prev=$a
  case "$a" in
    -*) continue ;;
  esac
  case "$a" in
    */disk.qcow2) disk=$a ;;
    *) : ;;
  esac
done
if [ -n "$disk" ]; then
  d=$(dirname "$disk")
  if [ -f "$d/flat" ]; then
    printf '%s\n' '{"format":"qcow2","virtual-size":10737418240}'
    exit 0
  fi
  base=$(cat "$d/backing" 2>/dev/null)
  if [ -n "$base" ]; then
    printf '%s\n' "{\"format\":\"qcow2\",\"virtual-size\":10737418240,\"backing-filename\":\"$base\"}"
    exit 0
  fi
  echo "Could not open '$disk': Failed to get \"write\" lock." >&2
  exit 1
fi
printf '%s\n' '{"format":"qcow2","virtual-size":1}'
"#
        .to_owned()
    }

    /// Says what a machine's disk is built on. The name is written the way a
    /// real qcow2 writes one: relative, and resolved beside its disk.
    fn built_on(vm: &Path, base: &Path) {
        let name = base
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::fs::write(vm.join("backing"), format!("../../images/{name}\n")).unwrap();
    }

    /// Says a machine's disk is a whole image with nothing behind it. A disk the
    /// test says nothing about at all keeps its guest holding the write lock.
    fn standing_alone(vm: &Path) {
        std::fs::write(vm.join("flat"), b"").unwrap();
    }

    /// Writes a script the sweep's probe can run.
    fn write_exec(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A machine's disk file. What it is built on is said by `built_on` or
    /// `standing_alone`; until then the tool refuses to open it.
    fn machine_disk(machines: &Path, domain: &str) -> PathBuf {
        let disk = machines.join(domain).join("disk.qcow2");
        std::fs::write(&disk, b"overlay").unwrap();
        disk
    }

    /// A real libvirt host: every machine directory holds six files that are
    /// not disks and one that is. A sweep that asked after the six would never
    /// clean anything, so the disk's image is held, an old stray goes, and the
    /// sweep is not blind.
    #[tokio::test]
    async fn only_a_machines_disk_is_asked_about_on_a_real_layout() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        let machines = dir.path().join("libvirt-images").join("machines");
        let live = libvirt_machine(&machines, "pistack-live");
        let other = libvirt_machine(&machines, "pistack-other");

        let used = source(b"the image two running machines are layered on");
        let stray = source(qcow2_header());
        let cache_dir = dir.path().join("images");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached = cache_dir.join(format!("{}.qcow2", used.sha256));
        let gone = cache_dir.join(format!("{}.qcow2", stray.sha256));
        for path in [&cached, &gone] {
            std::fs::write(path, b"bytes").unwrap();
            aged(path, 30);
        }
        machine_disk(&machines, "pistack-live");
        machine_disk(&machines, "pistack-other");
        built_on(&live, &cached);
        built_on(&other, &cached);
        // The seven files a machine directory holds on a host, laid out as the
        // runtime lays them out.
        for vm in [&live, &other] {
            for name in LIBVIRT_MACHINE_FILES {
                assert!(
                    vm.join(name).is_file(),
                    "{} missing",
                    vm.join(name).display()
                );
            }
            assert!(vm.join("disk.qcow2").is_file());
        }

        // A sweep that judged a disk and nothing else reads the whole keep time
        // before it deletes, so the first sweep only learns what is where.
        let first = cache.sweep(0).await.unwrap();
        assert!(first.blind.is_empty(), "{:?}", first.blind);
        let second = cache.sweep(0).await.unwrap();

        assert!(second.blind.is_empty(), "{:?}", second.blind);
        assert_eq!(
            second.deleted,
            vec![format!("{}.qcow2", stray.sha256)],
            "{second:?}"
        );
        assert!(cached.exists(), "the image two machines use went");
        assert_eq!(second.count(Keep::Held), 1, "{second:?}");
        assert!(
            !second.blind.iter().any(|why| why.contains("seed.iso")),
            "the sweep still probes a seed as a disk: {:?}",
            second.blind
        );
    }

    /// The agent's own directories are not on every host: a runtime that keeps
    /// no machines here keeps no directory at all, and `/var/lib/pistack/agent`
    /// holds no `machines` or `disks` beside its cache. A sweep reads that as a
    /// host with nothing running, not as a read it could not make.
    #[tokio::test]
    async fn a_host_with_no_agent_machine_directory_is_not_blind() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        for part in ["machines", "disks"] {
            assert!(
                !dir.path().join(part).exists(),
                "the test's host must not hold the directory it asks about"
            );
        }
        let machines = dir.path().join("libvirt-images").join("machines");
        let live = libvirt_machine(&machines, "pistack-live");
        machine_disk(&machines, "pistack-live");
        let used = source(b"the image one machine is layered on");
        let stray = source(qcow2_header());
        let cached = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", used.sha256));
        let gone = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        for path in [&cached, &gone] {
            std::fs::write(path, b"bytes").unwrap();
            aged(path, 30);
        }
        built_on(&live, &cached);

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.blind.is_empty(), "{:?}", second.blind);
        assert_eq!(
            second.deleted,
            vec![format!("{}.qcow2", stray.sha256)],
            "{second:?}"
        );
        assert!(cached.exists(), "the one machine's image went");
    }

    /// A disk file that is a whole image with nothing behind it is not built on
    /// anything in the cache. That is the disk answering, not the host failing
    /// to answer, and the sweep goes on to delete.
    #[tokio::test]
    async fn a_flat_disk_holds_nothing_and_blinds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        let machines = dir.path().join("libvirt-images").join("machines");
        let plain = libvirt_machine(&machines, "pistack-plain");
        machine_disk(&machines, "pistack-plain");
        let stray = source(qcow2_header());
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        let gone = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&gone, b"bytes").unwrap();
        aged(&gone, 30);
        // A disk built on nothing: the tool answers with a format and no base.
        standing_alone(&plain);

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.blind.is_empty(), "{:?}", second.blind);
        assert_eq!(
            second.deleted,
            vec![format!("{}.qcow2", stray.sha256)],
            "{second:?}"
        );
    }

    /// A machine directory with the domain definition and no disk of the
    /// runtime's name: the disk is somewhere this sweep is not looking, so
    /// whatever it is built on stays.
    #[tokio::test]
    async fn a_machine_definition_with_no_disk_file_blinds_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        let machines = dir.path().join("libvirt-images").join("machines");
        // The six files of a machine, and no `disk.qcow2` among them.
        libvirt_machine(&machines, "pistack-moved");
        let stray = source(qcow2_header());
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        let gone = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&gone, b"bytes").unwrap();
        aged(&gone, 30);

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            gone.exists(),
            "an image went on a sweep that could not find a machine's disk"
        );
        assert!(
            second.blind.iter().any(|why| why.contains("pistack-moved")),
            "{:?}",
            second.blind
        );
    }

    /// Two faults, one fail-safe. A disk the host will not open — its guest
    /// holds the write lock, so a plain `qemu-img info` refuses it — and a tool
    /// that is not there at all both leave the sweep unable to say what is in
    /// use, and neither deletes anything.
    #[tokio::test]
    async fn a_disk_that_cannot_be_probed_on_a_real_layout_blinds_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        let machines = dir.path().join("libvirt-images").join("machines");
        libvirt_machine(&machines, "pistack-locked");
        let used = source(b"the image behind the disk nobody can open");
        let stray = source(qcow2_header());
        let cache_dir = dir.path().join("images");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached = cache_dir.join(format!("{}.qcow2", used.sha256));
        let gone = cache_dir.join(format!("{}.qcow2", stray.sha256));
        for path in [&cached, &gone] {
            std::fs::write(path, b"bytes").unwrap();
            aged(path, 30);
        }
        // The disk is there and the guest holds its own write lock.
        machine_disk(&machines, "pistack-locked");

        let second = cache.sweep(0).await.unwrap();
        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            cached.exists(),
            "a base image went with a disk that could not be probed"
        );
        assert!(
            gone.exists(),
            "an image went on a sweep that could not probe a disk"
        );
        assert!(
            second.blind.iter().any(|why| why.contains("disk.qcow2")),
            "{:?}",
            second.blind
        );

        // The tool itself is gone: the same fail-safe, named as a missing tool.
        hide_tool(dir.path());
        let third = cache.sweep(0).await.unwrap();
        show_tool(dir.path());

        assert!(third.deleted.is_empty(), "{third:?}");
        assert!(cached.exists(), "a base image went with no tool to ask");
        assert!(gone.exists(), "an image went with no tool to ask");
        assert!(
            third.blind.iter().any(|why| why.contains("not installed")), // dbg
            "{:?}",
            third.blind
        );
    }

    /// A machine directory that is there and cannot be listed is a fault, not
    /// an empty runtime: it may hold any number of disks layered on this
    /// cache's images.
    #[tokio::test]
    async fn a_machine_directory_that_cannot_be_listed_on_a_real_layout_blinds() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FixedClock::now_ahead();
        let (cache, _env) = real_layout(dir.path(), clock.clone()).await;
        let machines = dir.path().join("libvirt-images").join("machines");
        let shut = libvirt_machine(&machines, "pistack-shut");
        let stray = source(qcow2_header());
        std::fs::create_dir_all(dir.path().join("images")).unwrap();
        let gone = dir
            .path()
            .join("images")
            .join(format!("{}.qcow2", stray.sha256));
        std::fs::write(&gone, b"bytes").unwrap();
        aged(&gone, 30);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();

        cache.sweep(0).await.unwrap();
        let second = cache.sweep(0).await.unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(second.deleted.is_empty(), "{second:?}");
        assert!(
            gone.exists(),
            "an image went on a sweep that could not list a machine directory"
        );
        assert!(
            second.blind.iter().any(|why| why.contains("pistack-shut")),
            "{:?}",
            second.blind
        );
    }
}

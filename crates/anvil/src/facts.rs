// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host facts: what this box is and what it can run.
//!
//! Everything here is best-effort and never fails: a value that cannot be
//! read falls back to a conservative default, because an agent that
//! refuses to start on an odd kernel is worse than one that reports 0 GPUs.

use pistack_types::Resources;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// Where the agent keeps images, disks and state. Disk capacity is the
/// free space of the filesystem holding it.
pub const DATA_DIR: &str = "/var/lib/pistack";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFacts {
    pub node_name: String,
    pub resources: Resources,
    /// Detected drivers: "kvm", "libvirt", "youki", "ovs". The scheduler
    /// maps machine kinds onto these.
    pub capabilities: Vec<String>,
}

/// Gather everything. Cheap; safe to call on every heartbeat.
pub fn gather() -> HostFacts {
    gather_at(Path::new(DATA_DIR))
}

/// Like [`gather`] with an explicit data directory (tests, `--data-dir`).
pub fn gather_at(data_dir: &Path) -> HostFacts {
    HostFacts {
        node_name: node_name(),
        resources: Resources {
            vcpus: cpus(),
            memory_mib: memory_mib().unwrap_or(0),
            disk_gib: disk_gib(data_dir)
                .or_else(|| disk_gib(Path::new("/")))
                .unwrap_or(0),
            gpus: 0,
        },
        capabilities: capabilities(),
    }
}

/// The host name, as a node name. The agent itself runs under the stable
/// name from [`pistack_types::node`]; this is only the first-start default.
pub fn node_name() -> String {
    pistack_types::node::system_hostname().unwrap_or_else(|| "localhost".to_owned())
}

pub fn cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// `MemTotal` from /proc/meminfo, in MiB.
pub fn memory_mib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&s)
}

fn parse_meminfo(s: &str) -> Option<u64> {
    s.lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb / 1024)
}

/// Free space (in GiB) on the filesystem that holds `path`. `None` when the
/// path does not exist or statvfs fails.
pub fn disk_gib(path: &Path) -> Option<u64> {
    if !path.exists() {
        return None;
    }
    let c = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: `statvfs` writes into the zeroed struct we hand it and reads
    // only the NUL-terminated path; both outlive the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    let frsize = if st.f_frsize > 0 {
        stat_u64(st.f_frsize)
    } else {
        stat_u64(st.f_bsize)
    };
    Some(stat_u64(st.f_bavail).saturating_mul(frsize) / (1024 * 1024 * 1024))
}

/// `statvfs` field types differ by OS. Widen through `i128` so the value is
/// unchanged when it already fits in `u64`.
fn stat_u64(n: impl Into<i128>) -> u64 {
    n.into() as u64
}

/// Whether a directory is on a filesystem other hosts mount too. The
/// directory is shared when it is such a mount or sits under one. Only NFS
/// and SMB count as network filesystems, so anything else, a local disk
/// included and a directory that is not there, is read as local.
pub fn on_shared_filesystem(path: &Path) -> bool {
    mounts_shared(
        std::fs::read_to_string("/proc/mounts")
            .unwrap_or_default()
            .as_bytes(),
        path,
    )
}

/// The part of [`on_shared_filesystem`] that reads nothing: `mounts` is
/// `/proc/mounts` bytes, `path` is the directory being asked about.
///
/// A filesystem this kernel mounts under another name is read as local.
fn mounts_shared(mounts: &[u8], path: &Path) -> bool {
    let wanted = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let wanted = wanted.to_string_lossy();
    let wanted = wanted.trim_end_matches('/');
    let wanted = if wanted.is_empty() { "/" } else { wanted };
    let unescape = |src: &str| src.replace("\\040", " ").replace("\\011", "\t");
    for line in String::from_utf8_lossy(mounts).lines() {
        let mut fields = line.split_whitespace();
        let (Some(_device), Some(dir), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !matches!(unescape(fstype).as_str(), "nfs" | "nfs4" | "cifs" | "smb3") {
            continue;
        }
        let dir = unescape(dir);
        let dir = dir.trim_end_matches('/');
        let dir = if dir.is_empty() { "/" } else { dir };
        if dir == "/" || dir == wanted || starts_with(wanted, dir) {
            return true;
        }
    }
    false
}

/// Whether `path` sits below `dir`, on `/` boundaries. A directory is not
/// below itself: `/srv` is under the mount at `/mnt/srv`, not the one at
/// `/srv1`.
fn starts_with(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
}

/// Which drivers this host can offer.
pub fn capabilities() -> Vec<String> {
    let mut caps = Vec::new();
    if Path::new("/dev/kvm").exists() {
        caps.push("kvm".to_owned());
    }
    if which("youki").is_some() {
        caps.push("youki".to_owned());
    }
    if which("virsh").is_some() {
        caps.push("libvirt".to_owned());
    }
    if which("ovs-vsctl").is_some() {
        caps.push("ovs".to_owned());
    }
    caps
}

/// First executable named `name` on `$PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parses() {
        let s = "MemTotal:       16318296 kB\nMemFree:         1234 kB\n";
        assert_eq!(parse_meminfo(s), Some(15935));
        assert_eq!(parse_meminfo("garbage"), None);
    }

    #[test]
    fn disk_of_root_is_readable() {
        assert!(disk_gib(Path::new("/")).is_some());
        assert!(disk_gib(Path::new("/definitely/not/here")).is_none());
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("no-such-binary-xyz").is_none());
    }

    #[test]
    fn gather_never_panics() {
        let f = gather_at(Path::new("/nonexistent-data-dir"));
        assert!(f.resources.vcpus >= 1);
        assert!(!f.node_name.is_empty());
    }

    const MOUNTS: &[u8] =
        b"sysfs /sys sysfs rw 0 0\n/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /srv ext4 rw 0 0\n\
          server:/exports /mnt/nfs4 nfs4 rw 0 0\n//server/share /mnt/cifs cifs rw 0 0\n\
          //server/esc /mnt/my\\040dir cifs rw 0 0\nserver:/srv /srv/nested nfs rw 0 0\n\
          server:/y /srv/sub/deeper nfs rw 0 0\n";

    fn shared(dir: &str) -> bool {
        mounts_shared(MOUNTS, Path::new(dir))
    }

    #[test]
    fn a_local_directory_is_not_shared() {
        assert!(!shared("/"));
        assert!(!shared("/var/lib/libvirt/images/pistack"));
        assert!(!shared("/srv"));
        assert!(!shared("/srv1"), "a mount at /srv1 does not cover /srv");
        assert!(!shared("relative/dir"));
        assert!(!shared("/definitely/not/here"));
        assert!(
            !shared("/mnt"),
            "a parent of the mount holds no disk of its own"
        );
    }

    #[test]
    fn a_directory_on_a_network_filesystem_is_shared() {
        assert!(shared("/mnt/nfs4/pistack"));
        assert!(shared("/mnt/cifs/pistack"));
        assert!(shared("/mnt/cifs"), "the mount itself holds the disks");
        assert!(shared("/mnt/my dir/pistack"), "an escaped space parses");
        assert!(shared("/srv/nested/deep"), "a nested mount counts");
        assert!(shared("/srv/sub/deeper/x"));
        assert!(!shared("/srv/sub"), "a mount further down is not above us");
    }

    #[test]
    fn the_root_of_a_network_filesystem_holds_anything_below_it() {
        let mounts = b"server:/ /mnt/all nfs rw 0 0\n";
        assert!(mounts_shared(mounts, Path::new("/mnt/all/pistack")));
        assert!(!mounts_shared(b"", Path::new("/mnt/all/pistack")));
    }
}

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

pub fn node_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_owned())
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
        st.f_frsize as u64
    } else {
        st.f_bsize as u64
    };
    Some((st.f_bavail as u64).saturating_mul(frsize) / (1024 * 1024 * 1024))
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
}

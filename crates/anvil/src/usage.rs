// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What the host and its machines are using, for the heartbeat.
//!
//! The host figures are read from the kernel. Machine figures come from
//! the runtime that holds the machine: libvirt answers for every guest in
//! one `virsh domstats` call, which [`parse_domstats`] reads.

use pistack_rpc::proto::{DiskUsage, HostUsage, MachineUsage, NicUsage};
use pistack_types::{kind, Id};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::Path;

/// Libvirt domains PiStack made are named with this in front of the id.
const DOMAIN_PREFIX: &str = "pistack-";
/// The cloud-init seed the agent attaches to each guest. It is read once
/// at boot and its counters say nothing.
const SEED_FILE: &str = "seed.iso";

/// Load, memory and the data directory's filesystem. A figure that cannot
/// be read is left out; nothing here fails.
pub fn host_usage(data_dir: &Path) -> HostUsage {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let (disk_total, disk_free) = match disk_bytes(data_dir).or_else(|| disk_bytes(Path::new("/")))
    {
        Some((total, free)) => (Some(total), Some(free)),
        None => (None, None),
    };
    HostUsage {
        load_milli: load_milli(),
        memory_total_bytes: meminfo_bytes(&meminfo, "MemTotal:"),
        memory_available_bytes: meminfo_bytes(&meminfo, "MemAvailable:"),
        data_disk_total_bytes: disk_total,
        data_disk_free_bytes: disk_free,
    }
}

fn load_milli() -> [u32; 3] {
    let mut avg = [0f64; 3];
    // SAFETY: `getloadavg` writes at most three doubles into the array.
    let n = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
    let mut out = [0u32; 3];
    for i in 0..usize::try_from(n).unwrap_or(0).min(3) {
        out[i] = (avg[i].max(0.0) * 1000.0).round() as u32;
    }
    out
}

/// A `/proc/meminfo` line such as `MemAvailable:   123456 kB`, in bytes.
fn meminfo_bytes(text: &str, field: &str) -> Option<u64> {
    text.lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

/// Size and free space, in bytes, of the filesystem that holds `path`.
fn disk_bytes(path: &Path) -> Option<(u64, u64)> {
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
    // The field types differ by OS; all of them fit in an i128.
    let wide = |n: i128| u64::try_from(n).unwrap_or(0);
    let block = if st.f_frsize > 0 {
        wide(st.f_frsize.into())
    } else {
        wide(st.f_bsize.into())
    };
    Some((
        wide(st.f_blocks.into()).saturating_mul(block),
        wide(st.f_bavail.into()).saturating_mul(block),
    ))
}

/// Read the output of
/// `virsh domstats --cpu-total --balloon --block --interface`.
///
/// Each domain is a `Domain: 'name'` line followed by `key=value` lines.
/// Domains PiStack did not make are skipped, and so is anything this does
/// not know: libvirt adds keys from release to release. A figure that is
/// missing, as most are for a guest that is shut off, reads as zero.
pub fn parse_domstats(text: &str) -> Vec<MachineUsage> {
    let mut out = Vec::new();
    let mut current: Option<(Id, BTreeMap<&str, &str>)> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("Domain:") {
            out.extend(current.take().map(|(id, stats)| machine_usage(id, &stats)));
            let name = name.trim().trim_matches('\'');
            current = name
                .strip_prefix(DOMAIN_PREFIX)
                .and_then(|id| Id::parse(kind::MACHINE, id))
                .map(|id| (id, BTreeMap::new()));
        } else if let (Some((_, stats)), Some((key, value))) =
            (current.as_mut(), line.split_once('='))
        {
            stats.insert(key, value);
        }
    }
    out.extend(current.map(|(id, stats)| machine_usage(id, &stats)));
    out
}

fn machine_usage(id: Id, stats: &BTreeMap<&str, &str>) -> MachineUsage {
    let number = |key: &str| stats.get(key).and_then(|v| v.parse::<u64>().ok());
    let kib = |key: &str| number(key).map(|v| v.saturating_mul(1024));
    let text = |key: &str| stats.get(key).copied().unwrap_or_default();

    let mut disks = Vec::new();
    for i in 0..number("block.count").unwrap_or(0) {
        let stat = |name: &str| number(&format!("block.{i}.{name}"));
        let device = text(&format!("block.{i}.name"));
        let seed = Path::new(text(&format!("block.{i}.path")))
            .file_name()
            .is_some_and(|f| f == SEED_FILE);
        // A guest that is shut off lists its disks with no counters.
        if device.is_empty() || seed || stat("rd.bytes").is_none() {
            continue;
        }
        // With `--backing` a device is listed once per image in its chain;
        // the first entry is the device itself.
        if disks.iter().any(|d: &DiskUsage| d.device == device) {
            continue;
        }
        disks.push(DiskUsage {
            device: device.to_owned(),
            read_bytes: stat("rd.bytes").unwrap_or(0),
            write_bytes: stat("wr.bytes").unwrap_or(0),
            read_ops: stat("rd.reqs").unwrap_or(0),
            write_ops: stat("wr.reqs").unwrap_or(0),
        });
    }

    let mut nics = Vec::new();
    for i in 0..number("net.count").unwrap_or(0) {
        let name = text(&format!("net.{i}.name"));
        if name.is_empty() {
            continue;
        }
        nics.push(NicUsage {
            name: name.to_owned(),
            rx_bytes: number(&format!("net.{i}.rx.bytes")).unwrap_or(0),
            tx_bytes: number(&format!("net.{i}.tx.bytes")).unwrap_or(0),
        });
    }

    // `available` is the memory the guest sees and `usable` what it could
    // still hand out; both need a balloon driver that reports.
    let guest_used = match (kib("balloon.available"), kib("balloon.usable")) {
        (Some(available), Some(usable)) => Some(available.saturating_sub(usable)),
        _ => None,
    };
    MachineUsage {
        id,
        cpu_ns: number("cpu.time").unwrap_or(0),
        memory_bytes: kib("balloon.rss").unwrap_or(0),
        memory_assigned_bytes: kib("balloon.current").unwrap_or(0),
        memory_guest_used_bytes: guest_used,
        disks,
        nics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `virsh domstats --cpu-total --balloon --block --interface` on a host
    /// with libvirt 12.0.0: one guest PiStack did not make, shut off, and
    /// two running ones it did. The foreign guest's name was changed.
    const RECORDED: &str = include_str!("../testdata/virsh-domstats.txt");

    #[test]
    fn recorded_domstats_are_read_per_machine() {
        let usage = parse_domstats(RECORDED);
        // The guest PiStack did not make is not reported.
        assert_eq!(usage.len(), 2);

        let a = &usage[0];
        assert_eq!(
            a.id.to_string(),
            "mach-01a102de-3f33-73bf-86f7-57fe3b47f1ea"
        );
        assert_eq!(a.cpu_ns, 141_682_599_991_000);
        assert_eq!(a.memory_bytes, 1_047_776 * 1024);
        assert_eq!(a.memory_assigned_bytes, 1_048_576 * 1024);
        assert_eq!(a.memory_guest_used_bytes, Some((975_644 - 793_088) * 1024));
        // The root disk is reported and the cloud-init seed is not.
        assert_eq!(
            a.disks,
            vec![DiskUsage {
                device: "vda".into(),
                read_bytes: 5_753_219_584,
                write_bytes: 25_119_752_192,
                read_ops: 163_388,
                write_ops: 1_067_863,
            }]
        );
        assert_eq!(
            a.nics,
            vec![NicUsage {
                name: "vnet2".into(),
                rx_bytes: 758_636_721,
                tx_bytes: 5_292_811,
            }]
        );

        let b = &usage[1];
        assert_eq!(
            b.id.to_string(),
            "mach-01a10318-adb6-775e-aba8-387f3bca0d18"
        );
        assert_eq!(b.cpu_ns, 141_197_040_147_000);
        assert_eq!(b.memory_bytes, 751_460 * 1024);
        assert_eq!(b.disks.len(), 1);
        assert_eq!(b.disks[0].read_ops, 15_440);
        assert_eq!(b.nics[0].name, "vnet3");
        assert_eq!(b.nics[0].rx_bytes, 91_143_749);
    }

    #[test]
    fn a_guest_that_is_shut_off_reads_as_zeros_and_junk_is_skipped() {
        let text = "\
Domain: 'pistack-mach-01a102de-3f33-73bf-86f7-57fe3b47f1ea'
  cpu.time=5
  balloon.current=2048
  balloon.maximum=2048
  block.count=1
  block.0.name=vda
  block.0.path=/var/lib/pistack/machines/x/disk.qcow2
  block.0.capacity=10
  some.new.key=what
  not a pair

Domain: 'pistack-not-an-id'
  cpu.time=7
";
        let usage = parse_domstats(text);
        assert_eq!(usage.len(), 1);
        let m = &usage[0];
        assert_eq!(m.cpu_ns, 5);
        assert_eq!(m.memory_bytes, 0);
        assert_eq!(m.memory_assigned_bytes, 2048 * 1024);
        assert_eq!(m.memory_guest_used_bytes, None);
        assert!(m.disks.is_empty() && m.nics.is_empty());
        assert!(parse_domstats("").is_empty());
        assert!(parse_domstats("error: failed to connect").is_empty());
    }

    #[test]
    fn a_usage_report_for_one_machine_is_small_on_the_wire() {
        let usage = parse_domstats(RECORDED);
        let bytes = serde_json::to_vec(&usage[0]).unwrap().len();
        assert!((200..400).contains(&bytes), "{bytes} bytes");
    }

    #[test]
    fn meminfo_fields_are_read_in_bytes() {
        let text = "MemTotal:       16318296 kB\nMemFree:  1 kB\nMemAvailable:    8000000 kB\n";
        assert_eq!(meminfo_bytes(text, "MemTotal:"), Some(16_318_296 * 1024));
        assert_eq!(meminfo_bytes(text, "MemAvailable:"), Some(8_000_000 * 1024));
        assert_eq!(meminfo_bytes("garbage", "MemTotal:"), None);
    }

    #[test]
    fn the_host_answers_for_load_and_its_disk() {
        let usage = host_usage(Path::new("/definitely/not/here"));
        // Falls back to the root filesystem.
        let total = usage.data_disk_total_bytes.expect("a filesystem size");
        assert!(total > 0);
        assert!(usage.data_disk_free_bytes.unwrap() <= total);
        if let (Some(total), Some(available)) =
            (usage.memory_total_bytes, usage.memory_available_bytes)
        {
            assert!(available <= total);
        }
    }
}

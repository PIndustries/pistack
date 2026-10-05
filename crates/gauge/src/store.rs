// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! On-disk sample ring under `$PISTACK_DATA_DIR/gauge`.
//!
//! The process-wide ring is still an in-memory `Vec`, capped at
//! [`crate::MAX_SAMPLES`]. When a data directory is attached the ring is
//! loaded from `samples.json` on attach and rewritten after every push so a
//! restart keeps the same points.

use crate::types::{Sample, MAX_SAMPLES};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// File name under the gauge data directory.
pub const SAMPLES_FILE: &str = "samples.json";

/// In-memory ring with an optional on-disk snapshot.
#[derive(Debug, Default)]
pub struct SampleRing {
    buf: Mutex<Vec<Sample>>,
    path: Mutex<Option<PathBuf>>,
}

impl SampleRing {
    /// Empty ring that never touches the disk.
    pub fn memory() -> Self {
        Self {
            buf: Mutex::new(Vec::new()),
            path: Mutex::new(None),
        }
    }

    /// Attach `dir` as the gauge data directory and load `samples.json` when
    /// present. Idempotent: a second attach to the same path is a no-op; a
    /// different path replaces the snapshot location and reloads.
    pub fn attach(&self, dir: impl Into<PathBuf>) -> std::io::Result<()> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let file = dir.join(SAMPLES_FILE);
        {
            let mut path = self.path.lock().unwrap_or_else(|p| p.into_inner());
            if path.as_ref() == Some(&file) {
                return Ok(());
            }
            *path = Some(file.clone());
        }
        let loaded = load_samples(&file)?;
        {
            let mut buf = self.lock();
            *buf = loaded;
        }
        Ok(())
    }

    /// Snapshot path when persistence is attached.
    pub fn path(&self) -> Option<PathBuf> {
        self.path.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Vec<Sample>> {
        self.buf.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Append one sample, drop the oldest when over the cap, and rewrite the
    /// snapshot when a path is attached.
    pub fn push(&self, sample: Sample) {
        {
            let mut buf = self.lock();
            push_sample(&mut buf, sample);
        }
        self.persist();
    }

    /// Samples in arrival order. `metric` filters when set.
    pub fn list(&self, metric: Option<&str>) -> Vec<Sample> {
        let buf = self.lock();
        buf.iter()
            .filter(|s| metric.is_none_or(|m| s.metric == m))
            .cloned()
            .collect()
    }

    /// Latest sample for `metric`, if any.
    pub fn latest(&self, metric: &str) -> Option<Sample> {
        self.lock()
            .iter()
            .rev()
            .find(|s| s.metric == metric)
            .cloned()
    }

    /// Rewrite the snapshot. No-op when no path is attached or the write fails
    /// (failures are logged; the in-memory ring stays authoritative).
    pub fn persist(&self) {
        let path = match self.path() {
            Some(p) => p,
            None => return,
        };
        let snapshot: Vec<Sample> = self.lock().clone();
        if let Err(e) = write_samples(&path, &snapshot) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "gauge: failed to persist samples"
            );
        }
    }
}

/// `$PISTACK_DATA_DIR/gauge`, or `./data/gauge` when the variable is unset.
pub fn data_root() -> PathBuf {
    std::env::var_os("PISTACK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./data"))
        .join("gauge")
}

pub(crate) fn push_sample(buf: &mut Vec<Sample>, sample: Sample) {
    if buf.len() >= MAX_SAMPLES {
        let drop_n = buf.len() + 1 - MAX_SAMPLES;
        buf.drain(..drop_n);
    }
    buf.push(sample);
}

fn load_samples(path: &Path) -> std::io::Result<Vec<Sample>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut samples: Vec<Sample> = serde_json::from_slice(&bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("gauge samples: {e}"),
        )
    })?;
    if samples.len() > MAX_SAMPLES {
        let drop_n = samples.len() - MAX_SAMPLES;
        samples.drain(..drop_n);
    }
    Ok(samples)
}

fn write_samples(path: &Path, samples: &[Sample]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(samples).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("gauge samples: {e}"),
        )
    })?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample(metric: &str, value: f64) -> Sample {
        Sample {
            metric: metric.into(),
            value,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn persistence_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "gauge-samples-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let ring = SampleRing::memory();
        ring.attach(&dir).unwrap();
        ring.push(sample("cpu.usage", 10.0));
        ring.push(sample("cpu.usage", 80.0));
        assert_eq!(ring.list(Some("cpu.usage")).len(), 2);
        assert!(dir.join(SAMPLES_FILE).is_file());

        let again = SampleRing::memory();
        again.attach(&dir).unwrap();
        let loaded = again.list(Some("cpu.usage"));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].value, 10.0);
        assert_eq!(loaded[1].value, 80.0);
        assert_eq!(again.latest("cpu.usage").unwrap().value, 80.0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_ring_skips_disk() {
        let ring = SampleRing::memory();
        ring.push(sample("x", 1.0));
        assert!(ring.path().is_none());
        assert_eq!(ring.list(None).len(), 1);
    }
}

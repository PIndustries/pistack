// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Rootfs directory. Paths are resolved against the canonical root so a
/// symlink cannot point an extract at a host path outside it.
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::join_logical;
use crate::runtime::youki::image::loosen_dir;
use crate::runtime::youki::image::make_writable_dir;
use crate::runtime::youki::image::normal_parts;
use crate::runtime::youki::image::remove_path;
use pistack_types::ApiResult;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) struct Anchor {
    pub(in crate::runtime::youki) root: PathBuf,
}

impl Anchor {
    pub(in crate::runtime::youki) fn open(dest: &Path) -> ApiResult<Self> {
        match std::fs::symlink_metadata(dest) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(cannot_unpack());
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dest).map_err(|_| cannot_unpack())?;
            }
            Err(_) => return Err(cannot_unpack()),
        }
        let root = dest.canonicalize().map_err(|_| cannot_unpack())?;
        Ok(Self { root })
    }

    pub(in crate::runtime::youki) fn abs(&self, logical: &Path) -> PathBuf {
        self.root.join(logical)
    }

    /// Parent relative to the root, plus the final component.
    /// `create` makes missing parent directories writable (mode `0o755`).
    pub(in crate::runtime::youki) fn split(
        &self,
        rel: &Path,
        create: bool,
    ) -> ApiResult<Option<(PathBuf, OsString)>> {
        let mut parts = normal_parts(rel);
        let Some(name) = parts.pop() else {
            return Err(cannot_unpack());
        };
        let Some(logical) = self.walk(&parts, create)? else {
            return Ok(None);
        };
        Ok(Some((logical, name)))
    }

    pub(in crate::runtime::youki) fn logical_full(&self, rel: &Path) -> ApiResult<PathBuf> {
        let (logical, name) = self.split(rel, false)?.ok_or_else(cannot_unpack)?;
        Ok(logical.join(name))
    }

    pub(in crate::runtime::youki) fn existing(&self, rel: &Path) -> ApiResult<Option<PathBuf>> {
        let Some((logical, name)) = self.split(rel, false)? else {
            return Ok(None);
        };
        let full = self.abs(&logical).join(name);
        match std::fs::symlink_metadata(&full) {
            Ok(_) => Ok(Some(full)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(cannot_unpack()),
        }
    }

    pub(in crate::runtime::youki) fn prepare_file(&self, rel: &Path) -> ApiResult<PathBuf> {
        let (logical, name) = self.split(rel, true)?.ok_or_else(cannot_unpack)?;
        let dest = self.abs(&logical).join(name);
        remove_path(&dest)?;
        Ok(dest)
    }

    pub(in crate::runtime::youki) fn ensure_dir(&self, rel: &Path) -> ApiResult<()> {
        let (logical, name) = self.split(rel, true)?.ok_or_else(cannot_unpack)?;
        let dest = self.abs(&logical).join(name);
        match std::fs::symlink_metadata(&dest) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => loosen_dir(&dest)?,
            Ok(_) => {
                remove_path(&dest)?;
                make_writable_dir(&dest)?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => make_writable_dir(&dest)?,
            Err(_) => return Err(cannot_unpack()),
        }
        Ok(())
    }

    /// Walk `parts`, following symlinks that stay inside the root.
    /// An absolute link, or a relative link that climbs out, is an error.
    pub(in crate::runtime::youki) fn walk(
        &self,
        parts: &[OsString],
        create: bool,
    ) -> ApiResult<Option<PathBuf>> {
        let mut logical = PathBuf::new();
        let mut pending = parts.to_vec();
        let mut index = 0usize;
        let mut hops = 0u32;
        while index < pending.len() {
            let name = pending[index].clone();
            if name.is_empty() || name == "." || name == ".." {
                return Err(cannot_unpack());
            }
            let full = self.abs(&logical).join(&name);
            match std::fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    hops += 1;
                    if hops > 40 {
                        return Err(cannot_unpack());
                    }
                    let target = std::fs::read_link(&full).map_err(|_| cannot_unpack())?;
                    let Some(linked) = join_logical(&logical, &target) else {
                        return Err(cannot_unpack());
                    };
                    let mut next = normal_parts(&linked);
                    next.extend(pending[index + 1..].iter().cloned());
                    pending = next;
                    logical = PathBuf::new();
                    index = 0;
                }
                Ok(meta) if meta.is_dir() => {
                    logical.push(&name);
                    index += 1;
                }
                Ok(_) => return Err(cannot_unpack()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    if !create {
                        return Ok(None);
                    }
                    make_writable_dir(&full)?;
                    logical.push(&name);
                    index += 1;
                }
                Err(_) => return Err(cannot_unpack()),
            }
        }
        Ok(Some(logical))
    }
}

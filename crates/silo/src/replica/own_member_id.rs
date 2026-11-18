// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// This server's member id, from `<data_dir>/node.id`.
pub(crate) fn own_member_id() -> Option<String> {
    let path = crate::Silo::data_root().parent()?.join("node.id");
    let id = std::fs::read_to_string(path).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

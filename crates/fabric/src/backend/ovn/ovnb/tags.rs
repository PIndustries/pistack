// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovn_backend::OvnBackend;
use pistack_types::Id;

impl OvnBackend {
    /// Our `external_ids` for a row standing for `id`.
    pub(in crate::backend::ovn) fn tags(
        &self,
        id: &Id,
        project: Option<&Id>,
    ) -> Vec<(String, String)> {
        let mut t = vec![("pistack:id".to_string(), id.to_string())];
        if let Some(p) = project {
            t.push(("pistack:project".into(), p.to_string()));
        }
        t.push(("pistack:term".into(), self.term().to_string()));
        t
    }
}

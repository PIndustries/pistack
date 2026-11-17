// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What one realize did, by id.
#[derive(Default)]
pub(crate) struct Applied {
    pub(crate) created: Vec<String>,
    pub(crate) updated: Vec<String>,
    pub(crate) present: Vec<String>,
    pub(crate) ignored: Vec<String>,
    /// Managed ids deleted (or marked for async deletion) because the
    /// document no longer names them.
    pub(crate) deleted: Vec<String>,
    /// Managed ids the document no longer names that could not be deleted
    /// (unknown kind). Successfully deleted ids are not listed here.
    pub(crate) left: Vec<String>,
}
impl Applied {
    /// An existing row: `updated` when its spec changed, else `present`.
    pub(crate) fn keep(&mut self, id: String, changed: bool) {
        if changed {
            self.updated.push(id);
        } else {
            self.present.push(id);
        }
    }

    /// Rows written so far, for a failure message.
    pub(crate) fn progress(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if !self.created.is_empty() {
            parts.push(format!("created {}", self.created.join(", ")));
        }
        if !self.updated.is_empty() {
            parts.push(format!("updated {}", self.updated.join(", ")));
        }
        parts
    }

    pub(crate) fn message(&self) -> String {
        let mut parts = self.progress();
        if !self.present.is_empty() {
            parts.push(format!("present {}", self.present.join(", ")));
        }
        if !self.ignored.is_empty() {
            parts.push(format!("ignored {}", self.ignored.join(", ")));
        }
        if !self.deleted.is_empty() {
            parts.push(format!("deleted {}", self.deleted.join(", ")));
        }
        if !self.left.is_empty() {
            parts.push(format!(
                "left {} (no longer in the document; could not delete)",
                self.left.join(", ")
            ));
        }
        if parts.is_empty() {
            "no resources in the document".to_owned()
        } else {
            parts.join("; ")
        }
    }
}

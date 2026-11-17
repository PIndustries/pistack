// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{
    validate_document, validate_kit, validate_spec, validate_workflow, Kit, KitSpec, Stack,
    StackDocument, StackPhase, StackSpec, StackStatus, Workflow, WorkflowPhase, WorkflowSpec,
    WorkflowStatus, APPLYING_MESSAGE, KIT_KIND, WORKFLOW_KIND,
};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Resource, Spec};

use super::applied::Applied;
use super::finish::finish;
use super::managed_ids::managed_ids;
use super::realize::realize;
use super::record_revision::record_revision;

#[derive(Clone)]
pub struct Client {
    store: Store,
}
impl Client {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn stacks(&self) -> Collection<Stack> {
        self.store.collection()
    }

    pub fn kits(&self) -> Collection<Kit> {
        self.store.collection()
    }

    pub fn workflows(&self) -> Collection<Workflow> {
        self.store.collection()
    }

    pub fn create_stack(&self, mut meta: Meta, mut spec: StackSpec) -> ApiResult<Stack> {
        meta.id = Id::new(kind::STACK);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_spec(&mut spec)?;
        self.stacks()
            .create(Stack {
                meta,
                spec,
                status: StackStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_stack(&self, id: &Id) -> ApiResult<Stack> {
        self.stacks().get(id).map_err(Into::into)
    }

    pub fn resolve_stack(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Stack> {
        self.stacks()
            .resolve(project, name_or_id)
            .map_err(Into::into)
    }

    pub fn list_stacks(&self, project: Option<&Id>) -> ApiResult<Vec<Stack>> {
        self.stacks().list(project).map_err(Into::into)
    }

    pub fn delete_stack(&self, id: &Id) -> ApiResult<Stack> {
        self.stacks().delete(id).map_err(Into::into)
    }

    /// Change a stack's metadata or spec. The new spec is validated and
    /// stored; [`Client::apply_stack`] realizes it. Status is kept. With
    /// `expected`, the write fails when the stack's revision differs.
    pub fn update_stack(
        &self,
        id: &Id,
        expected: Option<u64>,
        mut f: impl FnMut(&mut Stack) -> ApiResult<()>,
    ) -> ApiResult<Stack> {
        for _ in 0..8 {
            let cur = self.get_stack(id)?;
            let mut next = cur.clone();
            f(&mut next)?;
            next.meta.id = cur.meta.id.clone();
            next.meta.name = cur.meta.name.clone();
            next.meta.project = cur.meta.project.clone();
            next.status = cur.status.clone();
            if next.spec.name.trim().is_empty() {
                next.spec.name = next.meta.name.clone();
            }
            validate_spec(&mut next.spec)?;
            match self
                .stacks()
                .update(next, Some(expected.unwrap_or(cur.meta.revision)))
            {
                Ok(stack) => return Ok(stack),
                Err(StoreError::Revision { .. }) if expected.is_none() => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(ApiError::conflict(format!(
            "stack {id}: too many concurrent writers"
        )))
    }

    /// Re-check `spec.document` and realize it.
    ///
    /// A YAML stream, or one JSON object, of `network`, `subnet`,
    /// `machine`, `router`, `pod`, `listener`, and DNS record set items is
    /// written into this store. A missing name is created. A name an
    /// earlier apply of this stack created is updated to the document's
    /// spec. Any other existing name is left as it is. Managed ids the
    /// document no longer names are deleted in dependency order (or marked
    /// for deletion when the owning module deletes asynchronously).
    /// Unknown kinds are reported and do not fail the apply.
    ///
    /// The phase is `Applying` while rows are written, then `Ready`. On an
    /// error the phase is `Failed`, the message says why, and the error is
    /// returned. A successful apply of a spec that differs from the newest
    /// revision records a new revision for [`Client::rollback_stack`].
    pub fn apply_stack(&self, id: &Id) -> ApiResult<Stack> {
        self.apply_stack_from(id, None)
    }

    /// Put an earlier applied spec back and apply it.
    ///
    /// `revision` picks one from `status.history`. Without it, the newest
    /// revision whose spec differs from the current spec is used. The
    /// applied result is recorded as a new revision that names the one it
    /// restored, unless it matches the newest revision already.
    pub fn rollback_stack(&self, id: &Id, revision: Option<u64>) -> ApiResult<Stack> {
        let cur = self.get_stack(id)?;
        let target = match revision {
            Some(rev) => cur
                .status
                .history
                .iter()
                .find(|r| r.revision == rev)
                .ok_or_else(|| ApiError::not_found(format!("revision {rev} not found")))?,
            None => cur
                .status
                .history
                .iter()
                .rev()
                .find(|r| r.spec != cur.spec)
                .ok_or_else(|| ApiError::unprocessable("no previous revision to roll back to"))?,
        };
        if target.spec == cur.spec {
            return Err(ApiError::unprocessable(format!(
                "revision {} is already the stack's spec; apply it instead",
                target.revision
            )));
        }
        let restored = target.spec.clone();
        let from = target.revision;
        self.stacks().modify(id, |s| {
            s.spec = restored.clone();
            Ok(())
        })?;
        tracing::debug!(id = %id, revision = from, "stack rolled back");
        self.apply_stack_from(id, Some(from))
    }

    fn apply_stack_from(&self, id: &Id, restored_from: Option<u64>) -> ApiResult<Stack> {
        let applied = self.apply_rows::<StackSpec>(id, |status, spec| {
            record_revision(status, spec, restored_from)
        })?;
        tracing::debug!(
            id = %applied.meta.id,
            revision = applied.status.revision,
            created = applied.status.created.len(),
            updated = applied.status.updated.len(),
            "stack applied"
        );
        Ok(applied)
    }

    /// The apply path shared by stacks and kits: substitute the parameters,
    /// mark `Applying`, realize the document, then store `Ready` or `Failed`.
    /// `on_ready` runs on the final status after a successful realize, with
    /// the spec that was applied. A parameter that is named without being
    /// declared, or declared with neither a value nor a default, is refused
    /// before the first write: the stack keeps the status it had.
    fn apply_rows<S>(
        &self,
        id: &Id,
        mut on_ready: impl FnMut(&mut StackStatus, &S),
    ) -> ApiResult<Resource<S, StackStatus>>
    where
        S: Spec + StackDocument,
    {
        let col = self.store.collection::<Resource<S, StackStatus>>();
        let cur = col.get(id)?;
        validate_document(cur.spec.document())?;
        let document = cur.spec.realized()?;
        let managed = managed_ids(&cur.status);
        let project = cur.meta.project.clone();
        let applied = cur.spec;
        let mut out = Applied::default();
        let result = realize(&self.store, project.as_ref(), &document, &managed, &mut out);
        // One write holds the whole outcome: the applying mark and the result
        // are not two revisions of the same apply.
        let next = col.modify(id, |r| {
            r.status.phase = StackPhase::Applying;
            r.status.message = APPLYING_MESSAGE.to_owned();
            finish(&mut r.status, &managed, &out, result.as_ref().err());
            if result.is_ok() {
                on_ready(&mut r.status, &applied);
            }
            Ok(())
        })?;
        result.map(|()| next)
    }

    pub fn create_kit(&self, mut meta: Meta, mut spec: KitSpec) -> ApiResult<Kit> {
        meta.id = Id::new(KIT_KIND);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_kit(&mut spec)?;
        self.kits()
            .create(Kit {
                meta,
                spec,
                status: StackStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_kit(&self, id: &Id) -> ApiResult<Kit> {
        self.kits().get(id).map_err(Into::into)
    }

    pub fn resolve_kit(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Kit> {
        self.kits().resolve(project, name_or_id).map_err(Into::into)
    }

    pub fn list_kits(&self, project: Option<&Id>) -> ApiResult<Vec<Kit>> {
        self.kits().list(project).map_err(Into::into)
    }

    pub fn delete_kit(&self, id: &Id) -> ApiResult<Kit> {
        self.kits().delete(id).map_err(Into::into)
    }

    /// Apply a stored kit document through [`realize`], substituting its
    /// parameters first. Phases and updates work as for a stack; a kit keeps
    /// no revision history.
    pub fn apply_kit(&self, id: &Id) -> ApiResult<Kit> {
        let applied = self.apply_rows::<KitSpec>(id, |_, _| {})?;
        tracing::debug!(
            id = %applied.meta.id,
            created = applied.status.created.len(),
            updated = applied.status.updated.len(),
            "kit applied"
        );
        Ok(applied)
    }

    pub fn create_workflow(&self, mut meta: Meta, mut spec: WorkflowSpec) -> ApiResult<Workflow> {
        meta.id = Id::new(WORKFLOW_KIND);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_workflow(&mut spec)?;
        self.workflows()
            .create(Workflow {
                meta,
                spec,
                status: WorkflowStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        self.workflows().get(id).map_err(Into::into)
    }

    pub fn resolve_workflow(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Workflow> {
        self.workflows()
            .resolve(project, name_or_id)
            .map_err(Into::into)
    }

    pub fn list_workflows(&self, project: Option<&Id>) -> ApiResult<Vec<Workflow>> {
        self.workflows().list(project).map_err(Into::into)
    }

    pub fn delete_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        self.workflows().delete(id).map_err(Into::into)
    }

    /// Apply each named stack in order. The first error is stored on the
    /// workflow and returned. Stacks after that name are not applied.
    pub fn apply_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        let cur = self.get_workflow(id)?;
        let project = cur.meta.project.clone();
        let names = cur.spec.stacks.clone();
        let mut applied = Vec::new();
        for name in &names {
            let stack = match self.resolve_stack(project.as_ref(), name) {
                Ok(stack) => stack,
                Err(e) => {
                    return self.stop_workflow(id, &applied, name, e);
                }
            };
            if let Err(e) = self.apply_stack(&stack.meta.id) {
                return self.stop_workflow(id, &applied, name, e);
            }
            applied.push(name.clone());
        }
        let message = if applied.is_empty() {
            "accepted; workflow has no stacks".to_owned()
        } else {
            format!("applied {}", applied.join(", "))
        };
        self.write_workflow(id, WorkflowPhase::Accepted, message, applied)
    }

    fn stop_workflow(
        &self,
        id: &Id,
        applied: &[String],
        name: &str,
        err: ApiError,
    ) -> ApiResult<Workflow> {
        let message = format!("stopped on stack {name}: {}", err.message);
        self.write_workflow(id, WorkflowPhase::Failed, message.clone(), applied.to_vec())?;
        Err(ApiError::new(err.code, message))
    }

    fn write_workflow(
        &self,
        id: &Id,
        phase: WorkflowPhase,
        message: String,
        applied: Vec<String>,
    ) -> ApiResult<Workflow> {
        let mut next = self.get_workflow(id)?;
        next.status = WorkflowStatus {
            phase,
            message,
            applied,
        };
        self.workflows().update(next, None).map_err(Into::into)
    }
}

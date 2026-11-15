// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A stack is a PiStack resource: a document the user wrote (YAML or JSON
//! text) plus declared parameters and the values given for them.
//!
//! A parameter is declared in `spec.declared` as a `name`, an optional
//! `default`, and an optional `description`. The document names one by
//! writing `{{ param.NAME }}` where a value belongs, quoted — bare `{{` is
//! the start of a mapping to YAML. Apply and update substitute it: the value
//! given in `spec.parameters` if there is one, else the declared default.
//! A reference stands for a whole value, so a number stays a number and a
//! flag a flag; set inside other text it is replaced within that text.
//!
//! An apply is refused before any row is written when a value was given for
//! a parameter nothing declares, when the document names a parameter that is
//! not declared, or when a declared parameter has neither a value nor a
//! default. Each message names the parameter.
//!
//! Apply creates network, subnet, machine, router, pod, listener, and record
//! set rows from the document, updates the rows an earlier apply of the same
//! stack created, and deletes managed rows the document no longer names. Each
//! successful stack apply records the spec, with the values it used, as a
//! revision; rollback puts an earlier revision back and applies it. A kit is a
//! stored document applied the same way. A workflow applies named stacks in
//! order and stops on the first error. Apply does not run a reconciler.

use chrono::{DateTime, Utc};
use pistack_types::{kind, ApiError, ApiResult, Resource, Spec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Status text stored when a stack or kit is created. Apply replaces it
/// with the ids it inserted, updated, or found already present.
pub const ACCEPTED_MESSAGE: &str =
    "accepted; apply creates network, subnet, machine, router, pod, listener, and record set rows";

/// Status text stored while an apply is realizing the document.
pub const APPLYING_MESSAGE: &str = "applying the document";

/// Applied stack specs kept for rollback. Older revisions are dropped.
pub const REVISION_HISTORY_LIMIT: usize = 10;

/// Status text stored when a workflow is created. Apply replaces it with
/// the stack names it ran.
pub const WORKFLOW_ACCEPTED_MESSAGE: &str =
    "accepted; apply runs each named stack in order and stops on the first error";

/// Id prefix and store table for a kit.
pub const KIT_KIND: &str = "kit";
/// Id prefix and store table for a workflow.
pub const WORKFLOW_KIND: &str = "workflow";

/// The name space a parameter reference may name.
const PARAM_PREFIX: &str = "param.";
/// Start of a parameter reference.
const REF_MARK: &str = "{{";
/// End of a parameter reference.
const REF_END: &str = "}}";

/// A parameter name: 1-64 characters of letters, digits, `.` or `-`, starting
/// with a letter.
pub fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 64
        && first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Does the reference spelling `said` name the declared parameter `param`?
/// A declared parameter is stored by its short name; the `param.` a reference
/// carries is the name space, not part of the name.
fn names(said: &str, param: &str) -> bool {
    said == param || said.strip_prefix(PARAM_PREFIX) == Some(param)
}

/// The `{{` in `text` that a scan stops on: the first one that is not a
/// reference, or the first that names a parameter.
#[derive(Debug, PartialEq, Eq)]
enum Slot<'a> {
    /// A `{{` holding a parameter name, and the whole `{{ ... }}`.
    Named(&'a str, &'a str),
    /// A `{{ ... }}` that is not a reference: no `}}`, a name space that is
    /// not `param.`, or a name that is not one.
    Foreign(&'a str),
}

/// The next `{{` in `text` and where it ends. `None` when `text` holds none.
/// A `{{` is read to its first `}}` with no `{{` before it, so a nested
/// brace makes the whole run foreign rather than ending the reference early.
fn slot(text: &str) -> Option<(Slot<'_>, usize)> {
    let at = text.find(REF_MARK)?;
    let rest = &text[at + REF_MARK.len()..];
    let mut depth = 1usize;
    let mut scanned = 0usize;
    let mut inner = None;
    while scanned < rest.len() {
        if rest[scanned..].starts_with(REF_END) {
            depth -= 1;
            if depth == 0 {
                inner = Some(&rest[..scanned]);
                break;
            }
            scanned += REF_END.len();
            continue;
        }
        if rest[scanned..].starts_with(REF_MARK) {
            depth += 1;
            scanned += REF_MARK.len();
            continue;
        }
        scanned += 1;
    }
    let inner = inner?;
    let len = REF_MARK.len() + scanned + REF_END.len();
    let whole = &text[at..at + len];
    match named(inner) {
        Some(param) => Some((Slot::Named(param, whole), at + len)),
        None => Some((Slot::Foreign(whole), at + len)),
    }
}

/// The parameter named by the contents of one `{{ ... }}`, or `None` when
/// they are not a reference.
fn named(inner: &str) -> Option<&str> {
    let param = inner.trim().strip_prefix(PARAM_PREFIX)?;
    is_param_name(param).then_some(param)
}

impl Slot<'_> {
    /// Does this `{{` hold a parameter name?
    fn is_named(&self) -> bool {
        matches!(self, Slot::Named(_, _))
    }
}

/// Every `{{ ... }}` a string holds, in the order they appear.
fn slots(text: &str) -> Vec<Slot<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((found, len)) = slot(rest) {
        out.push(found);
        rest = &rest[len..];
    }
    out
}

/// The value given for `param`, whichever spelling of the name it was stored
/// under.
fn given(values: &BTreeMap<String, String>, param: &str) -> Option<String> {
    values
        .get(param)
        .or_else(|| values.get(&format!("{PARAM_PREFIX}{param}")))
        .cloned()
}

/// A parameter a document declares. `spec.parameters` holds the values given
/// for these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterDecl {
    /// Name, without the `param.` a reference carries.
    pub name: String,
    /// Value used when no value was given. Without one, a value is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// What the parameter is for, for whoever reads the stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ParameterDecl {
    /// Trim the name to its short spelling and refuse a name no reference can
    /// carry.
    fn normalized(&mut self) -> ApiResult<()> {
        self.name = self.name.trim().trim_start_matches(PARAM_PREFIX).to_owned();
        if !is_param_name(&self.name) {
            return Err(ApiError::bad_request(format!(
                "parameter name '{}' must be 1-64 characters of letters, digits, '.' or '-', starting with a letter",
                self.name
            )));
        }
        Ok(())
    }

    /// The value a reference to this parameter takes when none was given.
    fn resolved_default(&self) -> Option<String> {
        self.default.clone()
    }
}

/// The document with every `{{ param.NAME }}` replaced by the value `pick`
/// gives the name it holds, or `None` when the document holds no reference, so
/// a document with none is realized exactly as it is stored.
///
/// A reference has to be quoted — `mtu: "{{ param.mtu }}"` — because bare `{{`
/// is the start of a mapping to YAML. Substitution happens in the parsed
/// document rather than the text, so the quotes are not carried into the
/// value: a reference that stands alone becomes the node its value spells, so
/// `mtu: "{{ param.mtu }}"` with `1500` is the number and `dhcp: "{{ param.on
/// }}"` with `true` is the flag. A reference set inside other text is replaced
/// within it, and several in one string are joined into that text.
///
/// A `{{ ... }}` that names no parameter — another name space, a name that is
/// not one, a `{{` with no `}}` — is left exactly as it is written, so a
/// document from another tool keeps its own braces.
pub fn substitute(
    document: &str,
    declared: &[ParameterDecl],
    values: &BTreeMap<String, String>,
) -> ApiResult<Option<String>> {
    if declared_uses(document).is_empty() {
        return Ok(None);
    }
    let pick = |param: &str| match given(values, param) {
        Some(value) => Some(value),
        None => declared
            .iter()
            .find(|decl| names(&decl.name, param))
            .and_then(ParameterDecl::resolved_default),
    };
    let mut out = String::new();
    for doc in parse_all(document)? {
        let filled = fill(doc, &pick)?;
        out.push_str(&write_doc(&filled)?);
    }
    Ok(Some(out))
}

/// The document as a stream of values, read the way apply reads it. A
/// reference that YAML read as a nested mapping — the same braces left
/// unquoted — is put back to the text it was written as.
fn parse_all(document: &str) -> ApiResult<Vec<Value>> {
    let mut docs = Vec::new();
    for doc in serde_yaml_ng::Deserializer::from_str(document) {
        let value = serde_json::Value::deserialize(doc)
            .map_err(|e| ApiError::bad_request(format!("document is not valid YAML: {e}")))?;
        docs.push(unstring(value));
    }
    Ok(docs)
}

/// A mapping that is one reference YAML split apart, `"{{"` mapped to
/// `"param.x }}"`, is the reference text again. Anything else is returned as
/// it is.
fn unstring(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(text) = as_reference(&map) {
                return Value::String(text);
            }
            Value::Object(
                map.into_iter()
                    .map(|(key, val)| (key, unstring(val)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(unstring).collect()),
        other => other,
    }
}

/// The whole `{{ ... }}` a mapping spells, when that is what it is: one pair
/// whose key is `{{` and whose value ends in `}}`.
fn as_reference(map: &serde_json::Map<String, Value>) -> Option<String> {
    let mut keys = map.iter();
    let (mark, inner) = keys.next()?;
    if mark != REF_MARK || keys.next().is_some() {
        return None;
    }
    let said = match inner {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).ok()?.trim().to_owned(),
    };
    let said = said.strip_suffix(REF_END)?.trim_end();
    named(&format!("{said}{REF_END}")).map(|_| format!("{REF_MARK} {said}{REF_END}"))
}

/// One parsed document written back as text.
fn write_doc(value: &Value) -> ApiResult<String> {
    serde_yaml_ng::to_string(value)
        .map_err(|e| ApiError::unprocessable(format!("the substituted text: {e}")))
}

/// The parsed document with each reference resolved. A document's own `kind`
/// is read before any value is looked at, so a reference in one is refused
/// there; a `name` may be built from parameters.
fn fill(value: Value, pick: &impl Fn(&str) -> Option<String>) -> ApiResult<Value> {
    Ok(match value {
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("kind") {
                check_head("kind", text)?;
            }
            Value::Object(
                map.into_iter()
                    .map(|(key, val)| Ok((check_key(key)?, fill(val, pick)?)))
                    .collect::<ApiResult<serde_json::Map<String, Value>>>()?,
            )
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| fill(item, pick))
                .collect::<ApiResult<Vec<Value>>>()?,
        ),
        Value::String(text) => one(&text, pick)?,
        other => other,
    })
}

/// A key is never a value, so a reference in one is refused where it sits.
fn check_key(key: String) -> ApiResult<String> {
    if slots(&key).into_iter().any(|found| found.is_named()) {
        return Err(not_a_value(&format!("'{key}'")));
    }
    Ok(key)
}

/// A document's `kind` names the module that will take the row, and is read
/// before the document is walked, so a reference there is refused where it
/// sits.
fn check_head(at: &str, text: &str) -> ApiResult<()> {
    if slots(text).into_iter().any(|found| found.is_named()) {
        return Err(not_a_value(&format!("a reference in {at}")));
    }
    Ok(())
}

fn not_a_value(what: &str) -> ApiError {
    ApiError::bad_request(format!(
        "{what} cannot stand for a parameter: a parameter goes where a value belongs"
    ))
}

/// The value a string holds once its references are resolved. A lone reference
/// standing for a whole value becomes the node that value spells, so a number
/// stays a number and a flag a flag. Anything else — one set in other text, or
/// several in one string — is replaced as text. A `{{ ... }}` that names
/// nothing is left as it is written.
fn one(text: &str, pick: &impl Fn(&str) -> Option<String>) -> ApiResult<Value> {
    let found = slots(text);
    if found.is_empty() {
        return Ok(Value::String(text.to_owned()));
    }
    if let [Slot::Named(param, _)] = &found[..] {
        if slot_is_whole(text) {
            return Ok(node(&pick(param).unwrap_or_default()));
        }
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some((found, len)) = slot(rest) {
        let at = rest.find(REF_MARK).unwrap_or(0);
        out.push_str(&rest[..at]);
        match found {
            Slot::Named(param, _) => out.push_str(pick(param).as_deref().unwrap_or_default()),
            Slot::Foreign(whole) => out.push_str(whole),
        }
        rest = &rest[len..];
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

/// Does the whole string consist of one reference?
fn slot_is_whole(text: &str) -> bool {
    text.trim() == text
        && slot(text).is_some_and(|(_, len)| {
            let at = text.find(REF_MARK).unwrap_or(0);
            len == text.len() && at == 0
        })
}

/// `value` as the node it spells: a flag, a number, or text.
fn node(value: &str) -> Value {
    let text = value.trim();
    if text.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(n) = text.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = text.parse::<u64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = text.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    Value::String(value.to_owned())
}

/// Every `{{ ... }}` a document holds, read straight off its text: the
/// parameter a reference names, or the whole spelling of one that names
/// nothing.
fn names_in(document: &str) -> Vec<Result<String, &str>> {
    let mut out = Vec::new();
    for found in slots(document) {
        match found {
            Slot::Named(param, _) => out.push(Ok(param.to_owned())),
            Slot::Foreign(whole) => out.push(Err(whole)),
        }
    }
    out
}

/// Every parameter the document names, in the order they appear. A `{{ ... }}`
/// naming nothing is not a reference and names no parameter.
pub fn declared_uses(document: &str) -> Vec<String> {
    names_in(document)
        .into_iter()
        .filter_map(|found| found.ok())
        .collect()
}

/// A stack spec: the document, the parameters it declares, and the values
/// given for them.
pub trait StackDocument {
    /// The user's stack text, YAML or JSON. Stored as given.
    fn document(&self) -> &str;
    /// The parameters the document may name.
    fn declared(&self) -> &[ParameterDecl];
    /// Values given for the declared parameters.
    fn values(&self) -> &BTreeMap<String, String>;
    fn declared_mut(&mut self) -> &mut Vec<ParameterDecl>;

    /// Trim every declared name, refuse a name declared twice, refuse a value
    /// no parameter was declared for, and refuse a declared parameter the
    /// document never names.
    fn normalize(&mut self) -> ApiResult<()> {
        for decl in self.declared_mut() {
            decl.normalized()?;
        }
        let decls = self.declared();
        if let Some(dup) = (1..decls.len()).find(|i| decls[*i].name == decls[i - 1].name) {
            return Err(ApiError::bad_request(format!(
                "spec.declared declares '{}' twice",
                decls[dup].name
            )));
        }
        let named = declared_uses(self.document());
        let mut unknown = Vec::new();
        for given in self.values().keys() {
            let asked = given.trim().trim_start_matches(PARAM_PREFIX);
            if !self.declared().iter().any(|decl| names(&decl.name, asked)) {
                push(&mut unknown, asked.to_owned());
            }
        }
        if !unknown.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no parameter is declared for '{}': declare it or drop the value",
                unknown.join("', '")
            )));
        }
        let mut unused = Vec::new();
        for decl in self.declared() {
            if !named.iter().any(|name| names(&decl.name, name)) {
                push(&mut unused, decl.name.clone());
            }
        }
        if !unused.is_empty() {
            return Err(ApiError::bad_request(format!(
                "the document never names declared parameter '{}'",
                unused.join("', '")
            )));
        }
        Ok(())
    }

    /// The value a reference to `param` resolves to: the one given, else the
    /// declared default, else nothing.
    fn value_of(&self, param: &str) -> Option<String> {
        given(self.values(), param).or_else(|| {
            self.declared()
                .iter()
                .find(|decl| names(&decl.name, param))
                .and_then(ParameterDecl::resolved_default)
        })
    }

    /// Refuse a reference to a parameter nothing declares, and a reference to
    /// a declared parameter with neither a value nor a default. Each message
    /// names what is wrong.
    fn check_refs(&self) -> ApiResult<()> {
        let mut missing = Vec::new();
        let mut undeclared = Vec::new();
        for name in declared_uses(self.document()) {
            if !self.declared().iter().any(|decl| names(&decl.name, &name)) {
                push(&mut undeclared, name);
            } else if self.value_of(&name).is_none() {
                push(&mut missing, name);
            }
        }
        if !missing.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no value given for parameter '{}' and it has no default",
                missing.join("', '")
            )));
        }
        if !undeclared.is_empty() {
            return Err(ApiError::bad_request(format!(
                "the document names undeclared parameter '{}'",
                undeclared.join("', '")
            )));
        }
        Ok(())
    }

    /// The document with every reference replaced by the value it resolves
    /// to. A name nothing declares and a value that is required and missing
    /// are refused here, so nothing half-substituted reaches a row. A
    /// document with no reference is returned as it is stored.
    fn realized(&self) -> ApiResult<String> {
        self.check_refs()?;
        Ok(substitute(self.document(), self.declared(), self.values())?
            .unwrap_or_else(|| self.document().to_owned()))
    }
}

fn push(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

impl StackDocument for StackSpec {
    fn document(&self) -> &str {
        &self.document
    }
    fn declared(&self) -> &[ParameterDecl] {
        &self.declared
    }
    fn values(&self) -> &BTreeMap<String, String> {
        &self.parameters
    }
    fn declared_mut(&mut self) -> &mut Vec<ParameterDecl> {
        &mut self.declared
    }
}

impl StackDocument for KitSpec {
    fn document(&self) -> &str {
        &self.document
    }
    fn declared(&self) -> &[ParameterDecl] {
        &self.declared
    }
    fn values(&self) -> &BTreeMap<String, String> {
        &self.parameters
    }
    fn declared_mut(&mut self) -> &mut Vec<ParameterDecl> {
        &mut self.declared
    }
}

/// What the user asked to apply.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackSpec {
    /// Name inside the document, independent of the resource name.
    pub name: String,
    /// The user's stack text, YAML or JSON. Stored as given, references and
    /// all; a value written `{{ param.NAME }}` is substituted at apply.
    pub document: String,
    /// Parameters the document may name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<ParameterDecl>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

impl Spec for StackSpec {
    const KIND: &'static str = kind::STACK;
}

/// Observed phase. `Accepted` until the first apply. Apply moves to
/// `Applying` while it writes rows, then to `Ready` or `Failed`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StackPhase {
    #[default]
    Accepted,
    Applying,
    Ready,
    Failed,
}

/// One applied spec. Rollback re-applies `spec` as a new revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackRevision {
    pub revision: u64,
    pub spec: StackSpec,
    pub applied_at: DateTime<Utc>,
    /// Set when a rollback produced this revision: the revision it restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackStatus {
    pub phase: StackPhase,
    pub message: String,
    /// Ids inserted by the last apply. Already-present names are not listed.
    #[serde(default)]
    pub created: Vec<String>,
    /// Ids whose spec the last apply changed.
    #[serde(default)]
    pub updated: Vec<String>,
    /// Managed ids the last apply's document no longer named that could not
    /// be deleted (unknown kind). Successfully deleted managed ids are not
    /// listed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
    /// Every id an apply of this resource inserted. Later applies update
    /// these rows when the document changes; rows it did not insert are
    /// left as they are.
    #[serde(default)]
    pub managed: Vec<String>,
    /// Revision of the last successful apply. `0` before the first one.
    #[serde(default)]
    pub revision: u64,
    /// Applied specs, oldest first, at most [`REVISION_HISTORY_LIMIT`].
    /// Only stacks keep history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<StackRevision>,
}

impl StackStatus {
    pub fn accepted() -> Self {
        Self {
            phase: StackPhase::Accepted,
            message: ACCEPTED_MESSAGE.to_owned(),
            created: Vec::new(),
            updated: Vec::new(),
            left: Vec::new(),
            managed: Vec::new(),
            revision: 0,
            history: Vec::new(),
        }
    }
}

impl Default for StackStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Stack = Resource<StackSpec, StackStatus>;

/// A stored document. Apply runs the same realize path as a stack.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KitSpec {
    /// Name inside the document, independent of the resource name.
    pub name: String,
    /// The user's stack text, YAML or JSON. Stored as given, references and
    /// all; a value written `{{ param.NAME }}` is substituted at apply.
    pub document: String,
    /// Parameters the document may name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<ParameterDecl>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

impl Spec for KitSpec {
    const KIND: &'static str = KIT_KIND;
}

/// Observed phase for a kit. Same phases as a stack; a kit keeps no
/// revision history.
pub type KitStatus = StackStatus;

pub type Kit = Resource<KitSpec, KitStatus>;

/// An ordered list of stack names in one project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowSpec {
    /// Name inside the spec, independent of the resource name.
    #[serde(default)]
    pub name: String,
    /// Stack names or ids, applied in this order.
    #[serde(default)]
    pub stacks: Vec<String>,
}

impl Spec for WorkflowSpec {
    const KIND: &'static str = WORKFLOW_KIND;
}

/// Observed phase. Apply sets `Failed` when a stack returns an error.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    #[default]
    Accepted,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStatus {
    pub phase: WorkflowPhase,
    pub message: String,
    /// Stack names applied by the last run before it stopped.
    #[serde(default)]
    pub applied: Vec<String>,
}

impl WorkflowStatus {
    pub fn accepted() -> Self {
        Self {
            phase: WorkflowPhase::Accepted,
            message: WORKFLOW_ACCEPTED_MESSAGE.to_owned(),
            applied: Vec::new(),
        }
    }
}

impl Default for WorkflowStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Workflow = Resource<WorkflowSpec, WorkflowStatus>;

/// Reject an empty document. Otherwise accept JSON (`serde_json::Value`) or
/// text that looks like YAML: it contains `:` and does not start with `{`.
pub fn validate_document(document: &str) -> ApiResult<()> {
    let trimmed = document.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("document must not be empty"));
    }
    if serde_json::from_str::<serde_json::Value>(document).is_ok() {
        return Ok(());
    }
    if trimmed.contains(':') && !trimmed.starts_with('{') {
        return Ok(());
    }
    Err(ApiError::bad_request("document must be JSON or YAML"))
}

fn validate_resource_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "spec.name must be 1-128 characters without '/'",
        ));
    }
    Ok(())
}

/// Name, document, and parameters. Trims the name; leaves the document bytes
/// alone. Values are matched against the parameters, so a value for a
/// parameter that is not declared or not named is refused before the stack is
/// stored; a missing value is refused when the document is applied.
pub fn validate_spec(spec: &mut StackSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)?;
    spec.normalize()
}

/// Name, document, and parameters. Same rules as a stack.
pub fn validate_kit(spec: &mut KitSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)?;
    spec.normalize()
}

/// Name plus the ordered stack list. Each entry is trimmed.
pub fn validate_workflow(spec: &mut WorkflowSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    for name in &mut spec.stacks {
        *name = name.trim().to_owned();
        if name.is_empty() || name.len() > 128 || name.contains('/') {
            return Err(ApiError::bad_request(
                "workflow stack names must be 1-128 characters without '/'",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str, default: Option<&str>) -> ParameterDecl {
        ParameterDecl {
            name: name.to_owned(),
            default: default.map(str::to_owned),
            description: None,
        }
    }

    fn spec(
        document: &str,
        declared: &[(&str, Option<&str>)],
        given: &[(&str, &str)],
    ) -> StackSpec {
        StackSpec {
            name: "s".to_owned(),
            document: document.to_owned(),
            declared: declared
                .iter()
                .map(|(n, d)| decl(n, *d))
                .collect::<Vec<_>>(),
            parameters: given
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn message<T: std::fmt::Debug>(result: ApiResult<T>) -> String {
        match result {
            Err(e) => e.message,
            Ok(v) => panic!("expected a refusal, got {v:?}"),
        }
    }

    fn realized(
        document: &str,
        declared: &[(&str, Option<&str>)],
        given: &[(&str, &str)],
    ) -> String {
        spec(document, declared, given)
            .realized()
            .expect("realized")
    }

    #[test]
    fn a_value_replaces_the_reference_it_names() {
        let out = realized(
            "kind: network\nname: \"{{ param.size }}-net\"\nspec: {}\n",
            &[("size", None)],
            &[("size", "big")],
        );
        assert_eq!(out, "kind: network\nname: big-net\nspec: {}\n");
    }

    #[test]
    fn a_default_is_used_when_no_value_is_given() {
        let out = realized(
            "kind: network\nname: \"{{ param.size }}\"\nspec: {}\n",
            &[("size", Some("small"))],
            &[],
        );
        assert_eq!(out, "kind: network\nname: small\nspec: {}\n");
    }

    #[test]
    fn a_given_value_beats_the_default() {
        let out = realized(
            "kind: network\nname: \"{{ param.size }}\"\nspec: {}\n",
            &[("size", Some("small"))],
            &[("size", "big")],
        );
        assert_eq!(out, "kind: network\nname: big\nspec: {}\n");
    }

    #[test]
    fn a_reference_alone_takes_the_type_its_value_reads_as() {
        let out = realized(
            "kind: network\nname: n\nspec:\n  mtu: \"{{ param.mtu }}\"\n  external: \"{{ param.ext }}\"\n  dns_domain: \"{{ param.d }}\"\n",
            &[("mtu", None), ("ext", None), ("d", None)],
            &[("mtu", "1500"), ("ext", "true"), ("d", "local")],
        );
        let doc: serde_json::Value = serde_yaml_ng::from_str(&out).unwrap();
        assert_eq!(doc["spec"]["mtu"], 1500);
        assert_eq!(doc["spec"]["external"], true);
        assert_eq!(doc["spec"]["dns_domain"], "local");
    }

    #[test]
    fn a_reference_in_text_is_replaced_within_it() {
        let out = realized(
            "kind: network\nname: n\nspec:\n  dns_domain: \"{{ param.a }}.{{ param.b }}\"\n",
            &[("a", Some("web")), ("b", Some("local"))],
            &[("a", "db")],
        );
        assert!(out.contains("dns_domain: db.local"), "{out}");
    }

    #[test]
    fn every_reference_in_a_stream_is_replaced() {
        let out = realized(
            "kind: network\nname: \"{{ param.a }}\"\nspec: {}\n---\nkind: network\nname: \"{{ param.b }}\"\nspec: {}\n",
            &[("a", Some("one")), ("b", Some("two"))],
            &[],
        );
        assert!(
            out.contains("name: one") && out.contains("name: two"),
            "{out}"
        );
    }

    #[test]
    fn a_document_with_no_reference_is_kept_as_stored() {
        let doc = "kind: network\nname: app\nspec: {}\n";
        assert_eq!(realized(doc, &[], &[]), doc);
    }

    #[test]
    fn braces_that_name_nothing_are_left_as_written() {
        let out = realized(
            "kind: network\nname: n\nspec:\n  dns_domain: \"{{ jq.field }}\"\n  name2: \"{{ broken\"\n",
            &[],
            &[],
        );
        assert!(out.contains("{{ jq.field }}"), "{out}");
        assert!(out.contains("{{ broken"), "{out}");
    }

    #[test]
    fn a_value_for_an_undeclared_parameter_is_refused_by_name() {
        let mut s = spec(
            "kind: network\nname: n\nspec: {}\n",
            &[("size", Some("small"))],
            &[("colour", "blue")],
        );
        assert_eq!(
            message(s.normalize()),
            "no parameter is declared for 'colour': declare it or drop the value"
        );
    }

    #[test]
    fn a_reference_to_an_undeclared_parameter_is_refused_by_name() {
        let s = spec(
            "kind: network\nname: \"{{ param.gone }}\"\nspec: {}\n",
            &[("size", Some("small"))],
            &[("size", "big")],
        );
        assert_eq!(
            message(s.realized().map(|_| ())),
            "the document names undeclared parameter 'gone'"
        );
    }

    #[test]
    fn a_parameter_with_neither_a_value_nor_a_default_is_refused_by_name() {
        let s = spec(
            "kind: network\nname: n\nspec:\n  mtu: \"{{ param.mtu }}\"\n",
            &[("mtu", None)],
            &[],
        );
        assert_eq!(
            message(s.realized().map(|_| ())),
            "no value given for parameter 'mtu' and it has no default"
        );
    }

    #[test]
    fn a_declared_parameter_the_document_never_names_is_refused() {
        let mut s = spec("kind: network\nname: n\nspec: {}\n", &[("size", None)], &[]);
        assert_eq!(
            message(s.normalize()),
            "the document never names declared parameter 'size'"
        );
    }

    #[test]
    fn a_parameter_declared_twice_is_refused() {
        let mut s = spec(
            "kind: network\nname: \"{{ param.a }}\"\nspec: {}\n",
            &[("a", Some("1")), ("a", Some("2"))],
            &[],
        );
        assert_eq!(message(s.normalize()), "spec.declared declares 'a' twice");
    }

    #[test]
    fn a_name_no_reference_can_carry_is_refused() {
        let mut s = spec(
            "kind: network\nname: \"{{ param.a }}\"\nspec: {}\n",
            &[("9bad", None)],
            &[],
        );
        assert!(
            message(s.normalize()).contains("9bad"),
            "the message names the bad name"
        );
    }

    #[test]
    fn a_reference_where_no_value_belongs_is_refused() {
        let kinded = spec(
            "kind: \"{{ param.k }}\"\nname: n\nspec: {}\n",
            &[("k", Some("network"))],
            &[],
        );
        assert!(
            message(kinded.realized().map(|_| ())).contains("where a value belongs"),
            "a kind is not a value"
        );
        let keyed = spec(
            "kind: network\nname: n\nspec:\n  \"{{ param.k }}\": local\n",
            &[("k", Some("dns_domain"))],
            &[],
        );
        assert!(
            message(keyed.realized().map(|_| ())).contains("where a value belongs"),
            "a key is not a value"
        );
    }

    #[test]
    fn the_param_prefix_is_read_off_a_declared_name() {
        let mut s = spec(
            "kind: network\nname: \"{{ param.size }}\"\nspec: {}\n",
            &[("param.size", Some("small"))],
            &[("param.size", "big")],
        );
        s.normalize().expect("declared by its long spelling");
        assert_eq!(s.declared[0].name, "size");
        assert_eq!(
            s.realized().expect("realized"),
            "kind: network\nname: big\nspec: {}\n"
        );
    }

    #[test]
    fn a_document_that_names_nothing_needs_no_declarations() {
        let mut s = spec("kind: network\nname: app\nspec: {}\n", &[], &[]);
        s.normalize().expect("nothing to check");
        assert_eq!(
            s.realized().expect("realized"),
            "kind: network\nname: app\nspec: {}\n"
        );
    }

    #[test]
    fn a_kit_is_checked_the_same_way_as_a_stack() {
        let mut kit = KitSpec {
            name: "base".to_owned(),
            document: "kind: network\nname: \"{{ param.a }}\"\nspec: {}\n".to_owned(),
            declared: vec![decl("a", None)],
            parameters: BTreeMap::new(),
        };
        kit.normalize().expect("declared and named");
        assert_eq!(
            message(kit.realized().map(|_| ())),
            "no value given for parameter 'a' and it has no default"
        );
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The parameter template engine: reading `{{ param.NAME }}` out of a
//! document and substituting the value a parameter resolves to. Everything
//! here exists to answer "what does this reference become" and is shared by
//! [`substitute`], [`declared_uses`] and the `StackDocument` trait.

use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::parameter_decl::ParameterDecl;

/// The name space a parameter reference may name.
pub(crate) const PARAM_PREFIX: &str = "param.";
/// Start of a parameter reference.
pub(crate) const REF_MARK: &str = "{{";
/// End of a parameter reference.
pub(crate) const REF_END: &str = "}}";

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
pub(crate) fn names(said: &str, param: &str) -> bool {
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
pub(crate) fn given(values: &BTreeMap<String, String>, param: &str) -> Option<String> {
    values
        .get(param)
        .or_else(|| values.get(&format!("{PARAM_PREFIX}{param}")))
        .cloned()
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

pub(crate) fn push(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

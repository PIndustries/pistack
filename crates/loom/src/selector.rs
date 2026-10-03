// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Label selector matching, pod-template hashing and name generation.

use crate::types::{PodTemplate, Selector, SelectorOperator};
use std::collections::BTreeMap;

impl Selector {
    /// A selector that matches exactly these labels.
    pub fn from_labels(labels: &BTreeMap<String, String>) -> Self {
        Selector {
            match_labels: labels.clone(),
            match_expressions: Vec::new(),
        }
    }

    /// An empty selector matches nothing (as in Kubernetes for controllers).
    pub fn is_empty(&self) -> bool {
        self.match_labels.is_empty() && self.match_expressions.is_empty()
    }

    /// Does `labels` satisfy every requirement?
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        if self.is_empty() {
            return false;
        }
        self.match_labels
            .iter()
            .all(|(k, v)| labels.get(k) == Some(v))
            && self.match_expressions.iter().all(|r| match r.operator {
                SelectorOperator::In => labels.get(&r.key).is_some_and(|v| r.values.contains(v)),
                SelectorOperator::NotIn => {
                    !labels.get(&r.key).is_some_and(|v| r.values.contains(v))
                }
                SelectorOperator::Exists => labels.contains_key(&r.key),
                SelectorOperator::DoesNotExist => !labels.contains_key(&r.key),
            })
    }

    /// Return a copy with one more `match_labels` pair.
    pub fn with_label(mut self, key: &str, value: &str) -> Self {
        self.match_labels.insert(key.to_owned(), value.to_owned());
        self
    }
}

/// Do plain labels (a Service selector) match?
pub fn labels_match(
    selector: &BTreeMap<String, String>,
    labels: &BTreeMap<String, String>,
) -> bool {
    !selector.is_empty() && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
}

/// Stable, short hash of a pod template, for the `pod-template-hash` label.
/// FNV-1a over the canonical JSON; serde_json serialises maps in key order
/// so equal templates hash equal across processes and versions.
pub fn template_hash(t: &PodTemplate) -> String {
    let mut t = t.clone();
    // The hash label itself must not feed the hash.
    t.labels.remove(crate::types::POD_TEMPLATE_HASH);
    let json = serde_json::to_vec(&t).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    encode_lower(h.to_be_bytes(), 8)
}

/// Five random lowercase letters, for generated pod names.
pub fn random_suffix() -> String {
    encode_lower(*uuid::Uuid::new_v4().as_bytes(), 5)
}

fn encode_lower<const N: usize>(bytes: [u8; N], len: usize) -> String {
    // Kubernetes-style alphabet without vowels/ambiguous glyphs.
    const ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
    bytes
        .iter()
        .take(len)
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PodSpec, SelectorRequirement};

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn match_labels_and_expressions() {
        let l = labels(&[("app", "web"), ("tier", "frontend")]);
        assert!(Selector::from_labels(&labels(&[("app", "web")])).matches(&l));
        assert!(!Selector::from_labels(&labels(&[("app", "db")])).matches(&l));
        assert!(
            !Selector::default().matches(&l),
            "empty selector matches nothing"
        );

        let req = |key: &str, operator, values: &[&str]| SelectorRequirement {
            key: key.into(),
            operator,
            values: values.iter().map(|s| s.to_string()).collect(),
        };
        let sel = Selector {
            match_labels: BTreeMap::new(),
            match_expressions: vec![
                req("app", SelectorOperator::In, &["web", "api"]),
                req("tier", SelectorOperator::NotIn, &["backend"]),
                req("app", SelectorOperator::Exists, &[]),
                req("canary", SelectorOperator::DoesNotExist, &[]),
            ],
        };
        assert!(sel.matches(&l));
        assert!(!sel.matches(&labels(&[("app", "db"), ("tier", "frontend")])));
        assert!(!sel.matches(&labels(&[("app", "web"), ("tier", "backend")])));
        assert!(!sel.matches(&labels(&[("app", "web"), ("canary", "1")])));
        assert!(!sel.matches(&labels(&[("tier", "frontend")])));
    }

    #[test]
    fn service_selector() {
        let l = labels(&[("app", "web")]);
        assert!(labels_match(&labels(&[("app", "web")]), &l));
        assert!(!labels_match(&labels(&[]), &l));
        assert!(!labels_match(&labels(&[("app", "x")]), &l));
    }

    #[test]
    fn template_hash_is_stable_and_ignores_own_label() {
        let mut t = PodTemplate {
            labels: labels(&[("app", "web")]),
            annotations: BTreeMap::new(),
            spec: PodSpec::default(),
        };
        let a = template_hash(&t);
        assert_eq!(a.len(), 8);
        assert_eq!(a, template_hash(&t));
        t.labels.insert(POD_TEMPLATE_HASH.into(), "whatever".into());
        assert_eq!(a, template_hash(&t));
        t.spec.restart_policy = crate::types::RestartPolicy::Never;
        assert_ne!(a, template_hash(&t));
    }

    #[test]
    fn suffix_is_five_lowercase() {
        let s = random_suffix();
        assert_eq!(s.len(), 5);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    use crate::types::POD_TEMPLATE_HASH;
}

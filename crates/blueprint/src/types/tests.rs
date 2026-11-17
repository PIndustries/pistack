// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::kit_spec::KitSpec;
use super::parameter_decl::ParameterDecl;
use super::stack_document::StackDocument;
use super::stack_spec::StackSpec;

use std::collections::BTreeMap;

fn decl(name: &str, default: Option<&str>) -> ParameterDecl {
    ParameterDecl {
        name: name.to_owned(),
        default: default.map(str::to_owned),
        description: None,
    }
}

fn spec(document: &str, declared: &[(&str, Option<&str>)], given: &[(&str, &str)]) -> StackSpec {
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

fn realized(document: &str, declared: &[(&str, Option<&str>)], given: &[(&str, &str)]) -> String {
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

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ast::Expr;
use super::parse_error::ParseError;
use super::parser::parse;

fn b(e: Expr) -> Box<Expr> {
    Box::new(e)
}

#[test]
fn parses_terms() {
    assert_eq!(parse("true").unwrap(), Expr::True);
    assert_eq!(parse("false").unwrap(), Expr::False);
    assert_eq!(parse("system").unwrap(), Expr::System);
    assert_eq!(parse("project_member").unwrap(), Expr::ProjectMember);
    assert_eq!(parse("owner").unwrap(), Expr::Owner);
    assert_eq!(parse("role:admin").unwrap(), Expr::Role("admin".into()));
    assert_eq!(
        parse("rule:member-rw").unwrap(),
        Expr::Rule("member-rw".into())
    );
    assert_eq!(parse("  ( owner )  ").unwrap(), Expr::Owner);
}

#[test]
fn precedence_and_binds_tighter_than_or() {
    let e = parse("role:admin or project_member and role:member").unwrap();
    assert_eq!(
        e,
        Expr::Or(
            b(Expr::Role("admin".into())),
            b(Expr::And(
                b(Expr::ProjectMember),
                b(Expr::Role("member".into()))
            ))
        )
    );
}

#[test]
fn parentheses_override_precedence() {
    let e = parse("(role:admin or project_member) and role:member").unwrap();
    assert_eq!(
        e,
        Expr::And(
            b(Expr::Or(
                b(Expr::Role("admin".into())),
                b(Expr::ProjectMember)
            )),
            b(Expr::Role("member".into()))
        )
    );
}

#[test]
fn not_is_prefix_and_stacks() {
    assert_eq!(parse("not owner").unwrap(), Expr::Not(b(Expr::Owner)));
    assert_eq!(
        parse("not not owner").unwrap(),
        Expr::Not(b(Expr::Not(b(Expr::Owner))))
    );
    // `not` binds tighter than `and`.
    assert_eq!(
        parse("not owner and system").unwrap(),
        Expr::And(b(Expr::Not(b(Expr::Owner))), b(Expr::System))
    );
    assert_eq!(
        parse("not (owner and system)").unwrap(),
        Expr::Not(b(Expr::And(b(Expr::Owner), b(Expr::System))))
    );
}

#[test]
fn left_associative() {
    let e = parse("true or false or system").unwrap();
    assert_eq!(
        e,
        Expr::Or(b(Expr::Or(b(Expr::True), b(Expr::False))), b(Expr::System))
    );
}

#[test]
fn display_roundtrips() {
    for src in [
        "role:admin or project_member and role:member",
        "(role:admin or project_member) and role:member",
        "not (owner and system)",
        "not owner and system",
        "not not owner",
        "true",
        "rule:a or rule:b or rule:c",
        "rule:a or (rule:b or rule:c)",
    ] {
        let e = parse(src).unwrap();
        let rendered = e.to_string();
        let again = parse(&rendered).unwrap_or_else(|err| panic!("{rendered}: {err}"));
        assert_eq!(again, e, "{src} -> {rendered}");
    }
}

#[test]
fn rule_refs_collected() {
    let e = parse("rule:a and (rule:b or not rule:c) or role:x").unwrap();
    assert_eq!(e.rule_refs(), vec!["a", "b", "c"]);
}

fn err(src: &str) -> ParseError {
    parse(src).expect_err(src)
}

#[test]
fn error_empty() {
    let e = err("");
    assert_eq!(e.column, 1);
    assert_eq!(e.message, "empty expression");
    assert_eq!(err("   ").message, "empty expression");
}

#[test]
fn error_unknown_term_names_it_and_column() {
    let e = err("role:admin or admin");
    assert_eq!(e.column, 15);
    assert!(
        e.message.starts_with("unknown term 'admin'"),
        "{}",
        e.message
    );
    assert!(e.message.contains("role:<name>"));
    assert_eq!(e.to_string(), format!("column 15: {}", e.message));
}

#[test]
fn error_keywords_are_case_sensitive() {
    let e = err("TRUE");
    assert!(e.message.starts_with("unknown term 'TRUE'"));
    let e = err("owner AND system");
    assert!(
        e.message
            .contains("unexpected 'AND' after complete expression"),
        "{}",
        e.message
    );
}

#[test]
fn error_missing_name() {
    let e = err("role:");
    assert_eq!(e.column, 1);
    assert_eq!(e.message, "'role:' needs a name, e.g. role:admin");
    let e = err("owner or rule:");
    assert_eq!(e.column, 10);
    assert_eq!(e.message, "'rule:' needs a name, e.g. rule:admin");
    let e = err("role:a:b");
    assert_eq!(e.message, "role name 'a:b' may not contain ':'");
}

#[test]
fn error_unbalanced_parens() {
    let e = err("(owner or system");
    assert_eq!(e.column, 17);
    assert_eq!(e.message, "expected ')' to close '(' at column 1");

    let e = err("owner or system)");
    assert_eq!(e.column, 16);
    assert_eq!(e.message, "unexpected ')' after complete expression");

    let e = err(")");
    assert_eq!(e.message, "unexpected ')'");

    let e = err("(owner system)");
    assert_eq!(e.column, 8);
    assert_eq!(
        e.message,
        "expected ')' to close '(' at column 1, found 'system'"
    );
}

#[test]
fn error_dangling_operator() {
    let e = err("owner and");
    assert_eq!(e.column, 10);
    assert_eq!(e.message, "unexpected end of expression; expected a term");

    let e = err("and owner");
    assert_eq!(e.column, 1);
    assert_eq!(e.message, "unexpected 'and'; expected a term before it");

    let e = err("owner or or system");
    assert_eq!(e.column, 10);
    assert_eq!(e.message, "unexpected 'or'; expected a term before it");

    let e = err("not");
    assert_eq!(e.message, "unexpected end of expression; expected a term");
}

#[test]
fn error_two_terms_without_operator() {
    let e = err("owner system");
    assert_eq!(e.column, 7);
    assert_eq!(e.message, "unexpected 'system' after complete expression");
}

#[test]
fn error_bad_character() {
    let e = err("owner && system");
    assert_eq!(e.column, 7);
    assert_eq!(e.message, "unexpected character '&'");
    let e = err("role:admin!");
    assert_eq!(e.column, 11);
}

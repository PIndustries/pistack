// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The rule expression language: AST, lexer and a small recursive-descent
//! parser. No external parser crate; the grammar is tiny:
//!
//! ```text
//! expr    := or
//! or      := and ( "or" and )*
//! and     := not ( "and" not )*
//! not     := "not" not | primary
//! primary := "(" expr ")"
//!          | "true" | "false"
//!          | "system"          # caller holds a system-scoped token
//!          | "project_member"  # caller.project == target.project
//!          | "owner"           # caller.principal == target.owner
//!          | "role:" NAME      # caller has the named role
//!          | "rule:" NAME      # reference to a named rule in the document
//! NAME    := [A-Za-z0-9_.-]+
//! ```
//!
//! Keywords are case-sensitive and lower-case. Any other bare word is an
//! error; the message names the word and its column so a typo in a policy
//! file is found quickly.

use std::fmt;

/// A parsed rule expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    True,
    False,
    System,
    ProjectMember,
    Owner,
    Role(String),
    Rule(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Every `rule:<name>` referenced anywhere in this expression.
    pub fn rule_refs(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_rule_refs(&mut out);
        out
    }

    fn collect_rule_refs<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Rule(name) => out.push(name),
            Expr::Not(e) => e.collect_rule_refs(out),
            Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect_rule_refs(out);
                b.collect_rule_refs(out);
            }
            _ => {}
        }
    }

    fn precedence(&self) -> u8 {
        match self {
            Expr::Or(..) => 1,
            Expr::And(..) => 2,
            Expr::Not(..) => 3,
            _ => 4,
        }
    }

    /// Left operands of equal precedence need no parentheses (operators are
    /// left-associative); right operands do, so the tree shape survives a
    /// `Display` -> `parse` round trip.
    fn fmt_child(
        child: &Expr,
        parent_prec: u8,
        right: bool,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let p = child.precedence();
        if p < parent_prec || (right && p == parent_prec) {
            write!(f, "({child})")
        } else {
            write!(f, "{child}")
        }
    }
}

impl fmt::Display for Expr {
    /// Renders the expression in a form `parse` accepts again, with the
    /// minimum parentheses needed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::True => f.write_str("true"),
            Expr::False => f.write_str("false"),
            Expr::System => f.write_str("system"),
            Expr::ProjectMember => f.write_str("project_member"),
            Expr::Owner => f.write_str("owner"),
            Expr::Role(r) => write!(f, "role:{r}"),
            Expr::Rule(r) => write!(f, "rule:{r}"),
            Expr::Not(e) => {
                f.write_str("not ")?;
                Expr::fmt_child(e, 3, false, f)
            }
            Expr::And(a, b) => {
                Expr::fmt_child(a, 2, false, f)?;
                f.write_str(" and ")?;
                Expr::fmt_child(b, 2, true, f)
            }
            Expr::Or(a, b) => {
                Expr::fmt_child(a, 1, false, f)?;
                f.write_str(" or ")?;
                Expr::fmt_child(b, 1, true, f)
            }
        }
    }
}

/// A syntax error with the 1-based column it was found at.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("column {column}: {message}")]
pub struct ParseError {
    pub column: usize,
    pub message: String,
}

impl ParseError {
    fn new(column: usize, message: impl Into<String>) -> Self {
        Self {
            column,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    Word(String),
}

#[derive(Clone, Debug)]
struct Spanned {
    tok: Tok,
    /// 1-based column of the first character.
    column: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')
}

fn lex(src: &str) -> Result<Vec<Spanned>, ParseError> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (_, c) = chars[i];
        let column = i + 1;
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Spanned {
                tok: Tok::LParen,
                column,
            });
            i += 1;
        } else if c == ')' {
            out.push(Spanned {
                tok: Tok::RParen,
                column,
            });
            i += 1;
        } else if is_word_char(c) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i].1) {
                i += 1;
            }
            let word: String = chars[start..i].iter().map(|(_, c)| *c).collect();
            out.push(Spanned {
                tok: Tok::Word(word),
                column,
            });
        } else {
            return Err(ParseError::new(
                column,
                format!("unexpected character {c:?}"),
            ));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
    end_column: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Spanned> {
        self.toks.get(self.pos)
    }

    fn peek_word(&self) -> Option<&str> {
        match self.peek() {
            Some(Spanned {
                tok: Tok::Word(w), ..
            }) => Some(w.as_str()),
            _ => None,
        }
    }

    fn bump(&mut self) -> Option<Spanned> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and()?;
        while self.peek_word() == Some("or") {
            self.bump();
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.not()?;
        while self.peek_word() == Some("and") {
            self.bump();
            let right = self.not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, ParseError> {
        if self.peek_word() == Some("not") {
            self.bump();
            let inner = self.not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let Some(t) = self.bump() else {
            return Err(ParseError::new(
                self.end_column,
                "unexpected end of expression; expected a term",
            ));
        };
        match t.tok {
            Tok::LParen => {
                let inner = self.expr()?;
                match self.bump() {
                    Some(Spanned {
                        tok: Tok::RParen, ..
                    }) => Ok(inner),
                    Some(other) => Err(ParseError::new(
                        other.column,
                        format!(
                            "expected ')' to close '(' at column {}, found {}",
                            t.column,
                            describe(&other.tok)
                        ),
                    )),
                    None => Err(ParseError::new(
                        self.end_column,
                        format!("expected ')' to close '(' at column {}", t.column),
                    )),
                }
            }
            Tok::RParen => Err(ParseError::new(t.column, "unexpected ')'")),
            Tok::Word(w) => match w.as_str() {
                "true" => Ok(Expr::True),
                "false" => Ok(Expr::False),
                "system" => Ok(Expr::System),
                "project_member" => Ok(Expr::ProjectMember),
                "owner" => Ok(Expr::Owner),
                "and" | "or" => Err(ParseError::new(
                    t.column,
                    format!("unexpected '{w}'; expected a term before it"),
                )),
                "not" => Err(ParseError::new(t.column, "unexpected 'not'")),
                _ => {
                    if let Some(name) = w.strip_prefix("role:") {
                        return name_term(name, t.column, "role", Expr::Role);
                    }
                    if let Some(name) = w.strip_prefix("rule:") {
                        return name_term(name, t.column, "rule", Expr::Rule);
                    }
                    Err(ParseError::new(
                        t.column,
                        format!(
                            "unknown term '{w}'; expected one of true, false, system, project_member, owner, role:<name>, rule:<name>"
                        ),
                    ))
                }
            },
        }
    }
}

fn name_term(
    name: &str,
    column: usize,
    what: &str,
    mk: fn(String) -> Expr,
) -> Result<Expr, ParseError> {
    if name.is_empty() {
        return Err(ParseError::new(
            column,
            format!("'{what}:' needs a name, e.g. {what}:admin"),
        ));
    }
    if name.contains(':') {
        return Err(ParseError::new(
            column,
            format!("{what} name '{name}' may not contain ':'"),
        ));
    }
    Ok(mk(name.to_owned()))
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::LParen => "'('".into(),
        Tok::RParen => "')'".into(),
        Tok::Word(w) => format!("'{w}'"),
    }
}

/// Parse one rule expression.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = lex(src)?;
    let end_column = src.chars().count() + 1;
    if toks.is_empty() {
        return Err(ParseError::new(1, "empty expression"));
    }
    let mut p = Parser {
        toks,
        pos: 0,
        end_column,
    };
    let e = p.expr()?;
    if let Some(extra) = p.peek() {
        return Err(ParseError::new(
            extra.column,
            format!(
                "unexpected {} after complete expression",
                describe(&extra.tok)
            ),
        ));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

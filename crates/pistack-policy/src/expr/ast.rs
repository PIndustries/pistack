// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Expr`: a parsed rule expression.

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

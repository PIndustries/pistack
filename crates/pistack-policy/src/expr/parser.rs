// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The lexer and recursive-descent parser behind [`parse`].

use super::ast::Expr;
use super::parse_error::ParseError;

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

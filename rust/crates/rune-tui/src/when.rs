//! A recursive-descent parser + evaluator for binding `when` clauses (plan
//! WP6.S2). The floor is deliberately bare: a bare `ident` marker (a boolean
//! context field), `ident == "value"` (a string-valued field), `!`, `&&`,
//! `||`, and parentheses for grouping — no regex, no `in`, no numeric
//! comparison. That floor is what VS Code's whole `keybindings.json` needs
//! and what Zed's entire vim keymap is expressed in.

use std::fmt;

use crate::focus::FocusTarget;

/// The context a `when` clause is evaluated against — one flat snapshot of
/// "what's true right now", threaded in fresh at each keystroke rather than
/// captured by the clause itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub focus: FocusTarget,
    pub search_open: bool,
    pub has_selection: bool,
    pub has_multi_cursor: bool,
    pub read_only: bool,
    pub modal_open: bool,
    pub language: Option<&'static str>,
    /// The vim-set's normal/insert marker (plan WP6.S8). `"insert"` is this
    /// port's whole notion of "insert mode" while full vim modal editing
    /// stays out of scope (plan Goal) — see `keymap::vim`'s doc comment.
    pub mode: &'static str,
}

impl Default for Context {
    fn default() -> Context {
        Context {
            focus: FocusTarget::Editor,
            search_open: false,
            has_selection: false,
            has_multi_cursor: false,
            read_only: false,
            modal_open: false,
            language: None,
            mode: "insert",
        }
    }
}

impl Context {
    /// Looks a bare identifier up as a `bool` marker. `None` for an
    /// identifier this `Context` doesn't know — treated as `false` by
    /// `eval`'s `Marker` arm rather than a parse or evaluation error: an
    /// unknown identifier in a `when` clause is a binding-table typo, not a
    /// user-facing failure, and CONSTITUTION §1.3 forbids panicking to
    /// surface it.
    fn bool_field(&self, ident: &str) -> Option<bool> {
        match ident {
            "search_open" => Some(self.search_open),
            "has_selection" => Some(self.has_selection),
            "has_multi_cursor" => Some(self.has_multi_cursor),
            "read_only" => Some(self.read_only),
            "modal_open" => Some(self.modal_open),
            _ => None,
        }
    }

    /// Looks a bare identifier up as a string-valued field, for `==`
    /// comparisons. `focus` renders through its `Debug` output (`"Editor"`,
    /// `"Explorer"`, ...) — the exact spelling the plan's own example
    /// (`focus == "Editor"`) uses.
    fn string_field(&self, ident: &str) -> Option<String> {
        match ident {
            "focus" => Some(format!("{:?}", self.focus)),
            "mode" => Some(self.mode.to_string()),
            "language" => self.language.map(str::to_string),
            _ => None,
        }
    }
}

/// A parsed `when` clause's AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Marker(String),
    Eq(String, String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "when-clause parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Str(String),
    AndAnd,
    OrOr,
    Bang,
    EqEq,
    LParen,
    RParen,
}

fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            '!' => {
                chars.next();
                tokens.push(Token::Bang);
            }
            '&' => {
                chars.next();
                if chars.next() != Some('&') {
                    return Err(ParseError("expected `&&`".to_string()));
                }
                tokens.push(Token::AndAnd);
            }
            '|' => {
                chars.next();
                if chars.next() != Some('|') {
                    return Err(ParseError("expected `||`".to_string()));
                }
                tokens.push(Token::OrOr);
            }
            '=' => {
                chars.next();
                if chars.next() != Some('=') {
                    return Err(ParseError("expected `==`".to_string()));
                }
                tokens.push(Token::EqEq);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some(ch) => s.push(ch),
                        None => return Err(ParseError("unterminated string literal".to_string())),
                    }
                }
                tokens.push(Token::Str(s));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut s = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        s.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(s));
            }
            other => return Err(ParseError(format!("unexpected character {other:?}"))),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(Token::OrOr)) {
            self.pos += 1;
            let rhs = self.parse_and()?;
            lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        while matches!(self.peek(), Some(Token::AndAnd)) {
            self.pos += 1;
            let rhs = self.parse_unary()?;
            lhs = Expr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Some(Token::Bang)) {
            self.pos += 1;
            let inner = self.parse_unary()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.peek().cloned() {
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.parse_expr()?;
                match self.peek() {
                    Some(Token::RParen) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err(ParseError("expected `)`".to_string())),
                }
            }
            Some(Token::Ident(name)) => {
                self.pos += 1;
                if matches!(self.peek(), Some(Token::EqEq)) {
                    self.pos += 1;
                    match self.peek().cloned() {
                        Some(Token::Str(value)) => {
                            self.pos += 1;
                            Ok(Expr::Eq(name, value))
                        }
                        _ => Err(ParseError(
                            "expected a string literal after `==`".to_string(),
                        )),
                    }
                } else {
                    Ok(Expr::Marker(name))
                }
            }
            other => Err(ParseError(format!("unexpected token {other:?}"))),
        }
    }
}

/// Parses one `when` clause. Never called on an empty string by
/// `crate::keymap::index` (empty means unconditional and skips the parser
/// entirely) — an empty `src` here fails with `ParseError`, same as any
/// other malformed input.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let expr = parser.parse_expr()?;
    if parser.pos != tokens.len() {
        return Err(ParseError(
            "trailing tokens after a complete expression".to_string(),
        ));
    }
    Ok(expr)
}

pub fn eval(expr: &Expr, ctx: &Context) -> bool {
    match expr {
        Expr::Marker(name) => ctx.bool_field(name).unwrap_or(false),
        Expr::Eq(name, value) => ctx.string_field(name).as_deref() == Some(value.as_str()),
        Expr::Not(inner) => !eval(inner, ctx),
        Expr::And(a, b) => eval(a, ctx) && eval(b, ctx),
        Expr::Or(a, b) => eval(a, ctx) || eval(b, ctx),
    }
}

/// Parses then evaluates in one call — the shape `crate::keymap::index`
/// actually needs at resolution time.
pub fn evaluate(src: &str, ctx: &Context) -> Result<bool, ParseError> {
    Ok(eval(&parse(src)?, ctx))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_evaluates_focus_and_not_read_only_both_ways() {
        let expr = parse(r#"focus == "Editor" && !read_only"#).expect("parses");

        let editable = Context {
            focus: FocusTarget::Editor,
            read_only: false,
            ..Context::default()
        };
        assert!(eval(&expr, &editable));

        let locked = Context {
            focus: FocusTarget::Editor,
            read_only: true,
            ..Context::default()
        };
        assert!(!eval(&expr, &locked));

        let wrong_focus = Context {
            focus: FocusTarget::Explorer,
            read_only: false,
            ..Context::default()
        };
        assert!(!eval(&expr, &wrong_focus));
    }

    #[test]
    fn bare_marker_is_a_bool_field() {
        let expr = parse("has_selection").expect("parses");
        assert!(eval(
            &expr,
            &Context {
                has_selection: true,
                ..Context::default()
            }
        ));
        assert!(!eval(&expr, &Context::default()));
    }

    #[test]
    fn negation_and_or_short_circuit_correctly() {
        let expr = parse("!modal_open || has_selection").expect("parses");
        assert!(eval(&expr, &Context::default()));
        assert!(!eval(
            &expr,
            &Context {
                modal_open: true,
                has_selection: false,
                ..Context::default()
            }
        ));
    }

    #[test]
    fn parentheses_group_correctly() {
        let expr = parse("!(has_selection || has_multi_cursor)").expect("parses");
        assert!(eval(&expr, &Context::default()));
        assert!(!eval(
            &expr,
            &Context {
                has_multi_cursor: true,
                ..Context::default()
            }
        ));
    }

    #[test]
    fn an_unknown_identifier_is_false_not_an_error() {
        let expr = parse("not_a_real_field").expect("parses");
        assert!(!eval(&expr, &Context::default()));
    }

    #[test]
    fn malformed_input_is_a_parse_error_not_a_panic() {
        assert!(parse("focus ==").is_err());
        assert!(parse("focus == \"Editor\" &&").is_err());
        assert!(parse("(has_selection").is_err());
        assert!(parse("has_selection)").is_err());
    }

    #[test]
    fn evaluate_parses_and_evals_in_one_call() {
        assert_eq!(evaluate("has_selection", &Context::default()), Ok(false));
        assert!(evaluate("focus ==", &Context::default()).is_err());
    }
}

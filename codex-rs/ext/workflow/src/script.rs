use serde_json::Number as JsonNumber;
use serde_json::Value as JsonValue;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Stmt {
    Let {
        name: String,
        expr: Expr,
    },
    Expr(Expr),
    Return(Expr),
    If {
        condition: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Vec<Stmt>,
    },
    ForOf {
        item: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Identifier(String),
    Array(Vec<Expr>),
    Object(Vec<ObjectProperty>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Await(Box<Expr>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    ArrowFunction {
        params: Vec<String>,
        body: FunctionBody,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectProperty {
    pub(crate) key: String,
    pub(crate) value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FunctionBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

pub(crate) fn parse_workflow_body(body: &str) -> Result<Vec<Stmt>, String> {
    let tokens = Lexer::new(body).tokenize()?;
    let mut parser = Parser { tokens, index: 0 };
    parser.parse_program()
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    String(String),
    Number(f64),
    Symbol(String),
    Eof,
}

struct Lexer<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        while !self.is_eof() {
            self.skip_ws_and_comments();
            let Some(ch) = self.peek_char() else {
                break;
            };
            if is_identifier_start(ch) {
                tokens.push(Token::Ident(self.read_identifier()));
                continue;
            }
            if ch == '\'' || ch == '"' {
                tokens.push(Token::String(self.read_string()?));
                continue;
            }
            if ch == '`' {
                tokens.push(Token::String(self.read_template_string()?));
                continue;
            }
            if ch.is_ascii_digit() {
                tokens.push(Token::Number(self.read_number()?));
                continue;
            }
            if let Some(symbol) = self.read_multi_char_symbol() {
                tokens.push(Token::Symbol(symbol));
                continue;
            }
            self.bump_char();
            tokens.push(Token::Symbol(ch.to_string()));
        }
        tokens.push(Token::Eof);
        Ok(tokens)
    }

    fn is_eof(&self) -> bool {
        self.position >= self.source.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.position += ch.len_utf8();
        Some(ch)
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.peek_char().is_some_and(char::is_whitespace) {
                self.bump_char();
            }
            if self.source[self.position..].starts_with("//") {
                while let Some(ch) = self.bump_char() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.source[self.position..].starts_with("/*") {
                self.position += 2;
                while !self.is_eof() && !self.source[self.position..].starts_with("*/") {
                    self.bump_char();
                }
                if self.source[self.position..].starts_with("*/") {
                    self.position += 2;
                }
                continue;
            }
            break;
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while self.peek_char().is_some_and(is_identifier_continue) {
            self.bump_char();
        }
        self.source[start..self.position].to_string()
    }

    fn read_string(&mut self) -> Result<String, String> {
        let quote = self.bump_char().unwrap_or_default();
        let mut value = String::new();
        while let Some(ch) = self.bump_char() {
            if ch == quote {
                return Ok(value);
            }
            if ch == '\\' {
                value.push(self.parse_escape()?);
            } else {
                value.push(ch);
            }
        }
        Err("unterminated string literal".to_string())
    }

    fn read_template_string(&mut self) -> Result<String, String> {
        self.bump_char();
        let mut value = String::new();
        while let Some(ch) = self.bump_char() {
            match ch {
                '`' => return Ok(value),
                '\\' => value.push(self.parse_escape()?),
                '$' if self.peek_char() == Some('{') => {
                    return Err(
                        "template interpolation is not supported by the Rust workflow runtime"
                            .to_string(),
                    );
                }
                _ => value.push(ch),
            }
        }
        Err("unterminated template literal".to_string())
    }

    fn parse_escape(&mut self) -> Result<char, String> {
        match self.bump_char() {
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('t') => Ok('\t'),
            Some('b') => Ok('\u{0008}'),
            Some('f') => Ok('\u{000c}'),
            Some('v') => Ok('\u{000b}'),
            Some('0') => Ok('\0'),
            Some('\'') => Ok('\''),
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('`') => Ok('`'),
            Some('u') => self.parse_unicode_escape(),
            Some(ch) => Ok(ch),
            None => Err("unterminated escape sequence".to_string()),
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(ch) = self.bump_char().and_then(|ch| ch.to_digit(16)) else {
                return Err("invalid unicode escape".to_string());
            };
            value = (value << 4) + ch;
        }
        char::from_u32(value).ok_or_else(|| "invalid unicode escape".to_string())
    }

    fn read_number(&mut self) -> Result<f64, String> {
        let start = self.position;
        while self
            .peek_char()
            .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-'))
        {
            self.bump_char();
        }
        self.source[start..self.position]
            .parse::<f64>()
            .map_err(|_| "invalid number literal".to_string())
    }

    fn read_multi_char_symbol(&mut self) -> Option<String> {
        for symbol in [
            "===", "!==", "=>", "&&", "||", "<=", ">=", "==", "!=", "?.", "::",
        ] {
            if self.source[self.position..].starts_with(symbol) {
                self.position += symbol.len();
                return Some(symbol.to_string());
            }
        }
        None
    }
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        self.parse_statements_until("EOF")
    }

    fn parse_statements_until(&mut self, terminator: &str) -> Result<Vec<Stmt>, String> {
        let mut statements = Vec::new();
        while !self.is_eof() && !self.peek_symbol(terminator) {
            if self.consume_symbol(";") {
                continue;
            }
            statements.push(self.parse_statement()?);
        }
        Ok(statements)
    }

    fn parse_statement(&mut self) -> Result<Stmt, String> {
        if self.consume_ident("if") {
            let condition = if self.consume_symbol("(") {
                let condition = self.parse_expression()?;
                self.expect_symbol(")")?;
                condition
            } else {
                self.parse_expression()?
            };
            let then_branch = self.parse_block()?;
            let else_branch = if self.consume_ident("else") {
                if self.peek_ident("if") {
                    vec![self.parse_statement()?]
                } else {
                    self.parse_block()?
                }
            } else {
                Vec::new()
            };
            return Ok(Stmt::If {
                condition,
                then_branch,
                else_branch,
            });
        }

        if self.consume_ident("for") {
            let (item, iterable) = if self.consume_symbol("(") {
                let _ = self.consume_ident("const")
                    || self.consume_ident("let")
                    || self.consume_ident("var");
                let item = self.expect_identifier()?;
                self.expect_ident("of")?;
                let iterable = self.parse_expression()?;
                self.expect_symbol(")")?;
                (item, iterable)
            } else {
                let item = self.expect_identifier()?;
                self.expect_ident("in")?;
                let iterable = self.parse_expression()?;
                (item, iterable)
            };
            let body = self.parse_block()?;
            return Ok(Stmt::ForOf {
                item,
                iterable,
                body,
            });
        }

        if self.consume_ident("return") {
            let expr = self.parse_expression()?;
            self.consume_symbol(";");
            return Ok(Stmt::Return(expr));
        }

        if self.consume_ident("const") || self.consume_ident("let") || self.consume_ident("var") {
            let name = self.expect_identifier()?;
            let expr = if self.consume_symbol("=") {
                self.parse_expression()?
            } else {
                Expr::Null
            };
            self.consume_symbol(";");
            return Ok(Stmt::Let { name, expr });
        }

        let expr = self.parse_expression()?;
        self.consume_symbol(";");
        Ok(Stmt::Expr(expr))
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect_symbol("{")?;
        let statements = self.parse_statements_until("}")?;
        self.expect_symbol("}")?;
        Ok(statements)
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        self.parse_arrow_or_binary()
    }

    fn parse_arrow_or_binary(&mut self) -> Result<Expr, String> {
        if let Some(params) = self.try_parse_rust_closure_params()? {
            let body = if self.peek_symbol("{") {
                FunctionBody::Block(self.parse_block()?)
            } else {
                FunctionBody::Expr(Box::new(self.parse_expression()?))
            };
            return Ok(Expr::ArrowFunction { params, body });
        }
        if let Some(params) = self.try_parse_arrow_params()? {
            self.expect_symbol("=>")?;
            let body = if self.peek_symbol("{") {
                FunctionBody::Block(self.parse_block()?)
            } else {
                FunctionBody::Expr(Box::new(self.parse_expression()?))
            };
            return Ok(Expr::ArrowFunction { params, body });
        }
        self.parse_binary(0)
    }

    fn try_parse_rust_closure_params(&mut self) -> Result<Option<Vec<String>>, String> {
        if self.consume_symbol("||") {
            return Ok(Some(Vec::new()));
        }
        if !self.peek_symbol("|") {
            return Ok(None);
        }
        let saved = self.index;
        self.advance();
        let mut params = Vec::new();
        if !self.peek_symbol("|") {
            loop {
                match self.peek().clone() {
                    Token::Ident(name) => {
                        self.advance();
                        params.push(name);
                    }
                    _ => {
                        self.index = saved;
                        return Ok(None);
                    }
                }
                if !self.consume_symbol(",") {
                    break;
                }
                if self.peek_symbol("|") {
                    break;
                }
            }
        }
        if !self.consume_symbol("|") {
            self.index = saved;
            return Ok(None);
        }
        Ok(Some(params))
    }

    fn try_parse_arrow_params(&mut self) -> Result<Option<Vec<String>>, String> {
        if let Token::Ident(name) = self.peek().clone()
            && self.peek_n_symbol(1, "=>")
        {
            self.advance();
            return Ok(Some(vec![name]));
        }

        if !self.peek_symbol("(") {
            return Ok(None);
        }
        let saved = self.index;
        self.advance();
        let mut params = Vec::new();
        if !self.peek_symbol(")") {
            loop {
                match self.peek().clone() {
                    Token::Ident(name) => {
                        self.advance();
                        params.push(name);
                    }
                    _ => {
                        self.index = saved;
                        return Ok(None);
                    }
                }
                if !self.consume_symbol(",") {
                    break;
                }
            }
        }
        if !self.consume_symbol(")") || !self.peek_symbol("=>") {
            self.index = saved;
            return Ok(None);
        }
        Ok(Some(params))
    }

    fn parse_binary(&mut self, min_precedence: u8) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        while let Some((op, precedence)) = self.peek_binary_op() {
            if precedence < min_precedence {
                break;
            }
            self.advance();
            let right = self.parse_binary(precedence + 1)?;
            left = Expr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.consume_ident("await") {
            return Ok(Expr::Await(Box::new(self.parse_unary()?)));
        }
        if self.consume_symbol("!") {
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.consume_symbol("-") {
            return Ok(Expr::Unary {
                op: UnaryOp::Negate,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.consume_symbol(".") || self.consume_symbol("?.") {
                let name = self.expect_identifier()?;
                expr = Expr::Member(Box::new(expr), name);
                continue;
            }
            if self.consume_symbol("[") {
                let index = self.parse_expression()?;
                self.expect_symbol("]")?;
                expr = Expr::Index(Box::new(expr), Box::new(index));
                continue;
            }
            if self.consume_symbol("(") {
                let mut args = Vec::new();
                if !self.peek_symbol(")") {
                    loop {
                        args.push(self.parse_expression()?);
                        if !self.consume_symbol(",") {
                            break;
                        }
                        if self.peek_symbol(")") {
                            break;
                        }
                    }
                }
                self.expect_symbol(")")?;
                expr = Expr::Call(Box::new(expr), args);
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Ident(name) if name == "true" => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            Token::Ident(name) if name == "false" => {
                self.advance();
                Ok(Expr::Bool(false))
            }
            Token::Ident(name) if name == "null" || name == "undefined" => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::Ident(name) => {
                self.advance();
                Ok(Expr::Identifier(name))
            }
            Token::String(value) => {
                self.advance();
                Ok(Expr::String(value))
            }
            Token::Number(value) => {
                self.advance();
                Ok(Expr::Number(value))
            }
            Token::Symbol(symbol) if symbol == "(" => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect_symbol(")")?;
                Ok(expr)
            }
            Token::Symbol(symbol) if symbol == "[" => self.parse_array(),
            Token::Symbol(symbol) if symbol == "{" => self.parse_object(),
            other => Err(format!(
                "unexpected token in workflow expression: {other:?}"
            )),
        }
    }

    fn parse_array(&mut self) -> Result<Expr, String> {
        self.expect_symbol("[")?;
        let mut values = Vec::new();
        if !self.peek_symbol("]") {
            loop {
                values.push(self.parse_expression()?);
                if !self.consume_symbol(",") {
                    break;
                }
                if self.peek_symbol("]") {
                    break;
                }
            }
        }
        self.expect_symbol("]")?;
        Ok(Expr::Array(values))
    }

    fn parse_object(&mut self) -> Result<Expr, String> {
        self.expect_symbol("{")?;
        let mut properties = Vec::new();
        if !self.peek_symbol("}") {
            loop {
                let key = match self.peek().clone() {
                    Token::Ident(name) => {
                        self.advance();
                        name
                    }
                    Token::String(value) => {
                        self.advance();
                        value
                    }
                    Token::Number(value) => {
                        self.advance();
                        json_number(value).to_string()
                    }
                    other => return Err(format!("unsupported object key in workflow: {other:?}")),
                };
                let value = if self.consume_symbol(":") {
                    self.parse_expression()?
                } else {
                    Expr::Identifier(key.clone())
                };
                properties.push(ObjectProperty { key, value });
                if !self.consume_symbol(",") {
                    break;
                }
                if self.peek_symbol("}") {
                    break;
                }
            }
        }
        self.expect_symbol("}")?;
        Ok(Expr::Object(properties))
    }

    fn peek_binary_op(&self) -> Option<(BinaryOp, u8)> {
        let Token::Symbol(symbol) = self.peek() else {
            return None;
        };
        match symbol.as_str() {
            "||" => Some((BinaryOp::Or, 1)),
            "&&" => Some((BinaryOp::And, 2)),
            "==" | "===" => Some((BinaryOp::Equal, 3)),
            "!=" | "!==" => Some((BinaryOp::NotEqual, 3)),
            "<" => Some((BinaryOp::Less, 4)),
            "<=" => Some((BinaryOp::LessEqual, 4)),
            ">" => Some((BinaryOp::Greater, 4)),
            ">=" => Some((BinaryOp::GreaterEqual, 4)),
            "+" => Some((BinaryOp::Add, 5)),
            "-" => Some((BinaryOp::Subtract, 5)),
            "*" => Some((BinaryOp::Multiply, 6)),
            "/" => Some((BinaryOp::Divide, 6)),
            "%" => Some((BinaryOp::Remainder, 6)),
            _ => None,
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.index).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) {
        if !self.is_eof() {
            self.index += 1;
        }
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn peek_ident(&self, ident: &str) -> bool {
        matches!(self.peek(), Token::Ident(value) if value == ident)
    }

    fn consume_ident(&mut self, ident: &str) -> bool {
        if self.peek_ident(ident) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self, ident: &str) -> Result<(), String> {
        if self.consume_ident(ident) {
            Ok(())
        } else {
            Err(format!("expected `{ident}` in workflow script"))
        }
    }

    fn expect_identifier(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(format!(
                "expected identifier in workflow script, got {other:?}"
            )),
        }
    }

    fn peek_symbol(&self, symbol: &str) -> bool {
        if symbol == "EOF" {
            return matches!(self.peek(), Token::Eof);
        }
        matches!(self.peek(), Token::Symbol(value) if value == symbol)
    }

    fn peek_n_symbol(&self, offset: usize, symbol: &str) -> bool {
        matches!(
            self.tokens.get(self.index + offset),
            Some(Token::Symbol(value)) if value == symbol
        )
    }

    fn consume_symbol(&mut self, symbol: &str) -> bool {
        if self.peek_symbol(symbol) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<(), String> {
        if self.consume_symbol(symbol) {
            Ok(())
        } else {
            Err(format!("expected `{symbol}` in workflow script"))
        }
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit()
}

fn json_number(value: f64) -> JsonNumber {
    JsonNumber::from_f64(value).unwrap_or_else(|| JsonNumber::from(0))
}

pub(crate) fn json_value_from_number(value: f64) -> JsonValue {
    JsonValue::Number(json_number(value))
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod tests;

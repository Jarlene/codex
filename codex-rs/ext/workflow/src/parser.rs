use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

const NONDETERMINISM_ERROR: &str =
    "Workflow scripts must be deterministic: current time and random APIs are unavailable";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowMetaPhase {
    pub(crate) title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowMeta {
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "when_to_use")]
    pub(crate) when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phases: Option<Vec<WorkflowMetaPhase>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedWorkflow {
    pub(crate) meta: WorkflowMeta,
    pub(crate) body: String,
}

pub(crate) fn parse_workflow_script(script: &str) -> Result<ParsedWorkflow, String> {
    assert_deterministic(script)?;
    let mut probe = Cursor::new(script);
    probe.skip_ws_and_comments();
    if probe.source[probe.position..].starts_with("export") {
        return parse_legacy_export_meta_script(script);
    }
    parse_rust_workflow_script(script)
}

fn parse_rust_workflow_script(script: &str) -> Result<ParsedWorkflow, String> {
    let mut cursor = Cursor::new(script);
    cursor.skip_ws_and_comments();
    cursor.expect_keyword(
        "workflow",
        "workflow script must start with `workflow! { meta: { name, description } ... }`",
    )?;
    cursor.skip_ws_and_comments();
    cursor.expect_char(
        '!',
        "workflow script must start with `workflow! { meta: { name, description } ... }`",
    )?;
    cursor.skip_ws_and_comments();
    let open_brace = cursor.position();
    cursor.expect_char(
        '{',
        "workflow script must start with `workflow! { meta: { name, description } ... }`",
    )?;
    let close_brace = find_matching_brace(script, open_brace)?;
    cursor.skip_ws_and_comments();
    cursor.expect_keyword("meta", "workflow! body must start with `meta: { ... }`")?;
    cursor.skip_ws_and_comments();
    cursor.expect_char(':', "workflow meta must be written as `meta: { ... }`")?;
    cursor.skip_ws_and_comments();
    let meta_value = cursor.parse_literal("meta")?;
    cursor.skip_ws_and_comments();
    if matches!(cursor.peek_char(), Some(',') | Some(';')) {
        cursor.bump_char();
    }
    let body_start = cursor.position();
    let body = script[body_start..close_brace].trim().to_string();
    let mut tail = Cursor::new(&script[close_brace + 1..]);
    tail.skip_ws_and_comments();
    if tail.peek_char() == Some(';') {
        tail.bump_char();
        tail.skip_ws_and_comments();
    }
    if !tail.is_eof() {
        return Err("workflow script must not contain text after the workflow! block".to_string());
    }
    Ok(ParsedWorkflow {
        meta: validate_meta(meta_value)?,
        body,
    })
}

fn parse_legacy_export_meta_script(script: &str) -> Result<ParsedWorkflow, String> {
    let mut cursor = Cursor::new(script);
    cursor.skip_ws_and_comments();
    let export_start = cursor.position();
    cursor.expect_keyword(
        "export",
        "`export const meta = { name, description }` must be the first statement in the script",
    )?;
    cursor.skip_ws_and_comments();
    cursor.expect_keyword("const", "meta export must be `export const meta = ...`")?;
    cursor.skip_ws_and_comments();
    cursor.expect_keyword("meta", "meta export must declare `meta`")?;
    cursor.skip_ws_and_comments();
    cursor.expect_char('=', "meta must have a literal value")?;
    cursor.skip_ws_and_comments();
    let meta_value = cursor.parse_literal("meta")?;
    cursor.skip_ws_and_comments();
    if cursor.peek_char() == Some(';') {
        cursor.bump_char();
    }
    let export_end = cursor.position();
    let meta = validate_meta(meta_value)?;
    let mut body = String::new();
    body.push_str(&script[..export_start]);
    body.push_str(&script[export_end..]);
    Ok(ParsedWorkflow { meta, body })
}

fn find_matching_brace(source: &str, open_position: usize) -> Result<usize, String> {
    let mut cursor = Cursor {
        source,
        position: open_position,
    };
    cursor.expect_char('{', "workflow block must start with `{`")?;
    let mut depth = 1_usize;
    while let Some(ch) = cursor.peek_char() {
        if ch == '\'' || ch == '"' {
            cursor.parse_string()?;
            continue;
        }
        if ch == '`' {
            cursor.parse_template_string("workflow")?;
            continue;
        }
        if source[cursor.position..].starts_with("//") {
            while let Some(ch) = cursor.bump_char() {
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        if source[cursor.position..].starts_with("/*") {
            cursor.position += 2;
            while !cursor.is_eof() && !source[cursor.position..].starts_with("*/") {
                cursor.bump_char();
            }
            if source[cursor.position..].starts_with("*/") {
                cursor.position += 2;
            }
            continue;
        }
        let position = cursor.position();
        cursor.bump_char();
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(position);
                }
            }
            _ => {}
        }
    }
    Err("unterminated workflow! block".to_string())
}

fn validate_meta(value: JsonValue) -> Result<WorkflowMeta, String> {
    let JsonValue::Object(object) = &value else {
        return Err("meta must be an object".to_string());
    };
    match object.get("name") {
        Some(JsonValue::String(name)) if !name.trim().is_empty() => {}
        _ => return Err("meta.name must be a non-empty string".to_string()),
    }
    match object.get("description") {
        Some(JsonValue::String(description)) if !description.trim().is_empty() => {}
        _ => return Err("meta.description must be a non-empty string".to_string()),
    }
    let meta: WorkflowMeta =
        serde_json::from_value(value).map_err(|err| format!("meta must be an object: {err}"))?;
    if let Some(phases) = &meta.phases {
        for phase in phases {
            if phase.title.trim().is_empty() {
                return Err("each meta phase must have a title string".to_string());
            }
        }
    }
    Ok(meta)
}

fn assert_deterministic(script: &str) -> Result<(), String> {
    let tokens = deterministic_tokens(script)?;
    for (index, token) in tokens.iter().enumerate() {
        if (token == "new" && is_new_date_expression(&tokens, index + 1)) || token == "thread_rng" {
            return Err(NONDETERMINISM_ERROR.to_string());
        }
        if (token == "rand" || token == "random") && next_non_optional_call(&tokens, index + 1) {
            return Err(NONDETERMINISM_ERROR.to_string());
        }
    }
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "Date" || token == "Math" {
            let Some((property, next_index)) = member_property(&tokens, index + 1) else {
                index += 1;
                continue;
            };
            if ((token == "Date" && property == "now") || (token == "Math" && property == "random"))
                && next_non_optional_call(&tokens, next_index)
            {
                return Err(NONDETERMINISM_ERROR.to_string());
            }
        }
        if (token == "SystemTime" || token == "Instant")
            && associated_property(&tokens, index + 1).as_deref() == Some("now")
        {
            return Err(NONDETERMINISM_ERROR.to_string());
        }
        index += 1;
    }
    Ok(())
}

fn is_new_date_expression(tokens: &[String], mut index: usize) -> bool {
    while tokens.get(index).is_some_and(|token| token == "(") {
        index += 1;
    }
    tokens.get(index).is_some_and(|token| token == "Date")
}

fn member_property(tokens: &[String], mut index: usize) -> Option<(String, usize)> {
    if tokens.get(index).is_some_and(|token| token == "?") {
        index += 1;
    }
    match tokens.get(index).map(String::as_str) {
        Some(".") => {
            index += 1;
            if tokens.get(index).is_some_and(|token| token == "?") {
                index += 1;
            }
            let property = tokens.get(index)?.clone();
            Some((property, index + 1))
        }
        Some("[") => {
            let (property, next_index) = static_string_from_tokens(tokens, index + 1)?;
            if tokens.get(next_index).is_some_and(|token| token == "]") {
                Some((property, next_index + 1))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn associated_property(tokens: &[String], index: usize) -> Option<String> {
    if tokens.get(index).is_some_and(|token| token == ":")
        && tokens.get(index + 1).is_some_and(|token| token == ":")
    {
        return tokens.get(index + 2).cloned();
    }
    None
}

fn static_string_from_tokens(tokens: &[String], index: usize) -> Option<(String, usize)> {
    let (mut value, mut index) = match tokens.get(index) {
        Some(token) if quoted_token_value(token).is_some() => {
            (quoted_token_value(token).unwrap_or_default(), index + 1)
        }
        _ => return None,
    };
    while tokens.get(index).is_some_and(|token| token == "+") {
        let next = tokens.get(index + 1)?;
        value.push_str(&quoted_token_value(next)?);
        index += 2;
    }
    Some((value, index))
}

fn quoted_token_value(token: &str) -> Option<String> {
    if token.len() >= 2 && token.starts_with('\'') && token.ends_with('\'') {
        return Some(token[1..token.len() - 1].to_string());
    }
    if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
        return Some(token[1..token.len() - 1].to_string());
    }
    if token.len() >= 2 && token.starts_with('`') && token.ends_with('`') {
        return Some(token[1..token.len() - 1].to_string());
    }
    None
}

fn next_non_optional_call(tokens: &[String], mut index: usize) -> bool {
    if tokens.get(index).is_some_and(|token| token == "?") {
        index += 1;
    }
    if tokens.get(index).is_some_and(|token| token == "?.") {
        index += 1;
    }
    if tokens.get(index).is_some_and(|token| token == ".")
        && tokens.get(index + 1).is_some_and(|token| token == "?")
    {
        index += 2;
    }
    if tokens.get(index).is_some_and(|token| token == ".")
        && tokens.get(index + 1).is_some_and(|token| token == "(")
    {
        index += 1;
    }
    tokens.get(index).is_some_and(|token| token == "(")
}

fn deterministic_tokens(script: &str) -> Result<Vec<String>, String> {
    let mut cursor = Cursor::new(script);
    let mut tokens = Vec::new();
    while !cursor.is_eof() {
        cursor.skip_ws_and_comments();
        let Some(ch) = cursor.peek_char() else {
            break;
        };
        if is_identifier_start(ch) {
            tokens.push(cursor.read_identifier());
            continue;
        }
        if ch == '\'' || ch == '"' {
            let value = cursor.read_string_literal_token()?;
            tokens.push(value);
            continue;
        }
        if ch == '`' {
            if cursor.template_has_interpolation()? {
                tokens.extend(cursor.template_expression_tokens()?);
            } else {
                tokens.push(cursor.read_template_literal_token()?);
            }
            continue;
        }
        tokens.push(ch.to_string());
        cursor.bump_char();
    }
    Ok(tokens)
}

struct Cursor<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn position(&self) -> usize {
        self.position
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

    fn expect_keyword(&mut self, keyword: &str, error: &str) -> Result<(), String> {
        let start = self.position;
        if !self.source[start..].starts_with(keyword) {
            return Err(error.to_string());
        }
        let end = start + keyword.len();
        if self.source[end..]
            .chars()
            .next()
            .is_some_and(is_identifier_continue)
        {
            return Err(error.to_string());
        }
        self.position = end;
        Ok(())
    }

    fn expect_char(&mut self, expected: char, error: &str) -> Result<(), String> {
        match self.bump_char() {
            Some(ch) if ch == expected => Ok(()),
            _ => Err(error.to_string()),
        }
    }

    fn parse_literal(&mut self, path: &str) -> Result<JsonValue, String> {
        self.skip_ws_and_comments();
        let Some(ch) = self.peek_char() else {
            return Err(format!("non-literal node type in {path}: EOF"));
        };
        match ch {
            '{' => self.parse_object(path),
            '[' => self.parse_array(path),
            '\'' | '"' => self.parse_string().map(JsonValue::String),
            '`' => self.parse_template_string(path).map(JsonValue::String),
            '-' => self.parse_negative_number(path),
            '0'..='9' => self.parse_number(path),
            _ if is_identifier_start(ch) => {
                let identifier = self.read_identifier();
                match identifier.as_str() {
                    "true" => Ok(JsonValue::Bool(true)),
                    "false" => Ok(JsonValue::Bool(false)),
                    "null" => Ok(JsonValue::Null),
                    _ => Err(format!("non-literal node type in {path}: Identifier")),
                }
            }
            _ => Err(format!("non-literal node type in {path}: {ch}")),
        }
    }

    fn parse_object(&mut self, path: &str) -> Result<JsonValue, String> {
        self.expect_char('{', "meta must have a literal value")?;
        let mut map = serde_json::Map::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') {
                self.bump_char();
                break;
            }
            if self.source[self.position..].starts_with("...") {
                return Err(format!("spread not allowed in {path}"));
            }
            if self.peek_char() == Some('[') {
                return Err(format!("computed keys not allowed in {path}"));
            }
            let key = self.parse_property_key(path)?;
            self.skip_ws_and_comments();
            if self.peek_char() == Some('(') {
                return Err(format!("methods/accessors not allowed in {path}"));
            }
            if (key == "get" || key == "set") && self.peek_property_accessor_ahead() {
                return Err(format!("methods/accessors not allowed in {path}"));
            }
            self.expect_char(
                ':',
                &format!("non-literal node type in {path}.{key}: Identifier"),
            )?;
            if matches!(key.as_str(), "__proto__" | "constructor" | "prototype") {
                return Err(format!("reserved key name not allowed in {path}: {key}"));
            }
            let value = self.parse_literal(&format!("{path}.{key}"))?;
            map.insert(key, value);
            self.skip_ws_and_comments();
            match self.peek_char() {
                Some(',') => {
                    self.bump_char();
                }
                Some('}') => {}
                _ => return Err(format!("only plain properties allowed in {path}")),
            }
        }
        Ok(JsonValue::Object(map))
    }

    fn peek_property_accessor_ahead(&self) -> bool {
        let mut cursor = Cursor {
            source: self.source,
            position: self.position,
        };
        cursor.skip_ws_and_comments();
        cursor.read_identifier();
        cursor.skip_ws_and_comments();
        cursor.peek_char() == Some('(')
    }

    fn parse_property_key(&mut self, path: &str) -> Result<String, String> {
        self.skip_ws_and_comments();
        let Some(ch) = self.peek_char() else {
            return Err(format!("unsupported key type in {path}: EOF"));
        };
        if is_identifier_start(ch) {
            return Ok(self.read_identifier());
        }
        if ch == '\'' || ch == '"' {
            return self.parse_string();
        }
        if ch.is_ascii_digit() {
            let start = self.position;
            while self.peek_char().is_some_and(|ch| ch.is_ascii_digit()) {
                self.bump_char();
            }
            return Ok(self.source[start..self.position].to_string());
        }
        Err(format!("unsupported key type in {path}: {ch}"))
    }

    fn parse_array(&mut self, path: &str) -> Result<JsonValue, String> {
        self.expect_char('[', "array must start with [")?;
        let mut values = Vec::new();
        let mut expect_value = true;
        loop {
            self.skip_ws_and_comments();
            match self.peek_char() {
                Some(']') => {
                    self.bump_char();
                    break;
                }
                Some(',') if expect_value => {
                    return Err(format!("sparse arrays not allowed in {path}"));
                }
                Some(',') => {
                    self.bump_char();
                    expect_value = true;
                }
                Some(_) => {
                    if self.source[self.position..].starts_with("...") {
                        return Err(format!("spread not allowed in {path}"));
                    }
                    let index = values.len();
                    values.push(self.parse_literal(&format!("{path}[{index}]"))?);
                    expect_value = false;
                }
                None => return Err(format!("non-literal node type in {path}: EOF")),
            }
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_string(&mut self) -> Result<String, String> {
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

    fn parse_template_string(&mut self, path: &str) -> Result<String, String> {
        self.expect_char('`', "template literal must start with `")?;
        let mut value = String::new();
        while let Some(ch) = self.bump_char() {
            match ch {
                '`' => return Ok(value),
                '\\' => value.push(self.parse_escape()?),
                '$' if self.peek_char() == Some('{') => {
                    return Err(format!("template interpolation not allowed in {path}"));
                }
                _ => value.push(ch),
            }
        }
        Err("unterminated template literal".to_string())
    }

    fn parse_negative_number(&mut self, path: &str) -> Result<JsonValue, String> {
        self.expect_char('-', "negative number must start with -")?;
        let number = self.read_number_tail(path)?;
        Ok(JsonValue::Number(
            serde_json::Number::from_f64(-number)
                .ok_or_else(|| format!("invalid number in {path}"))?,
        ))
    }

    fn parse_number(&mut self, path: &str) -> Result<JsonValue, String> {
        let number = self.read_number_tail(path)?;
        Ok(JsonValue::Number(
            serde_json::Number::from_f64(number)
                .ok_or_else(|| format!("invalid number in {path}"))?,
        ))
    }

    fn read_number_tail(&mut self, path: &str) -> Result<f64, String> {
        let start = self.position;
        while self
            .peek_char()
            .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-'))
        {
            self.bump_char();
        }
        self.source[start..self.position]
            .parse::<f64>()
            .map_err(|_| format!("invalid number in {path}"))
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while self.peek_char().is_some_and(is_identifier_continue) {
            self.bump_char();
        }
        self.source[start..self.position].to_string()
    }

    fn read_string_literal_token(&mut self) -> Result<String, String> {
        let start = self.position;
        self.parse_string()?;
        Ok(self.source[start..self.position].to_string())
    }

    fn read_template_literal_token(&mut self) -> Result<String, String> {
        let start = self.position;
        self.parse_template_string("template")?;
        Ok(self.source[start..self.position].to_string())
    }

    fn template_has_interpolation(&self) -> Result<bool, String> {
        let mut cursor = Cursor {
            source: self.source,
            position: self.position,
        };
        cursor.expect_char('`', "template literal must start with `")?;
        while let Some(ch) = cursor.bump_char() {
            match ch {
                '`' => return Ok(false),
                '\\' => {
                    cursor.bump_char();
                }
                '$' if cursor.peek_char() == Some('{') => return Ok(true),
                _ => {}
            }
        }
        Err("unterminated template literal".to_string())
    }

    fn template_expression_tokens(&mut self) -> Result<Vec<String>, String> {
        self.expect_char('`', "template literal must start with `")?;
        let mut tokens = Vec::new();
        while let Some(ch) = self.bump_char() {
            match ch {
                '`' => return Ok(tokens),
                '\\' => {
                    self.bump_char();
                }
                '$' if self.peek_char() == Some('{') => {
                    self.bump_char();
                    let expression_start = self.position;
                    let expression_end = self.skip_balanced_expression()?;
                    tokens.extend(deterministic_tokens(
                        &self.source[expression_start..expression_end],
                    )?);
                }
                _ => {}
            }
        }
        Err("unterminated template literal".to_string())
    }

    fn skip_balanced_expression(&mut self) -> Result<usize, String> {
        let mut depth = 1_usize;
        while let Some(ch) = self.peek_char() {
            if ch == '\'' || ch == '"' {
                self.parse_string()?;
                continue;
            }
            if ch == '`' {
                self.template_expression_tokens()?;
                continue;
            }
            self.bump_char();
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(self.position - 1);
                    }
                }
                _ => {}
            }
        }
        Err("unterminated template expression".to_string())
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;

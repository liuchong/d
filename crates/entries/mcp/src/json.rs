//! Pure-`std` RFC 8259 JSON parser, serializer, and `ConfigValue` converters.

use frame::{ConfigValue, PluginSchema};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }

    pub fn to_compact_string(&self) -> String {
        let mut out = String::new();
        write_json(self, &mut out);
        out
    }

    pub fn to_pretty_string(&self) -> String {
        let mut out = String::new();
        write_json_pretty(self, &mut out, 0);
        out
    }
}

pub fn json_obj(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
    let mut map = BTreeMap::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v);
    }
    JsonValue::Object(map)
}

pub fn config_to_json(val: &ConfigValue) -> JsonValue {
    match val {
        ConfigValue::Null => JsonValue::Null,
        ConfigValue::Bool(b) => JsonValue::Bool(*b),
        ConfigValue::Int(n) => JsonValue::Int(*n),
        ConfigValue::Float(f) => JsonValue::Float(*f),
        ConfigValue::Str(s) => JsonValue::Str(s.clone()),
        ConfigValue::List(items) => {
            JsonValue::Array(items.iter().map(config_to_json).collect())
        }
        ConfigValue::Map(map) => JsonValue::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), config_to_json(v)))
                .collect(),
        ),
    }
}

pub fn config_to_json_redacted(
    val: &ConfigValue,
    schema: Option<&PluginSchema>,
) -> JsonValue {
    match (val, schema) {
        (ConfigValue::Map(map), Some(s)) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                let is_sensitive = s
                    .fields
                    .iter()
                    .find(|f| &f.name == k)
                    .is_some_and(|f| f.sensitive)
                    || is_sensitive_key(k);
                if is_sensitive && !matches!(v, ConfigValue::Null) {
                    out.insert(
                        k.clone(),
                        JsonValue::Str("***REDACTED***".to_string()),
                    );
                } else {
                    out.insert(k.clone(), config_to_json_redacted(v, None));
                }
            }
            JsonValue::Object(out)
        }
        (ConfigValue::Map(map), None) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if is_sensitive_key(k) && !matches!(v, ConfigValue::Null) {
                    out.insert(
                        k.clone(),
                        JsonValue::Str("***REDACTED***".to_string()),
                    );
                } else {
                    out.insert(k.clone(), config_to_json_redacted(v, None));
                }
            }
            JsonValue::Object(out)
        }
        _ => config_to_json(val),
    }
}

fn is_sensitive_key(k: &str) -> bool {
    let lower = k.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
}

pub fn json_to_config(val: &JsonValue) -> ConfigValue {
    match val {
        JsonValue::Null => ConfigValue::Null,
        JsonValue::Bool(b) => ConfigValue::Bool(*b),
        JsonValue::Int(n) => ConfigValue::Int(*n),
        JsonValue::Float(f) => ConfigValue::Float(*f),
        JsonValue::Str(s) => ConfigValue::Str(s.clone()),
        JsonValue::Array(items) => {
            ConfigValue::List(items.iter().map(json_to_config).collect())
        }
        JsonValue::Object(map) => ConfigValue::Map(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_config(v)))
                .collect(),
        ),
    }
}

pub fn parse(input: &str) -> Result<JsonValue, String> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let val = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing bytes at offset {}", p.pos));
    }
    Ok(val)
}

fn write_escaped_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_json(val: &JsonValue, out: &mut String) {
    match val {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(true) => out.push_str("true"),
        JsonValue::Bool(false) => out.push_str("false"),
        JsonValue::Int(n) => {
            let _ = write!(out, "{n}");
        }
        JsonValue::Float(f) => {
            if f.is_finite() {
                let _ = write!(out, "{f}");
            } else {
                out.push_str("null");
            }
        }
        JsonValue::Str(s) => write_escaped_str(s, out),
        JsonValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        JsonValue::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_escaped_str(k, out);
                out.push(':');
                write_json(v, out);
            }
            out.push('}');
        }
    }
}

fn write_json_pretty(val: &JsonValue, out: &mut String, indent: usize) {
    match val {
        JsonValue::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                push_indent(out, indent + 2);
                write_json_pretty(item, out, indent + 2);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push(']');
        }
        JsonValue::Object(map) if !map.is_empty() => {
            out.push_str("{\n");
            let len = map.len();
            for (i, (k, v)) in map.iter().enumerate() {
                push_indent(out, indent + 2);
                write_escaped_str(k, out);
                out.push_str(": ");
                write_json_pretty(v, out, indent + 2);
                if i + 1 < len {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push('}');
        }
        _ => write_json(val, out),
    }
}

fn push_indent(out: &mut String, spaces: usize) {
    for _ in 0..spaces {
        out.push(' ');
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), String> {
        match self.peek() {
            Some(b) if b == expected => {
                self.pos += 1;
                Ok(())
            }
            Some(b) => Err(format!(
                "expected '{}', found '{}' at offset {}",
                expected as char, b as char, self.pos
            )),
            None => Err(format!(
                "expected '{}', reached end of input",
                expected as char
            )),
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        match self.peek() {
            Some(b'n') => {
                self.expect_literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.expect_literal(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => Ok(JsonValue::Str(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(b) => Err(format!(
                "unexpected byte '{}' at offset {}",
                b as char, self.pos
            )),
            None => Err("unexpected end of JSON input".to_string()),
        }
    }

    fn expect_literal(&mut self, lit: &[u8]) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err(format!("invalid literal at offset {}", self.pos))
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        match self.bytes.get(self.pos).copied() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(format!("invalid number at offset {start}")),
        }
        let mut is_float = false;
        if self.bytes.get(self.pos) == Some(&b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                return Err(format!("invalid fraction at offset {}", self.pos));
            }
            while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                return Err(format!("invalid exponent at offset {}", self.pos));
            }
            while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|e| e.to_string())?;
        if is_float {
            let f: f64 = raw
                .parse()
                .map_err(|e| format!("invalid float `{raw}`: {e}"))?;
            Ok(JsonValue::Float(f))
        } else {
            let n: i64 = raw
                .parse()
                .map_err(|e| format!("integer overflow `{raw}`: {e}"))?;
            Ok(JsonValue::Int(n))
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err("unterminated string".to_string());
            };
            self.pos += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(&esc) = self.bytes.get(self.pos) else {
                        return Err("unterminated escape sequence".to_string());
                    };
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if !self.bytes[self.pos..].starts_with(b"\\u") {
                                    return Err(
                                        "missing low surrogate".to_string()
                                    );
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(
                                        "invalid low surrogate".to_string()
                                    );
                                }
                                let cp = 0x10000
                                    + (((code as u32 - 0xD800) << 10)
                                        | (low as u32 - 0xDC00));
                                let ch = char::from_u32(cp)
                                    .ok_or("invalid surrogate pair")?;
                                out.push(ch);
                            } else {
                                let ch = char::from_u32(code as u32)
                                    .ok_or("invalid unicode escape")?;
                                out.push(ch);
                            }
                        }
                        other => {
                            return Err(format!(
                                "invalid escape '\\{}'",
                                other as char
                            ));
                        }
                    }
                }
                0x00..=0x1F => {
                    return Err(
                        "unescaped control character in string".to_string()
                    );
                }
                _ => {
                    let start = self.pos - 1;
                    while matches!(self.bytes.get(self.pos), Some(&x) if (x & 0xC0) == 0x80)
                    {
                        self.pos += 1;
                    }
                    let slice =
                        std::str::from_utf8(&self.bytes[start..self.pos])
                            .map_err(|e| e.to_string())?;
                    out.push_str(slice);
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let slice = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or("truncated \\uXXXX escape")?;
        self.pos += 4;
        let s = std::str::from_utf8(slice).map_err(|e| e.to_string())?;
        u16::from_str_radix(s, 16)
            .map_err(|_| format!("invalid hex escape '{s}'"))
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect_byte(b'[')?;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => {
                    return Err(format!(
                        "expected ',' or ']' in array at offset {}",
                        self.pos
                    ));
                }
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect_byte(b'{')?;
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            let key = self.parse_string()?;
            self.expect_byte(b':')?;
            let val = self.parse_value()?;
            map.insert(key, val);
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(map));
                }
                _ => {
                    return Err(format!(
                        "expected ',' or '}}' in object at offset {}",
                        self.pos
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_json_and_config_values_and_redacts_secrets() {
        let raw = r#"{"ok":true,"n":42,"s":"a\nb","nested":{"k":[1,null]}}"#;
        let parsed = parse(raw).unwrap();
        let cfg = json_to_config(&parsed);
        assert_eq!(config_to_json(&cfg), parsed);
        let serialized = parsed.to_compact_string();
        assert_eq!(parse(&serialized).unwrap(), parsed);

        let schema = PluginSchema::new("test").field(
            frame::ConfigFieldSchema::new(
                "custom_secret",
                frame::ConfigFieldType::Str,
                "secret",
            )
            .sensitive(),
        );
        let cfg_secret = ConfigValue::Map(BTreeMap::from([
            (
                "custom_secret".into(),
                ConfigValue::Str("raw-secret".into()),
            ),
            ("public".into(), ConfigValue::Bool(true)),
        ]));
        let redacted = config_to_json_redacted(&cfg_secret, Some(&schema));
        assert_eq!(
            redacted.get("custom_secret").and_then(JsonValue::as_str),
            Some("***REDACTED***")
        );
        assert_eq!(
            redacted.get("public").and_then(JsonValue::as_bool),
            Some(true)
        );
    }
}

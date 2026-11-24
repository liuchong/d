//! Minimal strict RFC 8259 JSON parser implemented purely with `std`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
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
}

pub fn parse(input: &str) -> Result<JsonValue, String> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let val = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("trailing garbage at byte offset {}", parser.pos));
    }
    Ok(val)
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
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                return Err(format!("invalid fraction at offset {}", self.pos));
            }
            while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
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
        let s = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|e| e.to_string())?
            .to_string();
        Ok(JsonValue::Number(s))
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
                    // Copy full UTF-8 scalar cleanly.
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
    fn parses_nested_cargo_like_json_and_escapes() {
        let raw = r#"{
            "ok": true,
            "none": null,
            "count": -12.5e2,
            "text": "line1\n\"quoted\" \u4E2D\u6587 \uD83D\uDE00",
            "items": [1, false, {"k": "v"}]
        }"#;
        let v = parse(raw).unwrap();
        assert_eq!(v.get("ok"), Some(&JsonValue::Bool(true)));
        assert_eq!(v.get("none"), Some(&JsonValue::Null));
        assert_eq!(v.get("count"), Some(&JsonValue::Number("-12.5e2".into())));
        assert_eq!(
            v.get("text").and_then(JsonValue::as_str),
            Some("line1\n\"quoted\" 中文 😀")
        );
        let arr = v.get("items").and_then(JsonValue::as_array).unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[2].get("k").and_then(JsonValue::as_str), Some("v"));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse("{]").is_err());
        assert!(parse("\"unterminated").is_err());
        assert!(parse("[1, 2] trailing").is_err());
    }
}

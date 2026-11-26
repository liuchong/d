//! Zero-dependency pure-`std` RFC 8259 JSON parser converting declarative
//! manifest files into `Vec<frame::PluginEntry>`.

use frame::{
    ConfigValue, EntryTarget, ErrorKind, FrameError, LoaderService,
    PluginEntry, ServiceKey,
};
use std::collections::BTreeMap;
use std::path::Path;

fn parse_isolate_key(name: &str) -> Result<ServiceKey, String> {
    match name {
        "http.router" | "http-api" => {
            Ok(ServiceKey::of::<http_api::RouterService>())
        }
        "http.bound_info" => Ok(ServiceKey::of::<http_api::BoundServerInfo>()),
        "net.transport" | "net-api" => {
            Ok(ServiceKey::of::<net_api::TransportService>())
        }
        "files.viewer_registry" | "files-api" => {
            Ok(ServiceKey::of::<files_api::ViewerRegistryService>())
        }
        "render.service" | "render-api" => {
            Ok(ServiceKey::of::<render_api::RenderService>())
        }
        "frame.loader" => Ok(ServiceKey::of::<LoaderService>()),
        other => Err(format!("unknown isolate service key `{other}`")),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum JsonVal {
    Null,
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(BTreeMap<String, JsonVal>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn parse_val(&mut self) -> Result<JsonVal, String> {
        let Some(ch) = self.peek() else {
            return Err("unexpected end of JSON input".to_string());
        };
        match ch {
            b'n' => {
                self.expect_lit(b"null")?;
                Ok(JsonVal::Null)
            }
            b't' => {
                self.expect_lit(b"true")?;
                Ok(JsonVal::Bool(true))
            }
            b'f' => {
                self.expect_lit(b"false")?;
                Ok(JsonVal::Bool(false))
            }
            b'"' => Ok(JsonVal::Str(self.parse_str()?)),
            b'[' => self.parse_arr(),
            b'{' => self.parse_obj(),
            b'-' | b'0'..=b'9' => self.parse_int(),
            _ => Err(format!(
                "unexpected token `{}` at byte {}",
                ch as char, self.pos
            )),
        }
    }

    fn expect_lit(&mut self, lit: &[u8]) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err(format!("invalid literal at byte {}", self.pos))
        }
    }

    fn parse_str(&mut self) -> Result<String, String> {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("expected string quote".to_string());
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(esc) = self.bytes.get(self.pos).copied() else {
                        return Err("unterminated escape".to_string());
                    };
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return Err("truncated \\u escape".to_string());
                            }
                            let hex = std::str::from_utf8(
                                &self.bytes[self.pos..self.pos + 4],
                            )
                            .map_err(|_| {
                                "bad utf8 in \\u escape".to_string()
                            })?;
                            let code =
                                u32::from_str_radix(hex, 16).map_err(|_| {
                                    "invalid hex in \\u".to_string()
                                })?;
                            self.pos += 4;
                            let ch = char::from_u32(code)
                                .ok_or("invalid unicode scalar")?;
                            out.push(ch);
                        }
                        other => {
                            return Err(format!(
                                "invalid escape \\{}",
                                other as char
                            ));
                        }
                    }
                }
                c if c < 0x20 => {
                    return Err("unescaped control character in JSON string"
                        .to_string());
                }
                _ => {
                    self.pos -= 1;
                    let s = std::str::from_utf8(&self.bytes[self.pos..])
                        .map_err(|_| "invalid UTF-8 in JSON".to_string())?;
                    let ch = s.chars().next().ok_or("empty utf8 char")?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        Err("unterminated JSON string".to_string())
    }

    fn parse_int(&mut self) -> Result<JsonVal, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.pos < self.bytes.len()
            && self.bytes[self.pos].is_ascii_digit()
        {
            self.pos += 1;
        }
        if self.pos == digits_start {
            return Err("invalid JSON number".to_string());
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "bad number utf8".to_string())?;
        let n: i64 = raw
            .parse()
            .map_err(|e| format!("integer `{raw}` out of range: {e}"))?;
        Ok(JsonVal::Num(n))
    }

    fn parse_arr(&mut self) -> Result<JsonVal, String> {
        self.pos += 1;
        let mut arr = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonVal::Arr(arr));
        }
        loop {
            arr.push(self.parse_val()?);
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonVal::Arr(arr));
                }
                _ => return Err("expected `,` or `]` in array".to_string()),
            }
        }
    }

    fn parse_obj(&mut self) -> Result<JsonVal, String> {
        self.pos += 1;
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonVal::Obj(map));
        }
        loop {
            self.skip_ws();
            let k = self.parse_str()?;
            if self.peek() != Some(b':') {
                return Err("expected `:` after object key".to_string());
            }
            self.pos += 1;
            let v = self.parse_val()?;
            map.insert(k, v);
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonVal::Obj(map));
                }
                _ => return Err("expected `,` or `}` in object".to_string()),
            }
        }
    }
}

fn to_config_value(v: JsonVal) -> ConfigValue {
    match v {
        JsonVal::Null => ConfigValue::Null,
        JsonVal::Bool(b) => ConfigValue::Bool(b),
        JsonVal::Num(n) => ConfigValue::Int(n),
        JsonVal::Str(s) => ConfigValue::Str(s),
        JsonVal::Arr(a) => {
            ConfigValue::List(a.into_iter().map(to_config_value).collect())
        }
        JsonVal::Obj(o) => ConfigValue::Map(
            o.into_iter()
                .map(|(k, v)| (k, to_config_value(v)))
                .collect(),
        ),
    }
}

fn parse_entry(raw: JsonVal) -> Result<PluginEntry, String> {
    let JsonVal::Obj(mut map) = raw else {
        return Err("plugin entry must be a JSON object".to_string());
    };
    let Some(JsonVal::Str(id)) = map.remove("id") else {
        return Err("plugin entry requires string `id`".to_string());
    };
    let target = match map.remove("target") {
        None => EntryTarget::Name(id.clone()),
        Some(JsonVal::Str(name)) => EntryTarget::Name(name),
        Some(JsonVal::Obj(mut tobj)) => {
            let kind = tobj.remove("kind").and_then(|k| match k {
                JsonVal::Str(s) => Some(s),
                _ => None,
            });
            if kind.as_deref() == Some("group")
                || matches!(tobj.get("group"), Some(JsonVal::Bool(true)))
            {
                EntryTarget::Group
            } else if let Some(JsonVal::Str(name)) = tobj.remove("name") {
                EntryTarget::Name(name)
            } else {
                return Err(format!("invalid `target` on plugin `{id}`"));
            }
        }
        _ => return Err(format!("invalid `target` type on plugin `{id}`")),
    };
    let config = map
        .remove("config")
        .map_or(ConfigValue::Null, to_config_value);
    let disabled = match map.remove("disabled") {
        None | Some(JsonVal::Null) => false,
        Some(JsonVal::Bool(b)) => b,
        _ => return Err(format!("`disabled` on `{id}` must be boolean")),
    };
    let isolate = match map.remove("isolate") {
        None | Some(JsonVal::Null) => Vec::new(),
        Some(JsonVal::Arr(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let JsonVal::Str(s) = item else {
                    return Err(format!(
                        "`isolate` items on `{id}` must be strings"
                    ));
                };
                out.push(parse_isolate_key(&s)?);
            }
            out
        }
        _ => return Err(format!("`isolate` on `{id}` must be string array")),
    };
    let children = match map.remove("children") {
        None | Some(JsonVal::Null) => Vec::new(),
        Some(JsonVal::Arr(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for child in arr {
                out.push(parse_entry(child)?);
            }
            out
        }
        _ => return Err(format!("`children` on `{id}` must be entry array")),
    };

    Ok(PluginEntry {
        id,
        target,
        config,
        disabled,
        isolate,
        children,
    })
}

/// Parse a declarative JSON manifest (`[entry, ...]` or `{"plugins": [entry, ...]}`)
/// into [`PluginEntry`] records for [`frame::Loader`].
pub fn parse_entries_from_json(
    raw: &str,
) -> Result<Vec<PluginEntry>, FrameError> {
    let mut parser = Parser::new(raw);
    let root = parser.parse_val().map_err(|e| {
        FrameError::new(
            ErrorKind::ValidationFailed,
            format!("invalid config JSON: {e}"),
        )
    })?;
    if parser.peek().is_some() {
        return Err(FrameError::new(
            ErrorKind::ValidationFailed,
            "trailing characters after JSON manifest",
        ));
    }
    let arr = match root {
        JsonVal::Arr(arr) => arr,
        JsonVal::Obj(mut obj) => match obj.remove("plugins") {
            Some(JsonVal::Arr(arr)) => arr,
            _ => {
                return Err(FrameError::new(
                    ErrorKind::ValidationFailed,
                    "manifest object must contain array field `plugins`",
                ));
            }
        },
        _ => {
            return Err(FrameError::new(
                ErrorKind::ValidationFailed,
                "config manifest root must be JSON array or `{plugins: [...]}`",
            ));
        }
    };
    arr.into_iter()
        .map(parse_entry)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| FrameError::new(ErrorKind::ValidationFailed, e))
}

/// Read and parse a declarative JSON plugin manifest file from disk.
pub fn load_entries_from_json_file(
    path: &Path,
) -> Result<Vec<PluginEntry>, FrameError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        FrameError::new(
            ErrorKind::ValidationFailed,
            format!("failed to read config file `{}`: {e}", path.display()),
        )
    })?;
    parse_entries_from_json(&raw)
}

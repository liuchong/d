//! Pure-std URL percent decoding for request paths.

/// Error returned when a URL path contains malformed percent-escapes or invalid UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodePathError;

/// Percent-decode an HTTP request path ensuring a leading `/` and valid UTF-8.
pub fn decode_path(raw: &str) -> Result<String, DecodePathError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 1);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(DecodePathError);
            }
            let hi = hex_val(bytes[i + 1]).ok_or(DecodePathError)?;
            let lo = hex_val(bytes[i + 2]).ok_or(DecodePathError)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| DecodePathError)?;
    if decoded.starts_with('/') {
        Ok(decoded)
    } else {
        Ok(format!("/{decoded}"))
    }
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_and_rejects_bad_escapes() {
        assert_eq!(
            decode_path("/%E4%B8%AD%E6%96%87.txt").unwrap(),
            "/中文.txt"
        );
        assert_eq!(decode_path("plain/path").unwrap(), "/plain/path");
        assert!(decode_path("/bad%2").is_err());
        assert!(decode_path("/%ZZ").is_err());
    }
}

//! Pure-Rust RFC 6455 HTTP Upgrade validation, SHA-1 (FIPS 180-4), and Base64.

use http_api::{Headers, HttpRequest, HttpResponse, Method, StatusCode};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Compute `Sec-WebSocket-Accept` per RFC 6455 Section 4.2.2.
pub fn compute_accept_key(client_key: &str) -> String {
    let mut input = Vec::with_capacity(client_key.len() + WS_GUID.len());
    input.extend_from_slice(client_key.trim().as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());
    let hash = sha1_digest(&input);
    base64_encode(&hash)
}

/// Validate an inbound HTTP WebSocket upgrade request and return the computed
/// `Sec-WebSocket-Accept` value or a reject HTTP response.
pub fn validate_upgrade_request(
    req: &HttpRequest,
) -> Result<String, HttpResponse> {
    if req.method != Method::Get {
        return Err(HttpResponse::text(
            StatusCode::METHOD_NOT_ALLOWED,
            "WebSocket upgrade requires GET",
        ));
    }

    let has_ws_upgrade = req
        .headers
        .get_all("upgrade")
        .flat_map(|v| v.split(','))
        .any(|tok| tok.trim().eq_ignore_ascii_case("websocket"));
    if !has_ws_upgrade {
        return Err(HttpResponse::text(
            StatusCode::BAD_REQUEST,
            "Missing or invalid Upgrade: websocket header",
        ));
    }

    let has_conn_upgrade = req
        .headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"));
    if !has_conn_upgrade {
        return Err(HttpResponse::text(
            StatusCode::BAD_REQUEST,
            "Missing Connection: Upgrade header",
        ));
    }

    let Some(ver) = req.headers.get("sec-websocket-version") else {
        return Err(HttpResponse::text(
            StatusCode::BAD_REQUEST,
            "Missing Sec-WebSocket-Version",
        ));
    };
    if ver.trim() != "13" {
        return Err(HttpResponse::text(
            StatusCode::UPGRADE_REQUIRED,
            "Unsupported WebSocket version; version 13 required",
        )
        .with_header("Sec-WebSocket-Version", "13"));
    }

    let Some(key) = req.headers.get("sec-websocket-key") else {
        return Err(HttpResponse::text(
            StatusCode::BAD_REQUEST,
            "Missing Sec-WebSocket-Key",
        ));
    };
    let trimmed = key.trim();
    if trimmed.len() != 24 || !trimmed.ends_with("==") || !is_base64(trimmed) {
        return Err(HttpResponse::text(
            StatusCode::BAD_REQUEST,
            "Invalid Sec-WebSocket-Key",
        ));
    }

    Ok(compute_accept_key(trimmed))
}

/// Build the `101 Switching Protocols` response headers for an accepted handshake.
pub fn switching_protocols_headers(accept_key: &str) -> Headers {
    let mut headers = Headers::with_capacity(4);
    headers.insert("Upgrade", "websocket");
    headers.insert("Connection", "Upgrade");
    headers.insert("Sec-WebSocket-Accept", accept_key);
    headers
}

fn is_base64(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes[..bytes.len().saturating_sub(2)]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
}

/// Pure-`std` FIPS 180-4 SHA-1 digest.
pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xefcd_ab89;
    let mut h2: u32 = 0x98ba_dcfe;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xc3d2_e1f0;

    let (blocks, _) = msg.as_chunks::<64>();
    for block in blocks {
        let mut w = [0u32; 80];
        let (words, _) = block.as_chunks::<4>();
        for (i, word) in words.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdcu32),
                _ => (b ^ c ^ d, 0xca62_c1d6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

/// Pure-`std` standard Base64 encoder with `=` padding.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6455_accept_key_vector_matches() {
        assert_eq!(
            compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}

//! Pure-Rust RFC 6455 binary frame codec, masking, and fragmentation validation.

use ws_api::{WsCloseFrame, WsMessage, WsOpcode};

/// One wire-level RFC 6455 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireFrame {
    pub fin: bool,
    pub opcode: WsOpcode,
    pub masked: bool,
    pub payload: Vec<u8>,
}

/// Decode failure mapped to RFC 6455 close code or incomplete-buffer indication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecodeError {
    Incomplete,
    ProtocolError(u16, &'static str),
    MessageTooLarge,
}

/// Attempt to decode one RFC 6455 frame from the start of `buf`, returning
/// `Ok((frame, consumed_bytes))` on success.
pub fn decode_frame(
    buf: &[u8],
    max_frame_bytes: usize,
) -> Result<(WireFrame, usize), FrameDecodeError> {
    if buf.len() < 2 {
        return Err(FrameDecodeError::Incomplete);
    }

    let b0 = buf[0];
    let b1 = buf[1];
    if (b0 & 0x70) != 0 {
        return Err(FrameDecodeError::ProtocolError(
            1002,
            "reserved RSV bits set",
        ));
    }
    let fin = (b0 & 0x80) != 0;
    let Some(opcode) = WsOpcode::from_u8(b0 & 0x0f) else {
        return Err(FrameDecodeError::ProtocolError(
            1002,
            "unknown WebSocket opcode",
        ));
    };

    let masked = (b1 & 0x80) != 0;
    let len_code = (b1 & 0x7f) as usize;
    let mut pos = 2usize;

    let payload_len: usize = match len_code {
        0..=125 => len_code,
        126 => {
            if buf.len() < pos + 2 {
                return Err(FrameDecodeError::Incomplete);
            }
            let n = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
            pos += 2;
            n
        }
        127 => {
            if buf.len() < pos + 8 {
                return Err(FrameDecodeError::Incomplete);
            }
            let raw = u64::from_be_bytes([
                buf[pos],
                buf[pos + 1],
                buf[pos + 2],
                buf[pos + 3],
                buf[pos + 4],
                buf[pos + 5],
                buf[pos + 6],
                buf[pos + 7],
            ]);
            pos += 8;
            if (raw >> 63) != 0 {
                return Err(FrameDecodeError::ProtocolError(
                    1002,
                    "invalid 64-bit frame length MSB",
                ));
            }
            usize::try_from(raw)
                .map_err(|_| FrameDecodeError::MessageTooLarge)?
        }
        _ => unreachable!(),
    };

    if opcode.is_control() && (!fin || payload_len > 125) {
        return Err(FrameDecodeError::ProtocolError(
            1002,
            "control frames cannot be fragmented or exceed 125 bytes",
        ));
    }
    if payload_len > max_frame_bytes {
        return Err(FrameDecodeError::MessageTooLarge);
    }

    let mask_key = if masked {
        if buf.len() < pos + 4 {
            return Err(FrameDecodeError::Incomplete);
        }
        let k = [buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]];
        pos += 4;
        Some(k)
    } else {
        None
    };

    let end = pos
        .checked_add(payload_len)
        .ok_or(FrameDecodeError::MessageTooLarge)?;
    if buf.len() < end {
        return Err(FrameDecodeError::Incomplete);
    }

    let mut payload = buf[pos..end].to_vec();
    if let Some(mask) = mask_key {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i & 3];
        }
    }

    Ok((
        WireFrame {
            fin,
            opcode,
            masked,
            payload,
        },
        end,
    ))
}

/// Encode a single RFC 6455 frame. Server frames pass `mask = None`.
pub fn encode_frame(
    fin: bool,
    opcode: WsOpcode,
    payload: &[u8],
    mask: Option<[u8; 4]>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    let b0 = (if fin { 0x80 } else { 0 }) | (opcode.as_u8() & 0x0f);
    out.push(b0);

    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    if payload.len() <= 125 {
        out.push(mask_bit | (payload.len() as u8));
    } else if payload.len() <= u16::MAX as usize {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }

    if let Some(m) = mask {
        out.extend_from_slice(&m);
        for (i, &b) in payload.iter().enumerate() {
            out.push(b ^ m[i & 3]);
        }
    } else {
        out.extend_from_slice(payload);
    }
    out
}

/// Encode a high-level [`WsMessage`] as an unmasked server frame.
pub fn encode_message(msg: &WsMessage) -> Vec<u8> {
    match msg {
        WsMessage::Text(t) => {
            encode_frame(true, WsOpcode::Text, t.as_bytes(), None)
        }
        WsMessage::Binary(b) => encode_frame(true, WsOpcode::Binary, b, None),
        WsMessage::Ping(p) => {
            let capped = &p[..p.len().min(125)];
            encode_frame(true, WsOpcode::Ping, capped, None)
        }
        WsMessage::Pong(p) => {
            let capped = &p[..p.len().min(125)];
            encode_frame(true, WsOpcode::Pong, capped, None)
        }
        WsMessage::Close(frame) => {
            let mut payload = Vec::new();
            if let Some(cf) = frame {
                payload.extend_from_slice(&cf.code.to_be_bytes());
                let reason = cf.reason.as_bytes();
                let max_reason = reason.len().min(123);
                payload.extend_from_slice(&reason[..max_reason]);
            }
            encode_frame(true, WsOpcode::Close, &payload, None)
        }
    }
}

/// Decode a Close frame payload per RFC 6455 Section 5.5.1.
pub fn parse_close_payload(
    payload: &[u8],
) -> Result<Option<WsCloseFrame>, FrameDecodeError> {
    if payload.is_empty() {
        return Ok(None);
    }
    if payload.len() == 1 {
        return Err(FrameDecodeError::ProtocolError(
            1002,
            "invalid 1-byte Close payload",
        ));
    }
    let code = u16::from_be_bytes([payload[0], payload[1]]);
    let reason = std::str::from_utf8(&payload[2..])
        .map_err(|_| {
            FrameDecodeError::ProtocolError(1007, "invalid UTF-8 close reason")
        })?
        .to_string();
    Ok(Some(WsCloseFrame { code, reason }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_masked_fragment_and_extended_length() {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let payload = vec![b'x'; 300];
        let wire = encode_frame(true, WsOpcode::Binary, &payload, Some(mask));
        let (decoded, used) = decode_frame(&wire, 1024).expect("decode");
        assert_eq!(used, wire.len());
        assert!(decoded.fin);
        assert!(decoded.masked);
        assert_eq!(decoded.opcode, WsOpcode::Binary);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn rejects_fragmented_control_frame() {
        let bad = [0x09u8, 0x80, 0, 0, 0, 0];
        let err = decode_frame(&bad, 1024).expect_err("fragmented ping");
        assert!(matches!(err, FrameDecodeError::ProtocolError(1002, _)));
    }
}

//! Stdio and Streamable HTTP transports for `d-mcp`.

use crate::tools::McpServer;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt,
    BufReader,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;

/// Constant-time comparison helper for Bearer authentication tokens.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Serve line-delimited JSON-RPC 2.0 requests from `reader` onto `writer`.
/// Invokes `on_eof` when `reader` closes or fails.
pub async fn serve_stdio<R, W, F>(
    server: McpServer,
    reader: R,
    mut writer: W,
    mut shutdown_rx: watch::Receiver<bool>,
    on_eof: F,
) where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce() + Send,
{
    let mut lines = reader.lines();
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
            next = lines.next_line() => {
                match next {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Some(reply) = server.handle_str(trimmed).await
                            && (writer.write_all(reply.as_bytes()).await.is_err()
                                || writer.write_all(b"\n").await.is_err()
                                || writer.flush().await.is_err())
                        {
                            break;
                        }
                    }
                    Ok(None) | Err(_) => {
                        on_eof();
                        break;
                    }
                }
            }
        }
    }
}

/// Bind a dedicated HTTP/1.1 Streamable HTTP JSON-RPC 2.0 listener for MCP.
pub async fn bind_http_listener(
    addr: SocketAddr,
) -> io::Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    Ok((listener, local))
}

/// Accept loop serving MCP JSON-RPC 2.0 `POST <path>` requests until `shutdown_rx` becomes true.
pub async fn serve_http_loop(
    server: McpServer,
    listener: TcpListener,
    path: String,
    auth_token: Option<Arc<String>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer)) => {
                        let srv = server.clone();
                        let ep = path.clone();
                        let tok = auth_token.clone();
                        tokio::spawn(async move {
                            let _ = handle_http_connection(stream, srv, &ep, tok.as_deref()).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn handle_http_connection(
    mut stream: TcpStream,
    server: McpServer,
    endpoint_path: &str,
    auth_token: Option<&String>,
) -> io::Result<()> {
    let mut reader = BufReader::new(&mut stream);
    let mut header_buf = Vec::with_capacity(1024);
    let header_end = loop {
        if header_buf.len() >= MAX_HTTP_HEADER_BYTES {
            let raw = reader.into_inner();
            write_http_reply(
                raw,
                431,
                "Request Header Fields Too Large",
                "application/json",
                br#"{"error":"headers too large"}"#,
                false,
            )
            .await?;
            return Ok(());
        }
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(());
        }
        let take = chunk.len().min(MAX_HTTP_HEADER_BYTES - header_buf.len());
        header_buf.extend_from_slice(&chunk[..take]);
        reader.consume(take);

        if let Some(pos) = find_double_crlf(&header_buf) {
            break pos;
        }
    };

    let leftover_start = header_end + 4;
    let leftover = &header_buf[leftover_start..];

    let header_str = match std::str::from_utf8(&header_buf[..header_end]) {
        Ok(s) => s,
        Err(_) => {
            let raw = reader.into_inner();
            write_http_reply(
                raw,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                b"invalid UTF-8 HTTP headers",
                false,
            )
            .await?;
            return Ok(());
        }
    };

    let mut lines = header_str.split("\r\n");
    let Some(req_line) = lines.next() else {
        return Ok(());
    };
    let mut parts = req_line.split_whitespace();
    let (Some(method), Some(target), Some(_ver)) =
        (parts.next(), parts.next(), parts.next())
    else {
        let raw = reader.into_inner();
        write_http_reply(
            raw,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"malformed request line",
            false,
        )
        .await?;
        return Ok(());
    };

    let req_path = target.split('?').next().unwrap_or(target);
    let mut content_length: Option<usize> = None;
    let mut auth_header: Option<String> = None;

    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key.eq_ignore_ascii_case("content-length") {
                match val.parse::<usize>() {
                    Ok(n) => content_length = Some(n),
                    Err(_) => {
                        let raw = reader.into_inner();
                        write_http_reply(
                            raw,
                            400,
                            "Bad Request",
                            "text/plain; charset=utf-8",
                            b"invalid Content-Length",
                            false,
                        )
                        .await?;
                        return Ok(());
                    }
                }
            } else if key.eq_ignore_ascii_case("authorization") {
                auth_header = Some(val.to_string());
            }
        }
    }

    if let Some(expected_tok) = auth_token {
        let authorized = auth_header
            .as_deref()
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
            })
            .is_some_and(|presented| {
                constant_time_eq(
                    presented.trim().as_bytes(),
                    expected_tok.as_bytes(),
                )
            });
        if !authorized {
            let raw = reader.into_inner();
            write_http_reply(
                raw,
                401,
                "Unauthorized",
                "application/json",
                br#"{"error":"unauthorized"}"#,
                true,
            )
            .await?;
            return Ok(());
        }
    }

    if req_path != endpoint_path {
        let raw = reader.into_inner();
        write_http_reply(
            raw,
            404,
            "Not Found",
            "application/json",
            br#"{"error":"not found"}"#,
            false,
        )
        .await?;
        return Ok(());
    }

    if method != "POST" {
        let raw = reader.into_inner();
        write_http_reply(
            raw,
            405,
            "Method Not Allowed",
            "application/json",
            br#"{"error":"POST required"}"#,
            false,
        )
        .await?;
        return Ok(());
    }

    let len = content_length.unwrap_or(0);
    if len > MAX_HTTP_BODY_BYTES {
        let raw = reader.into_inner();
        write_http_reply(
            raw,
            413,
            "Payload Too Large",
            "application/json",
            br#"{"error":"payload too large"}"#,
            false,
        )
        .await?;
        return Ok(());
    }

    let mut body = Vec::with_capacity(len);
    let take_left = len.min(leftover.len());
    body.extend_from_slice(&leftover[..take_left]);
    if body.len() < len {
        let mut rest = vec![0u8; len - body.len()];
        reader.read_exact(&mut rest).await?;
        body.extend_from_slice(&rest);
    }

    let raw_stream = reader.into_inner();
    let Ok(body_str) = std::str::from_utf8(&body) else {
        write_http_reply(
            raw_stream,
            400,
            "Bad Request",
            "application/json",
            br#"{"error":"body must be UTF-8 JSON"}"#,
            false,
        )
        .await?;
        return Ok(());
    };

    match server.handle_str(body_str).await {
        Some(json) => {
            write_http_reply(
                raw_stream,
                200,
                "OK",
                "application/json",
                json.as_bytes(),
                false,
            )
            .await?;
        }
        None => {
            write_http_reply(
                raw_stream,
                202,
                "Accepted",
                "application/json",
                b"",
                false,
            )
            .await?;
        }
    }
    Ok(())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_http_reply(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    www_authenticate: bool,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n",
        body.len()
    );
    if status == 405 {
        head.push_str("Allow: POST\r\n");
    }
    if www_authenticate {
        head.push_str("WWW-Authenticate: Bearer\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await
}

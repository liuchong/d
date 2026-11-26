//! Self-built HTTP/1.1 streaming response serializer.

use http_api::{BodyKind, HttpResponse, UpgradeHandler, Version};
use net_api::{BoxStreamIo, DrainSignal};
use std::fmt::Write as _;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of writing an HTTP response over a connection.
pub(crate) enum WriteOutcome {
    KeepAlive,
    Close,
    Upgraded(Arc<dyn UpgradeHandler>),
}

/// Write one HTTP/1.x response over `io`, honoring `HEAD`, fixed-length streams,
/// `streaming: true` immediate chunked flushes, and `101` upgrade handoffs.
pub(crate) async fn write_response(
    io: &mut BoxStreamIo,
    version: Version,
    is_head: bool,
    keep_alive: bool,
    mut resp: HttpResponse,
    drain: &Arc<dyn DrainSignal>,
) -> io::Result<WriteOutcome> {
    if !resp.headers.contains("date") {
        resp.headers
            .insert("Date", format_imf_fixdate(SystemTime::now()));
    }

    let mut body = std::mem::replace(&mut resp.body, BodyKind::Empty);

    if let BodyKind::Upgrade(handler) = body {
        drain.mark_long_lived();
        let mut head = String::with_capacity(512);
        let _ = write!(
            head,
            "{} {} {}\r\n",
            version.as_str(),
            resp.status.as_u16(),
            resp.status.reason_phrase()
        );
        for (k, v) in resp.headers.iter() {
            let _ = write!(head, "{k}: {v}\r\n");
        }
        head.push_str("\r\n");
        write_all_io(io, head.as_bytes()).await?;
        flush_io(io).await?;
        return Ok(WriteOutcome::Upgraded(handler));
    }

    let body_forbidden = is_head
        || resp.status.is_informational()
        || resp.status.as_u16() == 204
        || resp.status.as_u16() == 304;

    let mut force_close = !keep_alive || drain.is_draining();
    let mut use_chunked = false;

    if let BodyKind::Stream {
        streaming: true, ..
    } = &body
    {
        drain.mark_long_lived();
    }

    if body_forbidden {
        if let BodyKind::Bytes(ref b) = body
            && !resp.headers.contains("content-length")
            && resp.status.as_u16() != 204
            && resp.status.as_u16() != 304
        {
            resp.headers.insert("Content-Length", b.len().to_string());
        }
        body = BodyKind::Empty;
    } else {
        match &body {
            BodyKind::Empty | BodyKind::Upgrade(_) => {
                if !resp.headers.contains("content-length") {
                    resp.headers.insert("Content-Length", "0");
                }
            }
            BodyKind::Bytes(data) => {
                if !resp.headers.contains("content-length") {
                    resp.headers
                        .insert("Content-Length", data.len().to_string());
                }
            }
            BodyKind::Stream {
                content_length,
                streaming,
                ..
            } => {
                if *streaming || content_length.is_none() {
                    resp.headers.remove("content-length");
                    if version == Version::Http11 {
                        use_chunked = true;
                        resp.headers.insert("Transfer-Encoding", "chunked");
                    } else {
                        force_close = true;
                    }
                } else if let Some(len) = content_length
                    && !resp.headers.contains("content-length")
                {
                    resp.headers.insert("Content-Length", len.to_string());
                }
            }
        }
    }

    let final_keep_alive = !force_close;
    resp.headers.insert(
        "Connection",
        if final_keep_alive {
            "keep-alive"
        } else {
            "close"
        },
    );

    let mut head = String::with_capacity(512);
    let _ = write!(
        head,
        "{} {} {}\r\n",
        version.as_str(),
        resp.status.as_u16(),
        resp.status.reason_phrase()
    );
    for (k, v) in resp.headers.iter() {
        let _ = write!(head, "{k}: {v}\r\n");
    }
    head.push_str("\r\n");

    write_all_io(io, head.as_bytes()).await?;

    match body {
        BodyKind::Empty | BodyKind::Upgrade(_) => {
            flush_io(io).await?;
        }
        BodyKind::Bytes(data) => {
            if !data.is_empty() {
                write_all_io(io, &data).await?;
            }
            flush_io(io).await?;
        }
        BodyKind::Stream {
            mut reader,
            streaming,
            ..
        } => {
            if streaming {
                flush_io(io).await?;
            }
            loop {
                let next =
                    poll_fn(|cx| Pin::new(&mut *reader).poll_next(cx)).await;
                match next {
                    Some(Ok(chunk)) => {
                        if chunk.is_empty() {
                            continue;
                        }
                        if use_chunked {
                            let hdr = format!("{:x}\r\n", chunk.len());
                            write_all_io(io, hdr.as_bytes()).await?;
                            write_all_io(io, &chunk).await?;
                            write_all_io(io, b"\r\n").await?;
                        } else {
                            write_all_io(io, &chunk).await?;
                        }
                        if streaming {
                            flush_io(io).await?;
                        }
                    }
                    Some(Err(err)) => return Err(err),
                    None => {
                        if use_chunked {
                            write_all_io(io, b"0\r\n\r\n").await?;
                        }
                        flush_io(io).await?;
                        break;
                    }
                }
            }
        }
    }

    if final_keep_alive {
        Ok(WriteOutcome::KeepAlive)
    } else {
        Ok(WriteOutcome::Close)
    }
}

async fn write_all_io(io: &mut BoxStreamIo, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "zero-byte stream write",
            ));
        }
        off += n;
    }
    Ok(())
}

async fn flush_io(io: &mut BoxStreamIo) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await
}

/// Pure-std RFC 9110 IMF-fixdate formatter (`Sun, 06 Nov 1994 08:49:37 GMT`).
fn format_imf_fixdate(time: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
        "Nov", "Dec",
    ];
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let total_days = (secs / 86_400) as i64;
    let day_secs = secs % 86_400;
    let dow = DAYS[(total_days.rem_euclid(7)) as usize];
    let (year, month, day) = civil_from_days(total_days);
    let hour = day_secs / 3_600;
    let min = (day_secs % 3_600) / 60;
    let sec = day_secs % 60;
    let mon = MONTHS[(month - 1) as usize];
    format!("{dow}, {day:02} {mon} {year:04} {hour:02}:{min:02}:{sec:02} GMT")
}

const fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u8, d as u8)
}

//! Upstream TCP/TLS connection dialing, stream compat wrappers, and bounded idle pooling.

use net_api::{AsyncByteIo, BoxStreamIo};
use proxy_api::BackendConfig;
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tls_api::{TlsClientConfig, TlsConnectorService};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

const IDLE_TTL: Duration = Duration::from_secs(30);

/// Wraps a [`TcpStream`] into Layer 1 [`AsyncByteIo`].
pub(crate) struct TcpStreamIo(pub(crate) TcpStream);

impl AsyncByteIo for TcpStreamIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut rbuf = ReadBuf::new(buf);
        match Pin::new(&mut self.0).poll_read(cx, &mut rbuf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(rbuf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// Bridges [`BoxStreamIo`] to `tokio::io::AsyncRead + AsyncWrite` for bidirectional tunnels.
pub(crate) struct AsyncByteIoCompat(pub(crate) BoxStreamIo);

impl AsyncRead for AsyncByteIoCompat {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let slice = buf.initialize_unfilled();
        match Pin::new(&mut *self.0).poll_read(cx, slice) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for AsyncByteIoCompat {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

#[derive(Default)]
pub(crate) struct IdleBucket {
    conns: Mutex<VecDeque<(BoxStreamIo, Instant)>>,
}

impl IdleBucket {
    pub(crate) fn pop_fresh(&self) -> Option<BoxStreamIo> {
        let mut guard = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while let Some((io, ts)) = guard.pop_back() {
            if now.duration_since(ts) <= IDLE_TTL {
                return Some(io);
            }
        }
        None
    }

    pub(crate) fn push(&self, io: BoxStreamIo, cap: usize) {
        if cap == 0 {
            return;
        }
        let mut guard = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        while guard.len() >= cap {
            guard.pop_front();
        }
        guard.push_back((io, Instant::now()));
    }
}

pub(crate) async fn dial_backend(
    cfg: &BackendConfig,
    connect_timeout_ms: u64,
    tls_connector: Option<&TlsConnectorService>,
) -> io::Result<BoxStreamIo> {
    let timeout = Duration::from_millis(connect_timeout_ms.max(1));
    let stream = tokio::time::timeout(timeout, TcpStream::connect(cfg.addr))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("connect timeout dialing backend {}", cfg.addr),
            )
        })??;
    let _ = stream.set_nodelay(true);
    let raw: BoxStreamIo = Box::new(TcpStreamIo(stream));

    if cfg.tls {
        let Some(conn) = tls_connector else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "backend requires TLS but TlsConnectorService is not active",
            ));
        };
        let sni = cfg.sni.clone().unwrap_or_else(|| cfg.addr.ip().to_string());
        let tls_cfg = TlsClientConfig {
            server_name: sni,
            alpn_protocols: vec!["http/1.1".to_string()],
            ca_cert_pem: cfg.ca_cert_pem.clone(),
            insecure_skip_verify: cfg.insecure_skip_verify,
        };
        let negotiated =
            tokio::time::timeout(timeout, conn.wrap_outbound(&tls_cfg, raw))
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "TLS handshake timeout",
                    )
                })??;
        Ok(negotiated.stream)
    } else {
        Ok(raw)
    }
}

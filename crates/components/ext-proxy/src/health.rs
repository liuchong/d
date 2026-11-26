//! Active background health probe tasks managed as Frame effects.

use crate::lb::{BackendState, UpstreamState};
use crate::pool::dial_backend;
use frame::{Context, EffectHandle, FrameError};
use proxy_api::HealthCheckConfig;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tls_api::TlsConnectorService;
use tokio::sync::watch;

async fn probe_backend(
    backend: &BackendState,
    hc: &HealthCheckConfig,
    tls: Option<&TlsConnectorService>,
) -> bool {
    let Ok(mut io) = dial_backend(&backend.cfg, hc.timeout_ms, tls).await
    else {
        return false;
    };

    let Some(path) = &hc.path else {
        let _ = poll_fn(|cx| Pin::new(&mut *io).poll_shutdown(cx)).await;
        return true;
    };

    let host = backend
        .cfg
        .sni
        .clone()
        .unwrap_or_else(|| backend.cfg.addr.to_string());
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    let timeout = Duration::from_millis(hc.timeout_ms.max(1));
    let res = tokio::time::timeout(timeout, async {
        let bytes = req.as_bytes();
        let mut off = 0;
        while off < bytes.len() {
            let n =
                poll_fn(|cx| Pin::new(&mut *io).poll_write(cx, &bytes[off..]))
                    .await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "probe write zero",
                ));
            }
            off += n;
        }
        poll_fn(|cx| Pin::new(&mut *io).poll_flush(cx)).await?;

        let mut head = Vec::with_capacity(512);
        let mut tmp = [0u8; 512];
        loop {
            let n = poll_fn(|cx| Pin::new(&mut *io).poll_read(cx, &mut tmp))
                .await?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&tmp[..n]);
            if head.windows(2).any(|w| w == b"\r\n") || head.len() >= 1024 {
                break;
            }
        }
        let line = String::from_utf8_lossy(&head);
        let ok = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .is_some_and(|code| (200..400).contains(&code));
        Ok::<bool, std::io::Error>(ok)
    })
    .await;

    matches!(res, Ok(Ok(true)))
}

pub(crate) fn spawn_health_probe(
    caller: &Context,
    upstream: Arc<UpstreamState>,
    hc: HealthCheckConfig,
) -> Result<EffectHandle, FrameError> {
    let ctx_lookup = caller.clone();
    let label = format!("proxy-health:{}", upstream.cfg.id);
    caller.effect_sync(label, move |scope| {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let interval = Duration::from_millis(hc.interval_ms.max(10));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = stop_rx.wait_for(|&s| s) => break,
                    _ = tokio::time::sleep(interval) => {}
                }
                if *stop_rx.borrow() {
                    break;
                }
                let tls = ctx_lookup.get::<TlsConnectorService>();
                for b in &upstream.backends {
                    let ok = probe_backend(b, &hc, tls.as_deref()).await;
                    if ok {
                        b.consecutive_failures.store(0, Ordering::Release);
                        let succ = b
                            .consecutive_successes
                            .fetch_add(1, Ordering::AcqRel)
                            + 1;
                        if succ >= hc.healthy_threshold.max(1) {
                            *b.ejected_until
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = None;
                            b.healthy.store(true, Ordering::Release);
                        }
                    } else {
                        b.consecutive_successes.store(0, Ordering::Release);
                        let fails = b
                            .consecutive_failures
                            .fetch_add(1, Ordering::AcqRel)
                            + 1;
                        if fails >= hc.unhealthy_threshold.max(1) {
                            *b.ejected_until
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = None;
                            b.healthy.store(false, Ordering::Release);
                        }
                    }
                }
            }
        });
        scope.defer(move || {
            let _ = stop_tx.send(true);
        });
        Ok(())
    })
}

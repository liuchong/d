//! Zero-external-script Live Reload file watcher, WS hub, and inline HTML injector.

use frame::{BoxFuture, Context, EffectHandle, FrameError};
use http_api::{
    BodyKind, FilterOutcome, HttpFilter, HttpRequest, HttpResponse, RequestMeta,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio::sync::{broadcast, watch};
use ws_api::{WsHandler, WsMessage, WsSession};

/// Configuration for local zero-CDN Live Reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveReloadConfig {
    pub enabled: bool,
    pub watch_dirs: Vec<PathBuf>,
    pub interval_ms: u64,
    pub ws_path: String,
    pub inject_html: bool,
}

impl Default for LiveReloadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            watch_dirs: Vec::new(),
            interval_ms: 250,
            ws_path: "/_d/livereload".to_string(),
            inject_html: true,
        }
    }
}

pub(crate) struct LiveReloadHub {
    tx: broadcast::Sender<String>,
    pub(crate) trigger_count: std::sync::atomic::AtomicU64,
}

impl LiveReloadHub {
    pub(crate) fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(32);
        Arc::new(Self {
            tx,
            trigger_count: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub(crate) fn notify_reload(&self, reason: &str) {
        self.trigger_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self
            .tx
            .send(format!("{{\"event\":\"reload\",\"reason\":\"{reason}\"}}"));
    }
}

pub(crate) struct LiveReloadWsHandler {
    pub(crate) hub: Arc<LiveReloadHub>,
}

impl WsHandler for LiveReloadWsHandler {
    fn on_connect(&self, session: WsSession) -> BoxFuture<'static, ()> {
        let mut rx = self.hub.tx.subscribe();
        Box::pin(async move {
            loop {
                tokio::select! {
                    biased;
                    msg = rx.recv() => {
                        match msg {
                            Ok(payload) => {
                                if session.send(WsMessage::Text(payload)).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(_) => break,
                        }
                    }
                    incoming = session.recv() => {
                        match incoming {
                            Some(Ok(WsMessage::Close(_))) | None => break,
                            Some(Ok(_)) => {}
                            Some(Err(_)) => break,
                        }
                    }
                }
            }
        })
    }
}

pub(crate) struct HtmlReloadInjector {
    pub(crate) ws_path: String,
    pub(crate) skip_prefix: String,
}

impl HttpFilter for HtmlReloadInjector {
    fn name(&self) -> &str {
        "ext-dev-kit-livereload"
    }

    fn priority(&self) -> i32 {
        -200
    }

    fn on_request(
        &self,
        req: HttpRequest,
    ) -> BoxFuture<'static, FilterOutcome> {
        Box::pin(async move { FilterOutcome::Continue(req) })
    }

    fn on_response(
        &self,
        req_meta: &RequestMeta,
        mut resp: HttpResponse,
    ) -> BoxFuture<'static, HttpResponse> {
        if req_meta.path.starts_with(&self.skip_prefix) || req_meta.is_head {
            return Box::pin(async move { resp });
        }
        let is_html = resp
            .headers
            .get("content-type")
            .is_some_and(|ct| ct.to_ascii_lowercase().starts_with("text/html"));
        if !is_html {
            return Box::pin(async move { resp });
        }
        let snippet = format!(
            "<script>(function(){{var p=location.protocol==='https:'?'wss:':'ws:';var s=new WebSocket(p+'//'+location.host+'{}');s.onmessage=function(){{location.reload();}};}})();</script>",
            self.ws_path
        );
        match std::mem::replace(&mut resp.body, BodyKind::Empty) {
            BodyKind::Bytes(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let out = if let Some(idx) = text.rfind("</body>") {
                    let mut combined =
                        String::with_capacity(text.len() + snippet.len());
                    combined.push_str(&text[..idx]);
                    combined.push_str(&snippet);
                    combined.push_str(&text[idx..]);
                    combined.into_bytes()
                } else {
                    let mut combined = bytes;
                    combined.extend_from_slice(snippet.as_bytes());
                    combined
                };
                resp.headers.insert("Content-Length", out.len().to_string());
                resp.body = BodyKind::Bytes(out);
            }
            other => {
                resp.body = other;
            }
        }
        Box::pin(async move { resp })
    }
}

fn dir_fingerprint(dirs: &[PathBuf]) -> u64 {
    fn walk(dir: &Path, hasher: &mut DefaultHasher) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths = Vec::new();
        for entry in rd.flatten() {
            paths.push(entry.path());
        }
        paths.sort();
        for path in paths {
            path.hash(hasher);
            if let Ok(meta) = std::fs::metadata(&path) {
                meta.len().hash(hasher);
                if let Ok(mtime) = meta.modified()
                    && let Ok(dur) = mtime.duration_since(UNIX_EPOCH)
                {
                    dur.as_nanos().hash(hasher);
                }
                if meta.is_dir() {
                    walk(&path, hasher);
                }
            }
        }
    }

    let mut hasher = DefaultHasher::new();
    for d in dirs {
        walk(d, &mut hasher);
    }
    hasher.finish()
}

pub(crate) fn spawn_dir_watcher(
    caller: &Context,
    dirs: Vec<PathBuf>,
    interval_ms: u64,
    hub: Arc<LiveReloadHub>,
) -> Result<EffectHandle, FrameError> {
    let initial = Mutex::new(dir_fingerprint(&dirs));
    caller.effect_sync("dev-kit-watcher", move |scope| {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let interval = Duration::from_millis(interval_ms.max(25));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = stop_rx.wait_for(|&s| s) => break,
                    _ = tokio::time::sleep(interval) => {}
                }
                let dirs_clone = dirs.clone();
                let next_fp = tokio::task::spawn_blocking(move || {
                    dir_fingerprint(&dirs_clone)
                })
                .await
                .unwrap_or(0);
                let changed = {
                    let mut prev =
                        initial.lock().unwrap_or_else(|e| e.into_inner());
                    if *prev != next_fp {
                        *prev = next_fp;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    hub.notify_reload("file_changed");
                }
            }
        });
        scope.defer(move || {
            let _ = stop_tx.send(true);
        });
        Ok(())
    })
}

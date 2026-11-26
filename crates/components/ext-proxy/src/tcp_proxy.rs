//! Layer 4 TCP reverse proxy listener handler.

use crate::lb::UpstreamState;
use crate::pool::{AsyncByteIoCompat, dial_backend};
use frame::{BoxFuture, Context};
use net_api::{BoxStreamIo, ConnectionHandler, ConnectionMeta, DrainSignal};
use std::sync::Arc;
use tls_api::TlsConnectorService;

pub(crate) struct TcpProxyHandler {
    pub(crate) upstream: Arc<UpstreamState>,
    pub(crate) ctx: Context,
}

impl ConnectionHandler for TcpProxyHandler {
    fn handle(
        &self,
        client_io: BoxStreamIo,
        _meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()> {
        let upstream = Arc::clone(&self.upstream);
        let ctx = self.ctx.clone();
        Box::pin(async move {
            drain.mark_long_lived();
            let Some((_idx, backend, guard)) = upstream.select_backend(None)
            else {
                return;
            };
            let tls = ctx.get::<TlsConnectorService>();
            let Ok(upstream_io) = dial_backend(
                &backend.cfg,
                upstream.cfg.connect_timeout_ms,
                tls.as_deref(),
            )
            .await
            else {
                backend.record_passive_failure(
                    upstream.cfg.passive_eject_threshold,
                    upstream.cfg.passive_eject_ms,
                );
                return;
            };
            backend.record_success();

            let mut client = AsyncByteIoCompat(client_io);
            let mut target = AsyncByteIoCompat(upstream_io);
            let drain_fut = drain.wait();
            tokio::pin!(drain_fut);
            let _guard = guard;
            tokio::select! {
                biased;
                _ = &mut drain_fut => {}
                _ = tokio::io::copy_bidirectional(&mut client, &mut target) => {}
            }
        })
    }
}

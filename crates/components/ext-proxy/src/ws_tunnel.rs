//! Bidirectional WebSocket (`101 Switching Protocols`) upgrade tunnel.

use crate::lb::ActiveConnGuard;
use crate::pool::AsyncByteIoCompat;
use frame::BoxFuture;
use http_api::UpgradeHandler;
use net_api::{BoxStreamIo, DrainSignal};
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

pub(crate) struct UpstreamTunnelState {
    pub(crate) upstream_io: BoxStreamIo,
    pub(crate) upstream_leftover: Vec<u8>,
    pub(crate) guard: ActiveConnGuard,
}

pub(crate) struct WsTunnelHandler {
    pub(crate) state: Mutex<Option<UpstreamTunnelState>>,
}

async fn write_all_raw(
    io: &mut BoxStreamIo,
    data: &[u8],
) -> std::io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = poll_fn(|cx| Pin::new(&mut **io).poll_write(cx, &data[off..]))
            .await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "tunnel write zero",
            ));
        }
        off += n;
    }
    poll_fn(|cx| Pin::new(&mut **io).poll_flush(cx)).await
}

impl UpgradeHandler for WsTunnelHandler {
    fn on_upgrade(
        &self,
        mut client_io: BoxStreamIo,
        client_leftover: Vec<u8>,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()> {
        let taken = self.state.lock().unwrap_or_else(|e| e.into_inner()).take();
        Box::pin(async move {
            let Some(mut up) = taken else {
                return;
            };
            let _guard = up.guard;
            drain.mark_long_lived();

            if !up.upstream_leftover.is_empty()
                && write_all_raw(&mut client_io, &up.upstream_leftover)
                    .await
                    .is_err()
            {
                return;
            }
            if !client_leftover.is_empty()
                && write_all_raw(&mut up.upstream_io, &client_leftover)
                    .await
                    .is_err()
            {
                return;
            }

            let mut client = AsyncByteIoCompat(client_io);
            let mut upstream = AsyncByteIoCompat(up.upstream_io);
            let drain_fut = drain.wait();
            tokio::pin!(drain_fut);
            tokio::select! {
                biased;
                _ = &mut drain_fut => {}
                _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
            }
        })
    }
}

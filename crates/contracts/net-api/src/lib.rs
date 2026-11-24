//! Layer 1 transport contracts: byte streams, connection metadata, and
//! two-phase graceful drain signaling.

use frame::{
    BoxFuture, Context as FrameContext, EffectHandle, FrameError,
    ScopedService, Service,
};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Transport-agnostic asynchronous bidirectional byte stream.
pub trait AsyncByteIo: Send + Unpin + 'static {
    /// Poll for reading bytes into `buf`.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>>;

    /// Poll for writing bytes from `buf`.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>>;

    /// Poll for flushing buffered writes.
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>>;

    /// Poll for cleanly shutting down the write half of the stream.
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>>;
}

/// Boxed dynamic byte stream handed from L4 transport to protocol engines.
pub type BoxStreamIo = Box<dyn AsyncByteIo>;

/// Connection lifetime classification used during graceful drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionKind {
    /// Short-lived request/response connections awaited in drain phase 1.
    ShortLived,
    /// Streaming or persistent connections notified via [`DrainSignal`] in phase 2.
    LongLived,
}

/// Metadata attached to an accepted transport connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionMeta {
    pub id: u64,
    pub remote_addr: SocketAddr,
    pub kind: ConnectionKind,
}

/// Per-connection coordination handle exposed by Layer 1 to Layer 2 engines.
pub trait DrainSignal: Send + Sync {
    /// Resolve once graceful drain reaches the signaling point for this connection.
    fn wait(&self) -> BoxFuture<'static, ()>;

    /// Return `true` as soon as listener or daemon shutdown has begun.
    fn is_draining(&self) -> bool;

    /// Promote the connection classification (for example, to [`ConnectionKind::LongLived`]
    /// when a stream or WebSocket upgrade begins).
    fn set_kind(&self, kind: ConnectionKind);

    /// Shorthand for promoting a connection to [`ConnectionKind::LongLived`].
    fn mark_long_lived(&self) {
        self.set_kind(ConnectionKind::LongLived);
    }
}

/// Protocol entrypoint invoked by Layer 1 for each accepted connection.
pub trait ConnectionHandler: Send + Sync + 'static {
    /// Serve one connection until completion or cancellation.
    fn handle(
        &self,
        io: BoxStreamIo,
        meta: ConnectionMeta,
        drain: Arc<dyn DrainSignal>,
    ) -> BoxFuture<'static, ()>;
}

/// Layer 1 listener and connection management contract trait.
pub trait Transport: Send + Sync + 'static {
    /// Bind a listener on `addr` owned by `caller`, forwarding accepted streams
    /// to `handler`. Disposing `caller` (or the returned [`EffectHandle`])
    /// stops accepting new connections and performs two-phase graceful drain.
    fn listen(
        &self,
        caller: &FrameContext,
        addr: SocketAddr,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>;

    /// Return the current number of tracked active connections.
    fn active_connections(&self) -> usize;
}

/// Frame service handle wrapping any [`Transport`] implementation.
#[derive(Clone)]
pub struct TransportService(pub Arc<dyn Transport>);

impl TransportService {
    /// Wrap a [`Transport`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn Transport>) -> Self {
        Self(inner)
    }

    /// Delegate [`Transport::listen`] explicitly binding effects to `caller`.
    pub fn listen(
        &self,
        caller: &FrameContext,
        addr: SocketAddr,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        self.0.listen(caller, addr, handler)
    }

    /// Delegate [`Transport::active_connections`].
    pub fn active_connections(&self) -> usize {
        self.0.active_connections()
    }
}

/// Caller-bound [`TransportService`] view obtained via `ctx.use_service::<TransportService>()`.
#[derive(Clone)]
pub struct ScopedTransport {
    inner: Arc<TransportService>,
    caller: FrameContext,
}

impl ScopedTransport {
    /// Bind a listener whose lifetime is owned by the caller fiber.
    pub fn listen(
        &self,
        addr: SocketAddr,
        handler: Arc<dyn ConnectionHandler>,
    ) -> BoxFuture<'static, Result<(SocketAddr, EffectHandle), FrameError>>
    {
        self.inner.listen(&self.caller, addr, handler)
    }

    /// Return the current number of tracked active connections.
    pub fn active_connections(&self) -> usize {
        self.inner.active_connections()
    }
}

impl Service for TransportService {
    const NAME: &'static str = "net.transport";
}

impl ScopedService for TransportService {
    type View = ScopedTransport;

    fn bind(self: Arc<Self>, caller: FrameContext) -> Self::View {
        ScopedTransport {
            inner: self,
            caller,
        }
    }
}

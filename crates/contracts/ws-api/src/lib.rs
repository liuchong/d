//! Zero-dependency Layer 2 WebSocket contracts: frames, messages,
//! caller-scoped sessions, and route registration.

use frame::{
    BoxFuture, Context as FrameContext, EffectHandle, FrameError,
    ScopedService, Service,
};
use http_api::RouteSpec;
use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

/// RFC 6455 frame opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WsOpcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl WsOpcode {
    /// Parse a 4-bit wire opcode.
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v & 0x0f {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xa => Some(Self::Pong),
            _ => None,
        }
    }

    /// Encode as wire 4-bit opcode byte.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Continuation => 0x0,
            Self::Text => 0x1,
            Self::Binary => 0x2,
            Self::Close => 0x8,
            Self::Ping => 0x9,
            Self::Pong => 0xa,
        }
    }

    /// Return `true` for control opcodes (`Close`, `Ping`, `Pong`).
    pub const fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

/// Optional close code and UTF-8 reason payload attached to a Close frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsCloseFrame {
    pub code: u16,
    pub reason: String,
}

/// High-level decoded WebSocket message delivered to and sent by handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<WsCloseFrame>),
}

/// Transport-backed WebSocket connection operations implemented by protocol engines.
pub trait WsSessionBackend: Send + Sync + 'static {
    /// Send a WebSocket message asynchronously, awaiting bounded queue capacity.
    fn send(&self, msg: WsMessage) -> BoxFuture<'static, io::Result<()>>;

    /// Attempt to enqueue a message immediately without waiting (for backpressure enforcement).
    fn try_send(&self, msg: WsMessage) -> io::Result<()>;

    /// Receive the next decoded client message, or `None` once closed cleanly.
    fn recv(&self) -> BoxFuture<'static, Option<io::Result<WsMessage>>>;

    /// Initiate a clean RFC 6455 close handshake.
    fn close(
        &self,
        frame: Option<WsCloseFrame>,
    ) -> BoxFuture<'static, io::Result<()>>;

    /// Request path captured on the HTTP upgrade handshake.
    fn path(&self) -> &str;

    /// Extracted [`http_api::RouteMatch::Param`] captures from the upgrade request.
    fn path_params(&self) -> &BTreeMap<String, String>;

    /// Client socket address from Layer 1 transport metadata.
    fn remote_addr(&self) -> Option<SocketAddr>;
}

/// Cloneable handle to an active WebSocket session.
#[derive(Clone)]
pub struct WsSession(pub Arc<dyn WsSessionBackend>);

impl WsSession {
    /// Send one WebSocket message and await queue flush.
    pub fn send(&self, msg: WsMessage) -> BoxFuture<'static, io::Result<()>> {
        self.0.send(msg)
    }

    /// Non-blocking send returning `WouldBlock` when the bounded outbound queue is full.
    pub fn try_send(&self, msg: WsMessage) -> io::Result<()> {
        self.0.try_send(msg)
    }

    /// Receive the next incoming message from the peer.
    pub fn recv(&self) -> BoxFuture<'static, Option<io::Result<WsMessage>>> {
        self.0.recv()
    }

    /// Send a Close frame and stop the write loop cleanly.
    pub fn close(
        &self,
        frame: Option<WsCloseFrame>,
    ) -> BoxFuture<'static, io::Result<()>> {
        self.0.close(frame)
    }

    /// Return the upgrade request path.
    pub fn path(&self) -> &str {
        self.0.path()
    }

    /// Return path parameters captured on upgrade.
    pub fn path_params(&self) -> &BTreeMap<String, String> {
        self.0.path_params()
    }

    /// Return the peer remote socket address when available.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.0.remote_addr()
    }
}

/// Application handler invoked once for each accepted WebSocket session.
pub trait WsHandler: Send + Sync + 'static {
    /// Drive a single WebSocket session until completion or close.
    fn on_connect(&self, session: WsSession) -> BoxFuture<'static, ()>;
}

/// Layer 2 WebSocket route mounting contract.
pub trait WsRegistry: Send + Sync + 'static {
    /// Mount a WebSocket endpoint owned by `caller`, returning an [`EffectHandle`]
    /// whose disposal removes the route from the HTTP router.
    fn mount_ws(
        &self,
        caller: &FrameContext,
        spec: RouteSpec,
        handler: Arc<dyn WsHandler>,
    ) -> Result<EffectHandle, FrameError>;
}

/// Frame service handle wrapping any [`WsRegistry`] implementation.
#[derive(Clone)]
pub struct WsRegistryService(pub Arc<dyn WsRegistry>);

impl WsRegistryService {
    /// Wrap a [`WsRegistry`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn WsRegistry>) -> Self {
        Self(inner)
    }

    /// Delegate [`WsRegistry::mount_ws`] with explicit caller ownership.
    pub fn mount_ws(
        &self,
        caller: &FrameContext,
        spec: RouteSpec,
        handler: Arc<dyn WsHandler>,
    ) -> Result<EffectHandle, FrameError> {
        self.0.mount_ws(caller, spec, handler)
    }
}

/// Caller-bound [`WsRegistryService`] view obtained via `ctx.use_service::<WsRegistryService>()`.
#[derive(Clone)]
pub struct ScopedWsRegistry {
    inner: Arc<WsRegistryService>,
    caller: FrameContext,
}

impl ScopedWsRegistry {
    /// Mount a WebSocket route owned by the caller fiber.
    pub fn mount_ws(
        &self,
        spec: RouteSpec,
        handler: Arc<dyn WsHandler>,
    ) -> Result<EffectHandle, FrameError> {
        self.inner.mount_ws(&self.caller, spec, handler)
    }
}

impl Service for WsRegistryService {
    const NAME: &'static str = "ws.registry";
}

impl ScopedService for WsRegistryService {
    type View = ScopedWsRegistry;

    fn bind(self: Arc<Self>, caller: FrameContext) -> Self::View {
        ScopedWsRegistry {
            inner: self,
            caller,
        }
    }
}

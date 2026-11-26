//! Zero-dependency Layer 1 TLS acceptor, connector, SNI, and ALPN contracts.

use frame::{BoxFuture, Service};
use net_api::BoxStreamIo;
use std::io;
use std::sync::Arc;

/// Certificate and private key pair scoped to an optional SNI hostname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SniCertConfig {
    /// Hostname (`example.com` or `*.example.com`), or `None` for the default certificate.
    pub server_name: Option<String>,
    pub cert_pem: String,
    pub key_pem: String,
}

/// Declarative TLS configuration bound to a named listener identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsListenerEntry {
    pub id: String,
    pub certs: Vec<SniCertConfig>,
    pub alpn_protocols: Vec<String>,
}

/// Outbound TLS handshake parameters for upstream client connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsClientConfig {
    pub server_name: String,
    pub alpn_protocols: Vec<String>,
    pub ca_cert_pem: Option<String>,
    pub insecure_skip_verify: bool,
}

impl TlsClientConfig {
    /// Construct a default TLS client config verifying against built-in webpki roots.
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            alpn_protocols: vec!["http/1.1".to_string()],
            ca_cert_pem: None,
            insecure_skip_verify: false,
        }
    }
}

/// Negotiated TLS byte stream together with handshake metadata.
pub struct NegotiatedTls {
    pub stream: BoxStreamIo,
    pub alpn: Option<Vec<u8>>,
    pub server_name: Option<String>,
}

/// Inbound TLS termination contract consumed optionally by Layer 1 listeners.
pub trait TlsAcceptor: Send + Sync + 'static {
    /// Perform server-side TLS handshake for `listener_id` over `raw`.
    fn wrap_inbound(
        &self,
        listener_id: &str,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>>;

    /// Return `true` when `listener_id` currently has a configured certificate set.
    fn has_listener(&self, listener_id: &str) -> bool;
}

/// Outbound TLS initiation contract consumed by upstream clients and proxies.
pub trait TlsConnector: Send + Sync + 'static {
    /// Perform client-side TLS handshake over `raw`.
    fn wrap_outbound(
        &self,
        cfg: &TlsClientConfig,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>>;
}

/// Frame service handle wrapping any [`TlsAcceptor`] implementation.
#[derive(Clone)]
pub struct TlsAcceptorService(pub Arc<dyn TlsAcceptor>);

impl TlsAcceptorService {
    /// Wrap a [`TlsAcceptor`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn TlsAcceptor>) -> Self {
        Self(inner)
    }

    /// Delegate [`TlsAcceptor::wrap_inbound`].
    pub fn wrap_inbound(
        &self,
        listener_id: &str,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>> {
        self.0.wrap_inbound(listener_id, raw)
    }

    /// Delegate [`TlsAcceptor::has_listener`].
    pub fn has_listener(&self, listener_id: &str) -> bool {
        self.0.has_listener(listener_id)
    }
}

impl Service for TlsAcceptorService {
    const NAME: &'static str = "tls.acceptor";
}

/// Frame service handle wrapping any [`TlsConnector`] implementation.
#[derive(Clone)]
pub struct TlsConnectorService(pub Arc<dyn TlsConnector>);

impl TlsConnectorService {
    /// Wrap a [`TlsConnector`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn TlsConnector>) -> Self {
        Self(inner)
    }

    /// Delegate [`TlsConnector::wrap_outbound`].
    pub fn wrap_outbound(
        &self,
        cfg: &TlsClientConfig,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>> {
        self.0.wrap_outbound(cfg, raw)
    }
}

impl Service for TlsConnectorService {
    const NAME: &'static str = "tls.connector";
}

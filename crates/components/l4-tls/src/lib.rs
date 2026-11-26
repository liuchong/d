//! Layer 1 pure-Rust TLS termination, SNI selection, ALPN negotiation, and
//! atomic certificate hot-reload plugin.

mod io;
mod store;

use frame::{
    BoxFuture, ConfigFieldSchema, ConfigFieldType, ConfigValue, Context, Deps,
    FrameError, Plugin, PluginSchema,
};
use std::sync::Arc;
pub use store::DefaultTlsConnector;
use store::{TlsStore, parse_and_compile_listeners};
pub use tls_api::{
    NegotiatedTls, SniCertConfig, TlsAcceptor, TlsAcceptorService,
    TlsClientConfig, TlsConnector, TlsConnectorService, TlsListenerEntry,
};

/// Layer 1 plugin providing [`TlsAcceptorService`] and [`TlsConnectorService`].
#[derive(Clone, Default)]
pub struct TlsPlugin {
    base: Vec<TlsListenerEntry>,
    store: Arc<TlsStore>,
}

impl std::fmt::Debug for TlsPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsPlugin")
            .field("base_listeners", &self.base.len())
            .finish()
    }
}

impl TlsPlugin {
    /// Create a TLS plugin with programmatic default listener entries.
    pub fn new(listeners: Vec<TlsListenerEntry>) -> Self {
        Self {
            base: listeners,
            store: Arc::new(TlsStore::default()),
        }
    }
}

impl Plugin for TlsPlugin {
    fn name(&self) -> &str {
        "l4-tls"
    }

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::new(
            "Layer 1 pure-Rust TLS 1.2/1.3 termination, SNI, ALPN, outbound connector, and atomic hot cert reload",
        )
        .field(ConfigFieldSchema::new(
            "listeners",
            ConfigFieldType::List,
            "List of TLS listener entries ({id, alpn, certs: [{server_name, cert_pem/cert_path, key_pem/key_path}]})",
        ))
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let _ = parse_and_compile_listeners(&self.base, raw)?;
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let base = self.base.clone();
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let compiled = parse_and_compile_listeners(&base, &config)?;
            store.swap(compiled);

            let acceptor_svc: Arc<TlsAcceptorService> =
                Arc::new(TlsAcceptorService::new(store));
            let connector_svc: Arc<TlsConnectorService> = Arc::new(
                TlsConnectorService::new(Arc::new(DefaultTlsConnector)),
            );

            ctx.provide(acceptor_svc).await?;
            ctx.provide(connector_svc).await?;
            Ok(())
        })
    }
}

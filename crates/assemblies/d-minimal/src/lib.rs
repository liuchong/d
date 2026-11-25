//! Minimal static HTTP assembly for `d`: wires Layer 0 (`frame`),
//! L4 transport, HTTP/1.1 engine, CORS, static file service, and the
//! zero-dependency [`PlainDocRenderPlugin`], omitting compression and rich viewers.

use app_files::FilesAppPlugin;
pub use d_daemon::{AppHandle, AssemblySpec, DaemonOptions as Options};
use doc_render_plain::PlainDocRenderPlugin;
use ext_cors::CorsPlugin;
use frame::{
    ConfigValue, EntryTarget, FrameError, PluginEntry, StaticResolver,
};
use l4_transport::TransportPlugin;
use proto_http::HttpEnginePlugin;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn entry(id: &str, config: ConfigValue) -> PluginEntry {
    PluginEntry {
        id: id.to_string(),
        target: EntryTarget::Name(id.to_string()),
        config,
        disabled: false,
        isolate: Vec::new(),
        children: Vec::new(),
    }
}

/// Build the declarative [`AssemblySpec`] for `d-minimal`.
pub fn build_spec(
    root: PathBuf,
    options: Options,
    bind_addr: Option<SocketAddr>,
) -> AssemblySpec {
    let resolver = Arc::new(
        StaticResolver::new()
            .register("l4-transport", TransportPlugin::default())
            .register("proto-http", HttpEnginePlugin::default())
            .register("ext-cors", CorsPlugin)
            .register("app-files", FilesAppPlugin::default())
            .register("doc-render-plain", PlainDocRenderPlugin),
    );

    let mut entries = vec![entry("l4-transport", ConfigValue::Null)];

    let http_config = match bind_addr {
        Some(addr) => ConfigValue::Map(BTreeMap::from([(
            "bind_addr".to_string(),
            ConfigValue::Str(addr.to_string()),
        )])),
        None => ConfigValue::Null,
    };
    entries.push(entry("proto-http", http_config));
    if options.cors {
        entries.push(entry("ext-cors", ConfigValue::Null));
    }
    entries.push(entry(
        "app-files",
        ConfigValue::Map(BTreeMap::from([
            (
                "root".to_string(),
                ConfigValue::Str(root.to_string_lossy().into_owned()),
            ),
            (
                "allow_hidden".to_string(),
                ConfigValue::Bool(options.allow_hidden),
            ),
        ])),
    ));
    entries.push(entry("doc-render-plain", ConfigValue::Null));

    AssemblySpec { resolver, entries }
}

/// Assemble the `d-minimal` plugin tree for `root` with optional TCP listener binding.
pub async fn build_runtime(
    root: PathBuf,
    options: Options,
    bind_addr: Option<SocketAddr>,
) -> Result<AppHandle, FrameError> {
    let spec = build_spec(root, options, bind_addr);
    d_daemon::build_from_spec_named("d-minimal", spec, bind_addr.is_some())
        .await
}

/// Construct an in-memory `d-minimal` application serving `root` with default options.
pub async fn create_app(
    root: impl AsRef<Path>,
    allow_hidden: bool,
) -> AppHandle {
    create_app_with_options(
        root,
        Options {
            allow_hidden,
            cors: false,
        },
    )
    .await
}

/// Construct an in-memory `d-minimal` application serving `root` with explicit [`Options`].
pub async fn create_app_with_options(
    root: impl AsRef<Path>,
    options: Options,
) -> AppHandle {
    let path = root.as_ref().to_path_buf();
    let canonical = path.canonicalize().unwrap_or(path);
    build_runtime(canonical, options, None)
        .await
        .expect("in-memory d-minimal runtime assembly must succeed")
}

/// Start `d-minimal` on `addr` serving `root` with default options.
pub async fn start(addr: &SocketAddr, root: &str) {
    start_with_options(addr, root, Options::default()).await;
}

/// Start `d-minimal` on `addr` serving `root` until `SIGINT` or `SIGTERM`.
pub async fn start_with_options(
    addr: &SocketAddr,
    root: &str,
    options: Options,
) {
    d_daemon::start_from_spec(addr, root, options, build_spec).await;
}

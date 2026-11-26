<p align="center">
  <img src="docs/pages/assets/logo.svg" alt="d logo" width="160">
</p>

# d

[![1PL licensed](https://img.shields.io/badge/license-1PL-blue.svg)](./LICENSE)
[![crates.io](https://img.shields.io/crates/v/d.svg)](https://crates.io/crates/d)

`d` is a modular, low-overhead standalone network daemon, reverse proxy, and file/document server built on a zero-dependency plugin microkernel (`d-frame`), a custom HTTP/1.1 & pure-Rust WebSocket engine, pure-Rust TLS, and pure-Rust server-side Markdown and syntax rendering.

**[Usage Handbook (中文)](./docs/usage.md)** ·
**[MCP Control](./docs/mcp.md)** ·
**[Agent Skills](./skills/d/SKILL.md)** ·
**[Project Site Source](./docs/index.html)** ·
**[Examples](./examples/README.md)** ·
**[Architecture](./docs/architecture.md)** ·
**[Microkernel (`frame`)](./docs/frame.md)**

---

## Key Capabilities

- **Layered plugin architecture (5 layers, 32 workspace crates — 31 publishable `d-*` crates on crates.io + `xtask`, 19 zero-external-dep pure crates)**:
  - Layer 0 (`d-frame`): zero-dependency pure-`std` microkernel (`ScopedService`, `PluginSchema`, `Context`, `Scope`, `Fiber`, `EffectHandle`, `Event`, `Loader`, `Inspect`).
  - Layer 1 (`d-l4-transport`, `d-l4-tls` via `d-net-api`/`d-tls-api`): multi-listener TCP (`max_connections`, `stats`), two-phase graceful connection drain, and pure-Rust TLS 1.2/1.3 (`rustls`) with SNI, ALPN, and atomic zero-drop hot certificate reload.
  - Layer 2 (`d-proto-http`, `d-proto-ws`, `d-proto-tcp` via `d-http-api`/`d-ws-api`): custom bounded HTTP/1.1 parser & streaming chunked writer (`RouteMatch::Param :name/*rest`, streaming request bodies, `101 Upgrade` handoff), lock-free snapshot `HttpRouter`, pure-Rust RFC 6455 WebSocket engine (only `tokio`), and raw TCP service/forwarding.
  - Layer 3 (`d-ext-proxy`, `d-ext-observability`, `d-ext-ratelimit`, `d-ext-auth`, `d-ext-dev-kit`, `d-ext-compression`, `d-ext-cors`): L7/L4 reverse proxy with Keep-Alive connection pooling, `round_robin`/`least_conn`/`weighted_round_robin` LB, active/passive health checks, RFC 7239 `Forwarded`, zero-buffer SSE passthrough, and WS `101` tunnels; three zero-external-dep pure-`std` governance extensions (structured access logs + OpenMetrics `/metrics` + `/healthz`/`/readyz`, token-bucket `429` rate limiting, constant-time Bearer/Basic/CIDR `401`/`403` auth); declarative dev workflow suite (HTTP/SSE mocks, zero-CDN Live Reload, white `#ffffff` `/_d/inspect` with SSE timeline & replay); streaming-aware `br`/`gzip`/`deflate` compression and opt-in CORS.
  - Layer 4 (`d-app-files`, `d-doc-render`, `d-doc-render-plain`, `d-viewer-markdown`, `d-viewer-code`, `d-viewer-media` via `d-files-api`/`d-render-api`): multi-mount (`mount_prefix`, `host`) sandboxed file server, pure-Rust GFM/HTML renderer with optional strict-allowlist `sanitize_html` mode, interchangeable `d-doc-render-plain`, and minimalist white viewers.
  - Entries & MCP (`d-cli`, `d-daemon`, optional `d-mcp`): pure-`std` CLI (including `-c/--config` / `D_CONFIG`), component-agnostic daemon harness with pure-`std` JSON manifest loading (using concise runtime plugin IDs such as `l4-tls`, `ext-proxy`, `app-files`) & `SIGHUP` preflight incremental reload (preserving active SSE/WS streams), and opt-in pure-Rust MCP control plane self-discovering all 17 full-assembly plugins paired with [`skills/d`](./skills/d/SKILL.md) and [`skills/d-dev`](./skills/d-dev/SKILL.md).
- **Built-in CommonMark + GFM, native HTML & strict allowlist sanitizer**: Zero-dependency `d-doc-render` compiles Markdown on the server with CJK/ASCII unique heading anchors, aligned GFM tables, task lists, strikethrough, footnotes, autolinks, and unescaped block/inline HTML (`<details>`, `<figure>`, `<img>`, `<table>`, `<kbd>`); setting `sanitize_html: true` enables a pure-Rust strict tag/attribute allowlist, quote-aware tag scanner, and URI entity decoder.
- **Server-side lexical code highlighting**: Built-in tokenizer and light stylesheet for Rust, JavaScript/TypeScript, Python, Go, C/C++/Java/Zig/Swift/Kotlin/Scala, Shell (`sh`/`bash`/`zsh`/`fish`), JSON, YAML, TOML, SQL, HTML/XML/SVG, and CSS/SCSS — zero external scripts or CDNs.
- **Minimalist white UI**: Self-contained offline `#ffffff` / `#f6f8fa` / `#0969da` layout across directory listings and document/code/media viewers; no dark themes, no colored badges, no emoji, and no client JavaScript required.
- **Conflict-free content & media routing**: Bare media/asset/HTML paths stream raw bytes cleanly; documents/source render white server-side viewer pages; explicit `?view=` parameters select `raw`, `preview`, `viewer`, or `download` (RFC 5987 UTF-8 `Content-Disposition`). Directories without trailing slashes redirect via `301` preserving query strings.
- **No mode split**: `d` makes no internal "dev mode vs prod mode" distinction — serving local documentation, static sites, and files on an engineer's machine is treated as a first-class production scenario using the exact same HTTP/1.1 engine and lifecycle guarantees.

---

## Usage

### Install

```bash
cargo install d
# or install from source
cargo install --git https://github.com/liuchong/d
```

Build locally from a cloned checkout:

```bash
cargo build --release
# binary at target/release/d
```

### Quick Start & Common Recipes

```bash
# Serve the current directory on http://localhost:8080
d

# Serve a specific directory on all interfaces
d -H 0.0.0.0 -p 8080 -r ./public

# Enable hidden dotfiles toggle and permissive CORS (GET/HEAD/OPTIONS)
d -r ./workspace --hidden --cors -l debug

# Using --flag=value syntax or environment variables (CLI flags override ENV)
d --host=127.0.0.1 --port=3000 --root=./docs
D_HOST=0.0.0.0 D_PORT=9000 D_ROOT=./examples/demo D_SHOW_HIDDEN=true d

# Start with a declarative JSON gateway/dev manifest and reload safely via SIGHUP
d -c examples/gateway/gateway.json
d -c examples/gateway/dev.json
kill -HUP <pid>

# Preview the project's documentation site locally
cargo run -- -r docs -p 8080
```

### CLI Options & Environment Variables

| Option | Env Var | Default | Notes |
|--------|---------|---------|-------|
| `-H, --host <HOST>` | `D_HOST` | `localhost` | Listening host; prefers IPv4 when resolving names like `localhost` |
| `-p, --port <PORT>` | `D_PORT` | `8080` | Listening TCP port (`u16`) |
| `-r, --root <PATH>` | `D_ROOT` | `.` | Root directory to serve (canonicalized; symlink escapes rejected) |
| `-c, --config <PATH>` | `D_CONFIG` | unset | Declarative JSON plugin manifest (`SIGHUP` triggers preflight incremental reload) |
| `-l, --log <LEVEL>` | `RUST_LOG` | `info` | `trace`, `debug`, `info`, `warn`, `error` |
| `--hidden` | `D_SHOW_HIDDEN` | disabled | Expose dotfiles and allow `?hidden=true\|false` toggling (`1`/`true`/`yes`/`on`) |
| `--cors` | `D_CORS` | disabled | Add `Access-Control-Allow-Origin: *` and handle `OPTIONS` preflight (`204`) |
| `--no-http` | `D_NO_HTTP` | disabled | Start router in memory without binding HTTP TCP port until configured via MCP |
| `--mcp-stdio` | `D_MCP_STDIO` | disabled | Enable pure-Rust stdio MCP control server (routes daemon logs to `stderr`) |
| `--mcp-http <ADDR>` | `D_MCP_HTTP` | unset | Enable Streamable HTTP MCP endpoint on `<host>:<port>` |
| `--mcp-path <PATH>` | `D_MCP_PATH` | `/mcp` | HTTP MCP `POST` route path |
| `--mcp-token <TOKEN>` | `D_MCP_TOKEN` | unset | Bearer token protecting the HTTP MCP endpoint |
| `--mcp-read-only` | `D_MCP_READ_ONLY` | disabled | Restrict MCP to read-only inspection, schema discovery, and probing |
| `--mcp-allow-shutdown` | `D_MCP_ALLOW_SHUTDOWN` | disabled | Allow MCP `d_shutdown` tool |
| `-h, --help` | — | — | Print help and exit |
| `-V, --version` | — | — | Print version and exit |

Precedence is strictly **CLI flag > Environment variable > Default**. Boolean environment variables treat `1`, `true`, `yes`, and `on` (case-insensitive) as enabled. MCP is disabled by default unless `--mcp-stdio` / `--mcp-http` is explicitly enabled; build without MCP via `cargo build -p d --no-default-features`, or add MCP to `d-minimal` via `cargo build -p d-minimal --features mcp`. See [docs/mcp.md](./docs/mcp.md).

---

## Web Interface, Routing & Query Parameters

### Conflict-free Resource Routing

| Resource Type | Bare Path (`/path/to/file`) | Explicit Query Modes |
|---------------|-----------------------------|----------------------|
| **Directories** (`/dir/`) | Serves sandboxed `index.html` if present, otherwise renders minimalist white listing; missing trailing `/` issues `301` preserving `?query` | `?listing=true` (bypass `index.html`), `?sort=name\|size\|time\|type`, `?order=desc`, `?hidden=true\|false` (requires `--hidden`) |
| **Markdown** (`.md`, `.markdown`, `.mkd`, `.mdown`) | Renders minimalist white reading view with server-side HTML + code highlighting and top action bar | `?view=raw` (raw UTF-8), `?view=preview` (document body without toolbar), `?view=download` |
| **Code / Text / Org** (`.rs`, `.py`, `.go`, `.ts`, `.toml`, `.yaml`, `.txt`, `.csv`, `.org`, etc.) | Renders white server-side syntax-highlighted or monospace viewer (note: `.js`/`.mjs`/`.cjs` bare paths stream raw script bytes for static websites) | `?view=raw`, `?view=preview`, `?view=viewer`, `?view=download` |
| **Media & Web Assets** (images, video, audio, `.html`, `.css`, `.js`, fonts, binaries) | Streams raw bytes with accurate MIME type, `Range` (`206`/`416`), `ETag`/`Last-Modified` (`304`), and `HEAD` | `?view=viewer` (white `<img/video/audio>` viewer embedding the bare stream), `?view=download` (RFC 5987 attachment) |

### Common `curl` Commands

```bash
# Directory trailing-slash normalization (301 Location: /docs/?sort=size)
curl -i "http://localhost:8080/docs?sort=size"

# Byte-range slice (206 Partial Content) & suffix range
curl -r 0-1023 http://localhost:8080/data.bin
curl -r -16 http://localhost:8080/data.bin

# Conditional caching with ETag (returns 304 Not Modified on match)
ETAG=$(curl -sI http://localhost:8080/data.bin | awk '/^etag:/{print $2}' | tr -d '\r')
curl -i -H "If-None-Match: $ETAG" http://localhost:8080/data.bin

# Fetch raw source of a rendered Markdown or code file, or force RFC 5987 download
curl "http://localhost:8080/README.md?view=raw"
curl -OJ "http://localhost:8080/README.md?view=download"

# Inspect negotiated compression vs 206 exemption
curl -i --compressed http://localhost:8080/README.md
curl -i --compressed -r 0-99 http://localhost:8080/data.bin
```

---

## Rust Library Usage

`d` can also be embedded in Rust applications or driven in-memory without binding a TCP port:

```rust
use std::net::SocketAddr;
use d::{Options, create_app_with_options, start_with_options};
use d_http_api::{HttpRequest, Method};

// 1. Start full HTTP/1.1 daemon until SIGINT/SIGTERM
async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    start_with_options(
        &addr,
        "./public",
        Options {
            allow_hidden: false,
            cors: true,
        },
    )
    .await;
    Ok(())
}

// 2. Dispatch in-memory through the full plugin & filter pipeline
async fn dispatch_once() -> Result<(), Box<dyn std::error::Error>> {
    let app = create_app_with_options(
        "./examples/docs",
        Options {
            allow_hidden: false,
            cors: false,
        },
    )
    .await;

    let resp = app.dispatch(HttpRequest::new(Method::Get, "/guide.md")).await;
    assert_eq!(resp.status.as_u16(), 200);
    let _body = resp.body.into_bytes().await?;

    app.dispose().await?;
    Ok(())
}
```

---

## Workspace Crates & Examples

| Layer | Crates (`crates.io` name) | Responsibility |
|-------|--------|----------------|
| **Layer 0** | `crates/frame` (`d-frame`) | Pure-`std` zero-dependency plugin microkernel (`ScopedService`, `Context`, `Scope`, `Fiber`, `EffectScope`, `Event`, `Loader`, `Inspect`) |
| **Contracts** | `crates/contracts/{net-api,tls-api,http-api,ws-api,proxy-api,files-api,render-api}` (`d-*-api`) | 7 zero-dependency trait and wire contracts decoupling all layers |
| **Layer 1** | `crates/components/{l4-transport,l4-tls}` (`d-l4-*`) | Listener limits & stats, two-phase drain, and pure-Rust TLS (SNI/ALPN/hot cert reload) |
| **Layer 2** | `crates/components/{proto-http,proto-ws,proto-tcp}` (`d-proto-*`) | Parameter/streaming HTTP/1.1 (`101` upgrade handoff), pure-Rust RFC 6455 WebSocket, and raw TCP engine |
| **Layer 3** | `crates/components/{ext-proxy,ext-observability,ext-ratelimit,ext-auth,ext-dev-kit,ext-compression,ext-cors}` (`d-ext-*`) | L7/L4 reverse proxy, pure-`std` logs/metrics/probes, `429` rate limit, `401`/`403` auth, Mock/Live Reload/Inspector, compression & CORS |
| **Layer 4** | `crates/components/{app-files,doc-render,doc-render-plain,viewer-*}` (`d-app-files`, `d-doc-render*`, `d-viewer-*`) | Multi-mount sandboxed files, CommonMark/GFM/HTML (+ optional `sanitize_html`), interchangeable `d-doc-render-plain`, white viewers |
| **Entries & Assemblies** | `crates/entries/{cli,daemon,mcp}`, `crates/assemblies/{d,d-minimal}` | Pure-`std` `d-cli`, component-agnostic `d-daemon` (`-c`/`SIGHUP`), optional `d-mcp`, full `d` (17 plugins) & lightweight `d-minimal` (`tokio` only) |
| **Skills & Tooling** | `skills/{d,d-dev}`, `xtask` | Self-evolving runtime/contributor Skills & pure-`std` workspace governance (`cargo xtask check`, `info`, `test`) |

Ready-to-run sample directories live in [`examples/`](./examples) — see [examples/README.md](./examples/README.md) for guided tours of `demo/`, `code/`, `docs/`, `media/`, `static-site/`, and `gateway/`.

---

## Testing & Acceptance

```bash
cargo xtask check          # Enforce all 8 workspace architecture & skill invariants across 32 packages
cargo xtask info           # Verify assembly matrix & 19 zero-external-dep pure crates
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p d --no-default-features --all-targets -- -D warnings
cargo clippy -p d-minimal --features mcp --all-targets -- -D warnings
cargo test --workspace     # Full workspace unit, contract, MCP, gateway & SIGHUP tests
cargo test -p d-minimal --features mcp
scripts/acceptance.sh      # End-to-end HTTP, Markdown, UI, gateway, governance & SIGHUP checks
```

For comprehensive Chinese usage documentation, see [docs/usage.md](./docs/usage.md).

---

## License

This project is licensed under the [One Public License (1PL)](./LICENSE).

---
name: d-dev
description: Architecture, extension, and release verification workflow for modifying the d codebase. Covers adding contracts, components, config schemas, assemblies, and xtask / Skill / MCP synchronization.
---

# `d-dev` Maintainer & Evolution Skill

Develop and extend `d` while keeping the 5-layer Virtual Workspace, `frame` microkernel invariants, `d-mcp` live schemas, Skills, and documentation synchronized.

## Key Architectural Invariants

Read [Architecture Rules](references/architecture-rules.md) for all 9 automated `cargo xtask check` rules. Never bypass them:

1. **`crates/frame`** (`d-frame`, `kernel`) is pure `std` with zero dependencies.
2. **`crates/contracts/*`** (`d-*-api`, `contracts`) contain traits/types only and depend solely on `d-frame` and fellow contracts.
3. **`crates/components/*`** (`d-*`, `components`) never depend on sibling components, `entries`, or `assemblies`. Bind resources via `ctx.use_service::<S>()` so caller fiber teardown auto-cleans effects.
4. **`crates/entries/*`** (`d-cli`, `d-daemon`, `d-mcp`) never depend on `crates/components/*`.
5. **`crates/assemblies/*`** (`d`, `d-minimal`) declaratively assemble components via `StaticResolver` + `PluginEntry` + `ConfigValue`. Keep `d-minimal` free of `d-l4-tls`, `d-proto-ws`, `d-proto-tcp`, `d-ext-proxy`, `d-ext-observability`, `d-ext-ratelimit`, `d-ext-auth`, `d-ext-dev-kit`, `d-ext-compression`, `d-doc-render`, `d-viewer-*`, `brotli`, `flate2`, `rustls`, and `tokio-rustls`.
6. **Single-Crate Publishing (`R-ARCH-9`)**: Every workspace crate except `d` sets `publish = false` (`publish_allowlist = ["d"]`). Never publish internal crates individually; use `cargo xtask publish-d --dry-run` (and `--execute` when authorized) to bundle and publish `d` alone.

## Adding or Extending Components So MCP & Skills Stay In Sync

When adding a config field or a new component plugin:

1. **Single Schema Source of Truth**: Implement `Plugin::validate_config` AND `Plugin::schema` in the component crate, marking any credential/token fields with `.sensitive()`. This automatically makes `d_catalog` and `d_inspect` (redaction) expose and protect the new option without editing `d-mcp`.
2. **Workspace Manifest**: Register any new crate in root `Cargo.toml` (`members` + `[workspace.metadata.architecture]`, and `zero_external_deps` if pure `std`).
3. **Register in Assemblies**: Register the plugin in `StaticResolver` inside `crates/assemblies/d` (and `d-minimal` only if appropriate for minimal static hosting).
4. **Sync Skills & Docs**: If adding new MCP tools, CLI flags, or core behavior, update `skills/d/`, `docs/`, `README.md`, and run `cargo xtask check`.

## Mandatory Verification Sequence

```bash
cargo xtask check
cargo xtask info
cargo xtask publish-d --dry-run
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p d --no-default-features -- -D warnings
cargo clippy -p d-minimal --features mcp --all-targets -- -D warnings
cargo test --workspace
cargo test -p d-minimal --features mcp
PORT=18099 scripts/acceptance.sh
```

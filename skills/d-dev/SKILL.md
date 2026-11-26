---
name: d-dev
description: Architecture, extension, and release verification workflow for modifying the d codebase. Covers adding contracts, components, config schemas, assemblies, and xtask / Skill / MCP synchronization.
---

# `d-dev` Maintainer & Evolution Skill

Develop and extend `d` while keeping the 5-layer Virtual Workspace, `frame` microkernel invariants, `d-mcp` live schemas, Skills, and documentation synchronized.

## Key Architectural Invariants

Read [Architecture Rules](references/architecture-rules.md) for all 8 automated `cargo xtask check` rules. Never bypass them:

1. **`crates/frame`** (`kernel`) is pure `std` with zero dependencies.
2. **`crates/contracts/*`** (`contracts`) contain traits/types only and depend solely on `frame` and fellow contracts.
3. **`crates/components/*`** (`components`) never depend on sibling components, `entries`, or `assemblies`. Bind resources via `ctx.use_service::<S>()` so caller fiber teardown auto-cleans effects.
4. **`crates/entries/*`** (`d-cli`, `d-daemon`, `d-mcp`) never depend on `crates/components/*`.
5. **`crates/assemblies/*`** (`d`, `d-minimal`) declaratively assemble components via `StaticResolver` + `PluginEntry` + `ConfigValue`. Keep `d-minimal` free of `l4-tls`, `proto-ws`, `proto-tcp`, `ext-proxy`, `ext-observability`, `ext-ratelimit`, `ext-auth`, `ext-dev-kit`, `ext-compression`, `doc-render`, `viewer-*`, `brotli`, `flate2`, `rustls`, and `tokio-rustls`.

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
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p d --no-default-features -- -D warnings
cargo clippy -p d-minimal --features mcp --all-targets -- -D warnings
cargo test --workspace
cargo test -p d-minimal --features mcp
PORT=18099 scripts/acceptance.sh
```

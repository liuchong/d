# `d` Workspace Architecture Rules (`cargo xtask`)

Root `Cargo.toml` defines `[workspace.metadata.architecture]`, statically verified by the zero-dependency pure-`std` `xtask` crate:

| Rule | Scope | Guarantee |
| :--- | :--- | :--- |
| `R-ARCH-1` | Classification | Every workspace package belongs to exactly one layer (`kernel`, `contracts`, `components`, `entries`, `assemblies`, `tools`). |
| `R-ARCH-2` | Pure Crates | Crates listed in `zero_external_deps` (`19` crates: `d-frame`, 7 `d-*-api` contracts, `d-ext-observability`, `d-ext-ratelimit`, `d-ext-auth`, `d-ext-cors`, `d-doc-render`, `d-doc-render-plain`, 3 `d-viewer-*`, `d-cli`, `xtask`) have zero external normal/build dependencies. |
| `R-ARCH-3` | Kernel | `d-frame` (`crates/frame`) has zero normal/build dependencies (pure `std`). |
| `R-ARCH-4` | Contracts | `crates/contracts/*` (`d-*-api`) depend only on `d-frame` and fellow `contracts/*`. |
| `R-ARCH-5` | Components | `crates/components/*` (`d-*`) never depend on other `components/*`, `entries/*`, or `assemblies/*`. Pure-Rust external TLS dependencies are strictly isolated inside `d-l4-tls`. |
| `R-ARCH-6` | Entries | `d-cli`, `d-daemon`, `d-mcp` never depend on `crates/components/*`, `assemblies/*`, or `tools`. |
| `R-ARCH-7` | Assemblies | `assemblies/*` never depend on sibling `assemblies/*` or `tools`; `d-minimal` never transitively includes any crate in `minimal_forbidden` (`d-l4-tls`, `d-proto-ws`, `d-proto-tcp`, `d-ext-proxy`, `d-ext-observability`, `d-ext-ratelimit`, `d-ext-auth`, `d-ext-dev-kit`, `d-ext-compression`, `d-doc-render`, `d-viewer-*`, `brotli`, `flate2`, `rustls`, `tokio-rustls`). |
| `R-ARCH-8` | DAG, Multi-Impl & Skills | Workspace dependency DAG is acyclic; `d-render-api` has >= 2 distinct component implementations (`d-doc-render`, `d-doc-render-plain`); bundled `skills/{d,d-dev}/SKILL.md` have valid YAML frontmatter and zero broken local references. |
| `R-ARCH-9` | Single-Crate Publish | Only packages listed in `publish_allowlist` (`["d"]`) are publishable to `crates.io`; all other 31 workspace crates enforce `publish = false`. Publishing uses `cargo xtask publish-d` to stage a self-contained single `d` crate with zero internal `crates.io` dependencies. |

## Incremental Testing

```bash
cargo xtask test --base master --dry-run
cargo xtask test --base master
```

`cargo xtask test` maps changed files (plus `Cargo.toml`/`Cargo.lock`/`.cargo/`) to workspace packages, computes the reverse transitive closure, and executes `cargo test` only on affected crates.

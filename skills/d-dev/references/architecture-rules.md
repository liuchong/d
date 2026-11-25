# `d` Workspace Architecture Rules (`cargo xtask`)

Root `Cargo.toml` defines `[workspace.metadata.architecture]`, statically verified by the zero-dependency pure-`std` `xtask` crate:

| Rule | Scope | Guarantee |
| :--- | :--- | :--- |
| `R-ARCH-1` | Classification | Every workspace package belongs to exactly one layer (`kernel`, `contracts`, `components`, `entries`, `assemblies`, `tools`). |
| `R-ARCH-2` | Pure Crates | Crates listed in `zero_external_deps` (`13` crates: `frame`, 4 contracts, `ext-cors`, `doc-render`, `doc-render-plain`, 3 `viewer-*`, `d-cli`, `xtask`) have zero external normal/build dependencies. |
| `R-ARCH-3` | Kernel | `frame` has zero normal/build dependencies (pure `std`). |
| `R-ARCH-4` | Contracts | `crates/contracts/*` depend only on `frame` and fellow `contracts/*`. |
| `R-ARCH-5` | Components | `crates/components/*` never depend on other `components/*`, `entries/*`, `assemblies/*`, or `tools`. |
| `R-ARCH-6` | Entries | `d-cli`, `d-daemon`, `d-mcp` never depend on `crates/components/*`, `assemblies/*`, or `tools`. |
| `R-ARCH-7` | Assemblies | `assemblies/*` never depend on sibling `assemblies/*` or `tools`; `d-minimal` never transitively includes `ext-compression`, `doc-render`, `viewer-*`, `brotli`, or `flate2`. |
| `R-ARCH-8` | DAG, Multi-Impl & Skills | Workspace dependency DAG is acyclic; `render-api` has >= 2 distinct component implementations (`doc-render`, `doc-render-plain`); bundled `skills/{d,d-dev}/SKILL.md` have valid YAML frontmatter and zero broken local references. |

## Incremental Testing

```bash
cargo xtask test --base master --dry-run
cargo xtask test --base master
```

`cargo xtask test` maps changed files (plus `Cargo.toml`/`Cargo.lock`/`.cargo/`) to workspace packages, computes the reverse transitive closure, and executes `cargo test` only on affected crates.

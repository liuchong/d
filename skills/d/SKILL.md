---
name: d
description: Run, inspect, and dynamically reconfigure the d / d-minimal HTTP server and its optional MCP control plane. Automatically discovers live plugin schemas and runtime topology so workflows stay compatible as d evolves.
---

# `d` Runtime & MCP Control Skill

Operate `d` (full assembly) and `d-minimal` (lightweight static assembly) from CLI, HTTP/routing contracts, and optional MCP (`d-mcp`).

## Core Principles

1. **Discover Before Mutating (Self-Evolving)**: Never hardcode plugin lists or config keys across versions. When connected via MCP, always start with `d_catalog` and `d_inspect` (or read `d://catalog` and `d://state`) so any newly added plugins, config fields, dependencies, or constraints are discovered dynamically from live `Plugin::schema()`.
2. **Preflight Before Commit**: Use `d_validate_patches` before `d_apply_patches` or `d_apply_manifest` when assembling non-trivial patch batches. Invalid config or unknown entry IDs fail preflight with zero disruption to active HTTP traffic.
3. **Verify Every Change Live**: After mutating state or config, verify both control plane (`d_inspect` / `d_server_info`) and data plane (`d_http_probe` for in-memory verification, or `curl` against `local_addr`).
4. **Respect Policy & Boundaries**: Honor `--mcp-read-only` (only read/probe tools are available) and `--mcp-allow-shutdown` (`d_shutdown` is disabled unless explicitly authorized at launch).

## Quick Decision Map

- **Start / configure CLI flags, ENV vars, conflict-free routes (`?view=raw|preview|viewer|download`, `?listing=true`)**: read [CLI and Routing](references/cli-and-routing.md).
- **Connect or operate MCP (`--mcp-stdio`, `--mcp-http`, `--mcp-token`, `--no-http`, 12 tools, 4 resources) & live adjustment recipes**: read [MCP Control](references/mcp-control.md).
- **Handle unknown plugins, custom assemblies, or future versions gracefully**: read [Self-Evolving Protocol](references/evolution.md).

## Standard Dynamic Reconfiguration Loop

```text
1. d_catalog          -> inspect registered plugins, required/optional deps, field types & defaults
2. d_inspect          -> inspect active entries, redacted configs, fiber states, local_addr, log_level
3. d_validate_patches -> dry-run candidate EntryPatch list (optional for complex topological changes)
4. Mutate             -> d_set_plugin_config | d_toggle_plugin | d_apply_patches | d_apply_manifest | d_restart_plugin | d_set_log_level
5. Verify             -> d_http_probe + d_inspect (confirm FiberState == Active & expected HTTP response)
```

# MCP Control Reference (`d-mcp`)

## 1. Connecting MCP Clients

### Stdio Transport (Single Dedicated Instance)

Logs automatically switch to `stderr` so `stdout` carries pure JSON-RPC 2.0 frames. When stdin reaches EOF, `d` gracefully shuts down.

```json
{
  "mcpServers": {
    "d": {
      "command": "d",
      "args": ["-r", "./docs", "--no-http", "--mcp-stdio", "--mcp-allow-shutdown"]
    }
  }
}
```

Or run HTTP + MCP stdio simultaneously:

```bash
d -H 127.0.0.1 -p 8080 -r ./public --mcp-stdio
```

### Streamable HTTP Transport (Attach to Long-Running Daemon)

```bash
d -H 127.0.0.1 -p 8080 -r ./public \
  --mcp-http 127.0.0.1:18081 \
  --mcp-path /mcp \
  --mcp-token "$D_MCP_TOKEN" \
  --mcp-allow-shutdown
```

Requests must use `POST <http_path>` with `Content-Type: application/json` (and `Authorization: Bearer <token>` if configured). Notifications return `202 Accepted`; requests return `200 OK`.

## 2. Tools Inventory (12)

### Read-Only / Discovery (Always Active)

- `d_server_info`: Live assembly (`d` / `d-minimal`), version, bound `local_addr`, `log_level`, active `mcp` flags (token presence masked), and fiber state counts.
- `d_catalog`: Live plugin list resolved from `LoaderService::catalog()`, returning each plugin's `name`, `description`, `required_deps`, `optional_deps`, and typed `fields` schema (`name`, `type`, `required`, `nullable`, `sensitive`, `default`, `description`).
- `d_inspect`: Combined live snapshot (`server`, `loader_states`, `entries` with sensitive config values masked as `"***REDACTED***"`, and `fibers` topology).
- `d_validate_patches`: Dry-run preflight for `{ "patches": [...] }` without mutating live plugins.
- `d_http_probe`: In-memory dispatch of `{ "method"?: "GET", "uri": "...", "headers"?: { ... } }` against the active `RouterService` (returns status, headers, byte count, and UTF-8 preview capped at 16 KiB).

### Dynamic State & Config Mutation (Disabled Under `--mcp-read-only`)

- `d_set_plugin_config`: `{ "id": "...", "config"?: {...}, "merge"?: true, "disabled"?: bool, "create_if_missing"?: bool, "target_name"?: "..." }`. Merges fields cleanly by default via `merge_config_values`, preserving untouched options.
- `d_toggle_plugin`: `{ "id": "...", "enabled": bool, "remove"?: false, "target_name"?: "...", "config"?: {...} }`. Enables, disables, removes, or inserts catalog plugins dynamically.
- `d_restart_plugin`: `{ "id": "..." }`. Restarts a managed plugin fiber in place.
- `d_apply_patches`: `{ "patches": [ ... ] }`. Supports `op = "insert"` (with optional `anchor: { "before": id } | { "after": id }`), `op = "remove"`, and `op = "replace_config"`, plus `EntryTarget::Name` or `EntryTarget::Group` with nested `children`.
- `d_apply_manifest`: `{ "entries": [ ... ] }`. Atomically replaces the managed plugin entry tree (MCP itself runs on a control-plane fiber above `Loader`, so manifest replacements never sever the MCP connection).
- `d_set_log_level`: `{ "level": "error" | "warn" | "info" | "debug" | "trace" }`. Changes runtime logging verbosity immediately without restarting HTTP.
- `d_shutdown` *(requires `--mcp-allow-shutdown`)*: Requests graceful shutdown and two-phase L4 connection drain.

## 3. MCP Resources (4)

- `d://state` (`application/json`) — same summary as `d_server_info`.
- `d://catalog` (`application/json`) — same live schema catalog as `d_catalog`.
- `d://entries` (`application/json`) — managed entries (redacted) and loader states.
- `d://fibers` (`application/json`) — full sanitized `FrameSnapshot` fiber tree.

## 4. Common Live Adjustment Recipes

- **Enable CORS with a custom origin**:
  `d_toggle_plugin` `{ "id": "ext-cors", "enabled": true, "config": { "allow_origin": "https://app.example" } }`
- **Switch served directory & reveal dotfiles**:
  `d_set_plugin_config` `{ "id": "app-files", "config": { "root": "/path/to/new/root", "allow_hidden": true } }`
- **Bind or rebind HTTP listener at runtime (even from `--no-http`)**:
  `d_set_plugin_config` `{ "id": "proto-http", "config": { "bind_addr": "127.0.0.1:8090" } }` (or `"bind_addr": null` to unbind while keeping in-memory routing active)
- **Adjust compression threshold or disable compression**:
  `d_set_plugin_config` `{ "id": "ext-compression", "config": { "min_size": 1024 } }` or `d_toggle_plugin` `{ "id": "ext-compression", "enabled": false }`
- **Mount / tune gateway plugins dynamically across the 17 cataloged plugins**:
  Use `d_catalog` first to inspect live schemas for `l4-tls`, `proto-ws`, `proto-tcp`, `ext-proxy` (`upstreams`, `http_routes`, `tcp_routes`), `ext-observability` (`/metrics`, `/healthz`, `/readyz`), `ext-ratelimit`, `ext-auth` (`rules` marked `sensitive`), `ext-dev-kit` (`mocks`, `live_reload`, `inspect_path`), `app-files` (`mount_prefix`, `host`), and `doc-render` (`sanitize_html`), then apply via `d_set_plugin_config` or `d_toggle_plugin`.
- **Tune HTTP parser limits or L4 drain windows**:
  Inspect exact field names in `d_catalog` (`max_request_line_bytes`, `max_headers`, `max_header_bytes`, `max_body_bytes`, `idle_timeout_ms`, `short_drain_ms`, `long_drain_ms`), then patch with `d_set_plugin_config`.

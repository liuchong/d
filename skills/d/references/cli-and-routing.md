# CLI Flags, Environment Variables & Routing Contract

## 1. Core HTTP & Runtime Options

| CLI Flag | Env Var | Default | Description |
| :--- | :--- | :--- | :--- |
| `-H, --host <HOST>` | `D_HOST` | `localhost` | Listening host |
| `-p, --port <PORT>` | `D_PORT` | `8080` | Listening port |
| `-r, --root <PATH>` | `D_ROOT` | `.` | Served root directory (canonicalized sandbox) |
| `-l, --log <LEVEL>` | `RUST_LOG` | `info` | `error` \| `warn` \| `info` \| `debug` \| `trace` |
| `--hidden` | `D_SHOW_HIDDEN` | `false` | Allow serving and listing dotfiles |
| `--cors` | `D_CORS` | `false` | Enable permissive CORS headers and `OPTIONS` 204 |
| `--no-http` | `D_NO_HTTP` | `false` | Boot router in memory without binding TCP socket until configured via MCP |
| `-h, --help` | — | — | Print help and exit |
| `-V, --version` | — | — | Print version and exit |

CLI arguments override environment variables when both are provided.

## 2. Optional MCP Control Options

Available on `d` by default (`--features mcp`) and optionally compiled into `d-minimal` (`--features mcp`). All MCP endpoints are **disabled** unless explicitly requested:

| CLI Flag | Env Var | Default | Description |
| :--- | :--- | :--- | :--- |
| `--mcp-stdio` | `D_MCP_STDIO` | `false` | Enable line-delimited JSON-RPC 2.0 MCP on stdin/stdout (routes logs to `stderr`) |
| `--mcp-http <ADDR>` | `D_MCP_HTTP` | unset | Bind Streamable HTTP JSON-RPC 2.0 endpoint on `<host>:<port>` |
| `--mcp-path <PATH>` | `D_MCP_PATH` | `/mcp` | URL path for HTTP MCP `POST` requests (must start with `/`) |
| `--mcp-token <TOKEN>` | `D_MCP_TOKEN` | unset | Require `Authorization: Bearer <TOKEN>` for HTTP MCP calls |
| `--mcp-read-only` | `D_MCP_READ_ONLY` | `false` | Expose only inspection, validation, and probing tools |
| `--mcp-allow-shutdown` | `D_MCP_ALLOW_SHUTDOWN` | `false` | Enable the `d_shutdown` tool |

## 3. Conflict-Free Route & Query Contract

| Target | Bare Path | `?view=raw` | `?view=preview` | `?view=viewer` | `?view=download` | `?listing=true` |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| Directory | `301 -> /dir/` or `index.html` or listing | `index.html` / listing | `index.html` / listing | `index.html` / listing | `404` / file fallback | Forces white HTML directory index (`?sort=name\|size\|time`, `?hidden=bool`) |
| Markdown (`.md`) | White reader HTML | Raw UTF-8 Markdown | Body-only HTML | Reader HTML | Attachment (`RFC 5987`) | Default file view |
| Code / Text | Server-highlighted HTML | Raw source bytes | Highlighted body HTML | Highlighted reader HTML | Attachment (`RFC 5987`) | Default file view |
| Image / Audio / Video | **Raw binary stream** (`Range`/`ETag`/`304`) | Raw stream | Raw stream | White media viewer HTML embedding bare stream | Attachment (`RFC 5987`) | Raw stream |
| Binary | Raw stream (`200`/`206`/`304`) | Raw stream | Raw stream | Raw stream | Attachment (`RFC 5987`) | Raw stream |

<p align="center">
  <img src="docs/pages/assets/logo.svg" alt="d logo" width="160">
</p>

# d

[![1PL licensed](https://img.shields.io/badge/license-1PL-blue.svg)](./LICENSE)
[![crates.io](https://meritbadge.herokuapp.com/d)](https://crates.io/crates/d)

D is a modular, low-overhead standalone network daemon and file/document server built on an internal plugin microkernel (`frame`), a custom HTTP/1.1 streaming engine, and pure-Rust server-side Markdown and syntax rendering.

## Features

- **Layered plugin architecture**: Clean separation across Layer 0 (`frame` microkernel), Layer 1 (`l4-transport` with two-phase graceful drain), Layer 2 (`proto-http` HTTP/1.1 parser, chunked zero-buffer streaming, and lock-free snapshot routing), Layer 3 (`ext-compression`, `ext-cors`), and Layer 4 (`app-files`, `viewer-*`).
- **Built-in CommonMark + GFM & native embedded HTML**: Zero-dependency pure-Rust `doc-render` supporting headings with CJK/ASCII anchors, aligned tables, task lists, strikethrough, footnotes, and unescaped block/inline embedded HTML (`<details>`, `<figure>`, `<img>`, `<table>`, `<kbd>`).
- **Server-side lexical code highlighting**: Built-in tokenizer and light stylesheet for Rust, JavaScript/TypeScript, Python, Go, C/C++, Bash, JSON, YAML, TOML, SQL, HTML, and CSS — zero external scripts or CDNs.
- **Minimalist white interface**: Offline self-contained white canvas (`#ffffff` / `#f6f8fa` / `#0969da`), no dark themes, no colored badges, no external assets, and no JavaScript required.
- **Conflict-free media and file routing**:
  - Bare media/asset paths (`/img.svg`, `/clip.mp4`, `/style.css`, `/app.js`) always stream raw bytes with full `Range` (`206`/`416`), `ETag`/`Last-Modified` (`304`), and `HEAD` support.
  - Explicit `?view=viewer` opens a minimalist white media viewer page embedding the bare stream; `?view=raw`, `?view=preview`, and `?view=download` (RFC 5987 UTF-8 `Content-Disposition`) work across documents and files.
  - Directories without a trailing slash redirect via `301` (preserving query parameters) so relative images and links always resolve cleanly.
- **Static sites & directory controls**: Automatic sandboxed `index.html` serving (`?listing=true` bypass), sorting (`name`, `size`, `time`, `type`), and permission-gated hidden file toggle (`--hidden`).
- **Streaming-aware compression & opt-in CORS**: `brotli`, `gzip`, and `deflate` negotiation that automatically exempts SSE/streaming bodies, `206`, `304`, `HEAD`, pre-compressed media, and tiny payloads; CORS disabled by default (`--cors`).

## Usage

### Install

```bash
cargo install d
# or
cargo install --git https://github.com/liuchong/d
```

### Run

```bash
# Serve current directory
d

# Custom port and root
d -p 8080 -r /path/to/serve

# Allow toggling hidden dotfiles and permissive CORS
d -H 0.0.0.0 -p 8080 -r ./public --hidden --cors
```

### Options

| Option | Description | Default |
|--------|-------------|---------|
| `-H, --host` | Listening host | `localhost` |
| `-p, --port` | Listening port | `8080` |
| `-r, --root` | Root directory to serve | `.` |
| `-l, --log` | Log level (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `--hidden` | Allow showing hidden files | disabled |
| `--cors` | Enable permissive CORS (`GET`/`HEAD`/`OPTIONS`) | disabled |

### Environment Variables

- `D_HOST` - Server host
- `D_PORT` - Server port
- `D_ROOT` - Root directory
- `D_SHOW_HIDDEN` - Allow showing hidden files (`true`/`1`)
- `D_CORS` - Enable permissive CORS (`true`/`1`)
- `RUST_LOG` - Log level

## Web Interface & Query Parameters

Directory queries:

- `?sort=name|size|time|type` - Sort directory entries
- `?order=desc` - Reverse sort order
- `?hidden=true|false` - Toggle hidden dotfiles (when `--hidden` is enabled)
- `?listing=true` - View directory listing even when `index.html` exists

File queries:

- `?view=raw` - Serve raw unmodified bytes
- `?view=preview` - Render pure preview HTML without chrome
- `?view=viewer` - Open minimalist white viewer page (especially for images, audio, and video)
- `?view=download` - Force attachment download with RFC 5987 filename encoding

## Examples

Ready-to-run demo directories live in [`examples/`](./examples) — see [examples/README.md](./examples/README.md) for guided tours of Markdown with embedded HTML and relative images, syntax highlighting, media viewing, and static sites.

## Testing & Acceptance

```bash
cargo test --workspace   # unit, contract, and integration tests
scripts/acceptance.sh    # end-to-end HTTP, Markdown, and UI acceptance checks
```

## License

This project is licensed under the [One Public License (1PL)](./LICENSE).

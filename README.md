# d

[![1PL licensed](https://img.shields.io/badge/license-1PL-blue.svg)](./LICENSE)
[![crates.io](https://meritbadge.herokuapp.com/d)](https://crates.io/crates/d)

D is a simple standalone httpd with modern features.

## Features

- 📁 **Directory listing** with rich file type icons and sorting
- 🎨 **Syntax highlighting** for code files
- 🖼️ **File preview** for images, videos, audio, markdown and code
- 📥 **Download/Raw/Preview** options for text files
- 🔍 **Breadcrumb navigation** with clickable paths
- 📊 **Multiple sort options** (name, size, time, type)
- 👁️ **Hidden files** toggle (configurable)
- 🗜️ **Compression** support (gzip, deflate, brotli)
- 🌐 **CORS** support
- 📡 **Range requests** for resumable downloads
- 🏷️ **ETag** and **cache control**
- 🖥️ **Graceful shutdown**

## Usage

### Install

```bash
cargo install d
# or
cargo install --git https://github.com/liuchong/d
```

### Run

```bash
# Basic usage
d

# Custom port
d -p 8080

# Custom root directory
d -r /path/to/serve

# Show hidden files (allows users to toggle)
d --hidden

# Full options
d -H 0.0.0.0 -p 8080 -r ./public --hidden
```

### Options

| Option | Description | Default |
|--------|-------------|---------|
| `-H, --host` | Listening host | `localhost` |
| `-p, --port` | Listening port | `8080` |
| `-r, --root` | Root directory to serve | `.` |
| `-l, --log` | Log level | `info` |
| `--hidden` | Allow showing hidden files | disabled |

### Environment Variables

- `D_HOST` - Server host
- `D_PORT` - Server port
- `D_ROOT` - Root directory
- `RUST_LOG` - Log level

## Web Interface

When accessing a directory, you can use query parameters:

- `?sort=name` - Sort by name (default)
- `?sort=size` - Sort by file size
- `?sort=time` - Sort by modification time
- `?sort=type` - Sort by file type
- `?hidden=true` - Show hidden files

## License

This project is licensed under the [One Public License (1PL)](./LICENSE).

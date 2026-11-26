# `d` 使用手册

> 本手册覆盖 `d` 的安装运行、命令行与环境变量、无冲突路由规则、内建 Markdown 与词法高亮、HTTP/1.1 与流式压缩语义，以及作为 Rust 库嵌入的完整用法。
> 相关设计文档：[架构总览](./architecture.md) · [`frame` 微内核](./frame.md) · [MCP 控制与 Skill](./mcp.md) · [示例集](../examples/README.md) · [README](../README.md)

---

## 一、产品定位与基本用法

`d` 是一个单二进制网络守护进程与零外链文件/文档服务器，由独立零依赖微内核 `frame`、自建 HTTP/1.1 流式引擎、流式感知扩展与纯 Rust 文档渲染/展示层装配而成。

`d` **不区分“开发模式”和“生产模式”**——运行在工程师本机支撑目录浏览、Markdown 预览、代码审阅和本地联调的场景，与线上静态托管一样属于 `d` 的核心正式场景，使用同一套 HTTP/1.1 状态机、路径沙箱与连接排空机制。

### 安装

```bash
# 从 crates.io 安装
cargo install d

# 从仓库源码安装最新 master
cargo install --git https://github.com/liuchong/d

# 或在本地仓库编译调试/发布二进制
cargo build
cargo build --release   # 输出到 target/release/d
```

### 快速启动

```bash
# 1. 服务当前目录，监听 http://localhost:8080
d

# 2. 指定监听地址、端口与根目录
d -H 0.0.0.0 -p 8080 -r ./public

# 3. 允许切换隐藏点文件并启用 CORS（GET / HEAD / OPTIONS）
d -r ./workspace --hidden --cors -l debug

# 4. 使用 --flag=value 写法
d --host=127.0.0.1 --port=3000 --root=./examples/docs

# 5. 使用环境变量启动（命令行参数优先级高于环境变量）
D_HOST=127.0.0.1 D_PORT=9000 D_ROOT=./docs D_SHOW_HIDDEN=true D_CORS=1 d

# 6. 使用声明式 JSON 清单启动网关或开发工作流，并用 SIGHUP 热重载
d -c examples/gateway/gateway.json
d -c examples/gateway/dev.json
kill -HUP <d_pid>

# 7. 从源码直接本地预览仓库自带的 Pages 站点
cargo run -- -r docs -p 8080
```

按 `Ctrl+C`（`SIGINT`）或发送 `SIGTERM` 时，`d` 会触发两阶段连接优雅排空：立即停止接受新连接，等待在途 HTTP 响应完成或达到排空窗口后有序释放插件树并退出。

---

## 二、命令行选项与环境变量

| 选项 | 环境变量 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- |
| `-H, --host <HOST>` | `D_HOST` | `localhost` | 监听主机名或 IP；解析类似 `localhost` 时优先选用 IPv4 地址 |
| `-p, --port <PORT>` | `D_PORT` | `8080` | 监听 TCP 端口（合法 `u16` 整数） |
| `-r, --root <PATH>` | `D_ROOT` | `.` | 服务根目录；启动时执行 `canonicalize` 规范化 |
| `-c, --config <PATH>` | `D_CONFIG` | 未指定 | 声明式 JSON 插件清单；配合 `SIGHUP` 执行全量预检与无中断增量热重载 |
| `-l, --log <LEVEL>` | `RUST_LOG` | `info` | 日志级别：`error`、`warn`、`info`、`debug`、`trace`（亦兼容 `warning` 与模块后缀写法） |
| `--hidden` | `D_SHOW_HIDDEN` | 关闭 | 允许显示点文件，并允许页面/请求通过 `?hidden=true\|false` 切换 |
| `--cors` | `D_CORS` | 关闭 | 对所有响应附加 `Access-Control-Allow-Origin: *`，并为 `OPTIONS` 预检返回 `204` |
| `--no-http` | `D_NO_HTTP` | 关闭 | 启动内存路由而不绑定 HTTP 端口（可稍后经 MCP 动态绑定 `proto-http.bind_addr`） |
| `--mcp-stdio` | `D_MCP_STDIO` | 关闭 | 开启 stdio MCP 控制面（日志自动切到 `stderr`） |
| `--mcp-http <ADDR>` | `D_MCP_HTTP` | 未开启 | 开启 Streamable HTTP MCP 端点 |
| `--mcp-path <PATH>` | `D_MCP_PATH` | `/mcp` | HTTP MCP 请求路径 |
| `--mcp-token <TOKEN>` | `D_MCP_TOKEN` | 无 | HTTP MCP Bearer 鉴权令牌 |
| `--mcp-read-only` | `D_MCP_READ_ONLY` | 关闭 | 禁用所有 MCP 写操作工具，仅保留检视/预检/探测 |
| `--mcp-allow-shutdown` | `D_MCP_ALLOW_SHUTDOWN` | 关闭 | 允许 MCP `d_shutdown` 工具 |
| `-h, --help` | — | — | 打印帮助信息并退出（返回码 `0`） |
| `-V, --version` | — | — | 打印版本信息（如 `d 0.1.1`）并退出（返回码 `0`） |

配置优先级与解析细节：

- **优先级**：命令行参数 > 环境变量 > 默认值。
- **布尔环境变量**：`D_SHOW_HIDDEN` 与 `D_CORS` 识别 `1`、`true`、`yes`、`on`（忽略大小写和前后空白）为开启，其余值视为关闭。
- **参数错误**：缺少参数值、端口非法、未知参数或根目录不存在时输出错误并以退出码 `2`（CLI 解析阶段）或 `1`（运行时启动阶段）退出。

---

## 三、无冲突路由与查询参数契约

为了让静态网站脚本、文档内嵌相对图片、媒体 `Range` 拖动、浏览器白底查看与命令行下载互不干扰，`d` 严格区分**裸路径请求**与**显式查询参数**：

### 1. 裸路径行为（不带 `?view=`）

| 资源类别 | 典型后缀 | 裸路径默认返回 | 设计原因 |
| :--- | :--- | :--- | :--- |
| **图片 / 音频 / 视频** | `.png` `.jpg` `.webp` `.svg` `.mp4` `.webm` `.mp3` `.flac` `.ogg` 等 | **直接返回原始字节流**（精确 MIME、`Range`、`ETag`） | 保证 `<img src>`、`<video src>`、`<audio src>` 和 `curl` 稳定可用，支持断点续传与进度拖动 |
| **前端资源 / 二进制** | `.html` `.htm` `.css` `.js` `.mjs` `.cjs` `.wasm` `.woff2` `.pdf` `.zip` `.bin` | **直接返回原始字节流** | 静态站点引用的 `.js` 不会被误包装成 HTML 查看页，可直接作为 `<script src>` 加载 |
| **Markdown 文档** | `.md` `.markdown` `.mkd` `.mdown` | **白底服务端渲染阅读页**（含极简顶栏 `Preview / Raw / Download`） | 浏览器打开即可阅读排版后的全文，离线无需任何外网脚本 |
| **源码 / 文本 / Org** | `.rs` `.py` `.go` `.ts` `.sh` `.sql` `.json` `.yaml` `.toml` `.txt` `.csv` `.org` 等 | **白底服务端高亮/等宽查看页** | 直接呈现语法着色或等宽正文，附带文件大小、语言、UTC 修改时间与一键切 Raw/Download |
| **目录（带 `/` 结尾）** | `/docs/`、`/` | 若目录内存在合法沙箱内 `index.html` 则直出，否则返回**极简白底目录索引** | 目录排在文件之前，无 emoji、无花哨徽标，聚焦文件名、大小与修改时间 |
| **目录（不带 `/` 结尾）** | `/docs`、`/sub?sort=size` | **返回 `301 Moved Permanently`** 跳转到 `/docs/`，完整保留查询参数 | 确保文档和网页中的相对路径 `./diagram.svg`、../assets 永远基于正确目录解析 |

### 2. 查询参数速查

#### 文件级查询参数

- `?view=raw`：强制以原始字节流与真实 MIME 返回文件内容（适合获取 `.md`/源码原文，或配合 `Range` 检查）。
- `?view=preview`：仅返回纯正文白底 HTML（不带顶部操作栏），便于嵌入 `<iframe>` 或单独预览文档排版。
- `?view=viewer`：显式打开极简白底查看器页面。对图片/音频/视频而言，查看器页内嵌入 `<img src="/裸路径">` / `<video controls src="/裸路径">` / `<audio controls src="/裸路径">`，既能在线浏览又保持媒体流可拖动。
- `?view=download`：强制触发下载，响应头携带标准及 RFC 5987 非 ASCII 文件名编码：
  `Content-Disposition: attachment; filename="..."; filename*=UTF-8''...`

#### 目录级查询参数

- `?listing=true`：即使目录下存在 `index.html`，也强制显示目录文件列表。
- `?sort=name|size|time|type`：按名称、大小、修改时间或类型排序（目录始终优先排在文件前）。
- `?order=asc|desc`：切换升序/降序。
- `?hidden=true|false`：切换隐藏文件显示（仅当服务端启动时开启了 `--hidden` / `D_SHOW_HIDDEN` 才生效；未授权时页面不显示开关，传入 `?hidden=true` 也不会暴露任何点文件，直接请求隐藏路径仍返回 `404`）。

---

## 四、高频使用场景

### 1. 日常阅读项目文档与代码

```bash
d -r . -p 8080
```

- 浏览器访问 `/README.md` 或 `/docs/usage.md` 即见纯净白底阅读页（主画布 `#ffffff`、代码块与表头 `#f6f8fa`、强调蓝 `#0969da`）。
- 文档中的相对图片（如 `![架构图](./diagram.svg)` 或 `assets/logo.png`）随裸路径自动直出；访问 `/docs` 会自动 `301` 规范化到 `/docs/`，避免相对路径偏移。
- 点击任意代码文件（如 `/crates/frame/src/lib.rs`）即享纯 Rust 服务端词法高亮，零客户端脚本依赖，内网/离线环境秒开。

### 2. 托管静态站点与本地 Pages 预览

```bash
# examples/static-site 或仓库自身的 docs/ 都包含 index.html
d -r docs -p 8080
```

- 访问 `http://localhost:8080/` 自动呈现 `index.html`，引用的 `.css`、`.js`、`.eli`、`.svg` 按裸路径准确返回对应 MIME。
- 需要查看目录内实际有哪些文件时，访问 `http://localhost:8080/?listing=true`。
- 若站点根目录下的 `index.html` 是指向根目录之外的符号链接，安全沙箱会自动拦截并回退为目录列表（决不泄漏沙箱外文件内容）。

### 3. 局域网共享与浏览器跨域联调

```bash
# 对局域网监听所有网卡，并允许前端页面跨域拉取资源
d -H 0.0.0.0 -p 8080 -r ./share --cors
```

- 默认配置下 CORS 关闭；启用 `--cors` 后，所有响应返回 `Access-Control-Allow-Origin: *`，且 `OPTIONS` 预检直接返回 `204 No Content`，通告允许 `GET, HEAD, OPTIONS`。

---

## 五、内建 Markdown、嵌入式 HTML 与语法高亮

所有渲染均由零外部依赖 crate `doc-render` 在服务端完成：

### 1. Markdown 与 GFM 支持范围

- 多级标题（`#` 到 `######`，自动生成唯一锚点 `id`，兼容英文、数字与中文字符，重复标题自动追加 `-1`、`-2` 后缀支持页内 `#anchor` 跳转）
- 列表（无序、有序、多层嵌套）与任务列表复选框（`- [x]` / `- [ ]` -> `<input type="checkbox" ... disabled>`）
- 表格及列对齐（`:---`、` :---:`、`---:` -> `style="text-align:..."`）
- 删除线（`~~text~~` -> `<del>`）、引用块（`>`）、分割线（`---`）、行内代码（`` `code` ``）、围栏代码块（```` ```lang ````）
- 脚注定义与引用（`[^1]` -> `id="fn-1"` / `id="fnref-1"`）与自动链接

### 2. 原生嵌入式 HTML 透传与安全净化

与把全文转义后再交由浏览器脚本解析不同，`doc-render` 在 Markdown 解析与渲染期原生保留块级与行内 HTML 标签（例如 `<details>`、`<summary>`、`<figure>`、`<figcaption>`、`<img>`、`<table>`、`<video>`、`<audio>`、`<kbd>`、`<div>` 及内联属性），非常适合技术手册常见的折叠细节、图文图注与多栏表格。面向公网或不受信内容时，可在 `doc-render` 插件配置中开启 `{"sanitize_html": true}` 启用内置零外依白名单 HTML 净化（剥离 `<script>`/`<iframe>`、事件处理器及 `javascript:` 链接），同时契约层保留 `MarkdownOptions::sanitizer` 自定义钩子。

### 3. 内建词法高亮覆盖语言

| 语法词法家族 | 识别的扩展名 / 语言标识 |
| :--- | :--- |
| Rust | `.rs` / `rust` |
| JavaScript / TypeScript | `.js` `.mjs` `.cjs` `.jsx` `.ts` `.tsx` / `javascript` `typescript` |
| Python | `.py` / `python` |
| Go | `.go` / `golang` |
| C 系列语法高亮 | `.c` `.cpp` `.cc` `.cxx` `.h` `.hpp` `.java` `.zig` `.swift` `.kt` `.scala` |
| Shell | `.sh` `.bash` `.zsh` `.fish` / `shell` |
| 结构化数据与配置 | `.json` `jsonc` `.yaml` `.yml` `.toml` |
| 查询与网页标记 | `.sql` `.html` `.htm` `.xml` `.svg` `.css` `.scss` |

代码块与单独源码查看页共用同一套浅色 CSS，输出体积小且无运行时闪烁。

---

## 六、HTTP/1.1 协议语义、压缩豁免与安全边界

- **字节范围（`Range`）**：支持 `bytes=start-end`、开放结尾 `bytes=start-`、后缀范围 `bytes=-N`；真正 `seek + take` 按需读取切片，返回 `206 Partial Content` 与 `Content-Range`；越界范围返回 `416 Range Not Satisfiable`（带 `Content-Range: bytes */len`）；格式错误则按 RFC 9110 忽略并返回完整 `200`。
- **条件缓存（`304 Not Modified`）**：自动计算基于文件大小与修改时间的 `ETag` 与 `Last-Modified`，完整支持 `If-None-Match`（精确匹配、弱比较 `W/`、`*`）和 `If-Modified-Since`（HTTP 日期严格解析），命中即返回 `304` 省去正文传输。
- **`HEAD` 请求**：返回完整实体头部（含精确 `Content-Length`）但不写入响应体。
- **自建 HTTP/1.1 解析防线**：内置请求行上限（默认 8 KiB，超限返回 `414`）、头部数量/总字节上限（默认 128 个 / 64 KiB，超限返回 `431`）、请求体上限（默认 64 MiB，超限返回 `413`）、不支持版本返回 `505`，以及空闲连接超时回收（默认 30 秒）。
- **流式感知压缩（`ext-compression`）**：支持 `Accept-Encoding` 的 `q` 值协商（优先级 `br` > `gzip` > `deflate`），支持逐块流式压缩并透传 `streaming: true` 实时响应；**自动豁免以下响应**：
  - `HEAD` 请求、`< 200` 状态码、`206 Partial Content`、`304 Not Modified`
  - 已携带 `Content-Encoding` 或 `Cache-Control: no-transform`
  - `text/event-stream`（SSE）、`video/*`、`audio/*`、`image/{png,jpeg,webp,avif,gif}`、压缩归档与 `woff`/`woff2` 字体
  - 静态响应体小于 256 字节（`min_size = 256`）的小负载
- **沙箱隔离**：所有路径先拒绝 NUL 字节与可疑 traversal 段，再经 `canonicalize` 验证真实磁盘路径严格位于服务根目录之内；点文件在未开启 `--hidden` 时无论目录列表还是直链访问一律隐匿返回 `404`；符号链接指向根目录之外时直接拒绝，防止信息泄漏。

---

## 七、作为 Rust 库嵌入

根包 `d` 向 Rust 工程暴露启动监听与零端口内存派发两种入口，完整复用相同的 5 层插件装配：

```rust
use d::{AppHandle, Options, create_app_with_options, start_with_options};
use d_http_api::{HttpRequest, Method};
use std::net::SocketAddr;

/// 方式 A：在指定 SocketAddr 上启动完整 HTTP/1.1 服务直到收到 SIGINT / SIGTERM
async fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    start_with_options(
        &addr,
        "./examples/demo",
        Options {
            allow_hidden: false,
            cors: true,
        },
    )
    .await;
    Ok(())
}

/// 方式 B：不绑定 TCP 端口，在内存中直接通过 RouterService 执行全链路请求派发
async fn run_in_memory() -> Result<(), Box<dyn std::error::Error>> {
    let app: AppHandle = create_app_with_options(
        "./examples/docs",
        Options {
            allow_hidden: false,
            cors: false,
        },
    )
    .await;

    let resp = app
        .dispatch(HttpRequest::new(Method::Get, "/guide.md?view=preview"))
        .await;
    assert_eq!(resp.status.as_u16(), 200);
    let html = String::from_utf8(resp.body.into_bytes().await?)?;
    assert!(html.contains("<article class=\"d-doc-body\">"));

    app.dispose().await?;
    Ok(())
}
```

需要自定义底层配置或挂载额外业务插件时，可使用 `d::build_runtime(root, options, bind_addr)`，并通过 `app.context()` 访问第 0 层微内核 `d_frame::Context`，或直接通过 `app.router()` 挂载自定义路由与过滤器。

---

## 八、`curl` 调试与验收测试

```bash
# 验证目录无尾斜杠 301（保留查询串）
curl -s -D - -o /dev/null "http://localhost:8080/docs?view=raw"

# 验证协商压缩（Brotli / Gzip）与 Range 206 自动豁免
curl -s -D - -o /dev/null -H "Accept-Encoding: br, gzip" "http://localhost:8080/README.md"
curl -s -D - -o /dev/null -H "Accept-Encoding: br, gzip" -r 0-15 "http://localhost:8080/data.bin"

# 验证 CORS 预检（需启动时加 --cors）
curl -s -D - -o /dev/null -X OPTIONS \
  -H "Origin: https://example.com" \
  -H "Access-Control-Request-Method: GET" \
  "http://localhost:8080/data.bin"

# 运行架构门禁与全套工作区测试、端到端验收脚本
cargo xtask check
cargo xtask info
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p d-minimal
scripts/acceptance.sh
```

---

## 九、网关清单、反向代理与开发工作流套件（`-c` / `SIGHUP`）

通过 `-c <PATH>`（或 `D_CONFIG`）可直接装配全栈 17 个内置插件，无“开发模式 / 生产模式”内部切换：

- **传输与纯 Rust TLS（`l4-transport` / `l4-tls`）**：支持 `max_connections`、`ListenOptions` 多监听、`TlsAcceptor` SNI/ALPN 与 `Loader` 原子热换证（旧 TLS 长流不中断）。
- **协议引擎（`proto-http` / `proto-ws` / `proto-tcp`）**：支持 `:param` / `*rest` 路径参数路由、大包与 chunked 请求体流式透传（超 `max_body_bytes` 返回 `413`）、`101 Upgrade` 移交、纯 Rust RFC 6455 WebSocket 引擎及原生 TCP 端口转发。
- **通用反向代理（`ext-proxy`）**：HTTP/1.1 Keep-Alive 连接池、`round_robin` / `least_conn` / `weighted_round_robin` 负载均衡、主被动健康检查与熔断、`Forwarded`/`X-Forwarded-*` 头注入、`strip_prefix`、零缓冲 SSE 穿透、WebSocket `101` 双向隧道与 L4 TCP 透传。
- **纯 `std` 流量治理（`ext-observability` / `ext-ratelimit` / `ext-auth`）**：访问日志、`GET /metrics`、`/healthz`、`/readyz`（异常返 `503`）、按 IP 令牌桶限流（`429` + `Retry-After`）以及常量时间 `Bearer` / `Basic` / IP CIDR 鉴权（`401`/`403`）。
- **开发工作流与多前缀文件挂载（`ext-dev-kit` / `app-files`）**：支持静态与延迟 SSE Mock、零外链 Live Reload、白底零 CDN `/_d/inspect`（含 SSE 时间线与一键重放），以及同进程按 `mount_prefix` 与 `host` 挂载多个目录（参见 [examples/gateway/README.md](../examples/gateway/README.md)）。
- **安全热重载（`SIGHUP`）**：修改清单后执行 `kill -HUP <pid>`；若语法或配置校验失败则整批拒载，校验通过则仅重建变更分支且保持无关 SSE/WS 长连接不断开。

---

## 十、可选 MCP 动态控制与随版本演进 Skill

`d`（默认带 `mcp` 特性）与 `d-minimal`（支持 `--features mcp`）内置可选纯 Rust `d-mcp` 控制面，默认关闭，传入 `--mcp-stdio` 或 `--mcp-http <ADDR>` 后启用：

```bash
# 仅跑内存路由并通过 stdio 提供 MCP 控制
d -r ./docs --no-http --mcp-stdio --mcp-allow-shutdown

# 同时托管 HTTP 与受 Bearer Token 保护的 HTTP MCP
d -H 127.0.0.1 -p 8080 -r ./public --mcp-http 127.0.0.1:18081 --mcp-token secret
```

全部 12 项控制工具（`d_catalog`、`d_inspect`、`d_validate_patches`、`d_http_probe`、`d_set_plugin_config`、`d_toggle_plugin`、`d_restart_plugin`、`d_apply_patches`、`d_apply_manifest`、`d_set_log_level`、`d_shutdown`）、4 项资源及与 `skills/{d,d-dev}` 协同演进的完整规范请查阅 **[MCP 控制文档](./mcp.md)**。

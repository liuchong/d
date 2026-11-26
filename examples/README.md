# d 使用示例集

> 相关文档：[README](../README.md) · [完整使用手册](../docs/usage.md) · [架构设计](../docs/architecture.md)

每个子目录都是一个可直接运行的示范。先构建：

```bash
cargo build          # 生成 target/debug/d
```

## 示例总览

| 目录 | 演示内容 | 启动命令 |
|------|----------|----------|
| `demo/` | 综合演示：目录列表、排序、各类白底文件查看器 | `d -r examples/demo` |
| `code/` | 服务端词法高亮（Rust/Python/JS/Go/Shell/SQL） | `d -r examples/code` |
| `docs/` | Markdown（GFM/嵌入式 HTML/相对图片）、Org/TXT/CSV | `d -r examples/docs` |
| `media/` | 媒体直链原流与 `?view=viewer` 白底查看页（SVG/音视频） | `d -r examples/media` |
| `static-site/` | 静态网站（HTML+CSS+JS） | `d -r examples/static-site` |
| `gateway/` | 声明式 JSON 配置（观测/限流/鉴权/Mock/SSE/Live Reload/`SIGHUP` 热重载） | `d -c examples/gateway/gateway.json` |

启动后用浏览器打开 <http://localhost:8080>。若要同时预览仓库主页与 Pages 站点源码编译效果，可运行 `cargo run -- -r docs`。

## 示例 1：综合演示 `demo/`

```bash
cargo run -- -r examples/demo
```

看点：

- 极简白底目录列表：文件类别微标、大小、修改时间
- 排序：点击 Name / Size / Time / Type
- `README.md` 服务端渲染为白底阅读页；`hello.rs` 服务端语法高亮
- `subdir/` 演示无尾斜杠自动 `301` 规范化与面包屑导航

## 示例 2：服务端语法高亮 `code/`

```bash
cargo run -- -r examples/code
```

依次访问 `app.py`、`main.go`、`build.sh`、`query.sql` 或 `server.js?view=viewer`，
观察零外部脚本的纯 Rust 服务端浅色高亮效果（裸路径 `/server.js` 保持原 JS 流以兼容静态网站）。

## 示例 3：文档、嵌入式 HTML 与相对图片 `docs/`

```bash
cargo run -- -r examples/docs
```

`guide.md` 演示标准 GFM 表格、复选框、脚注、中英文标题锚点、原生嵌入式 `<details>`/`<figure>`/`<table>` 以及相对图片 `./diagram.svg`；
`inventory.csv`、`release-notes.txt` 演示白底等宽预览。

## 示例 4：媒体直链与白底查看器 `media/`

```bash
cargo run -- -r examples/media
```

在目录页点击图片、音频或视频将通过 `?view=viewer` 打开白底媒体查看页；直接请求 `/logo.svg` 或 `/icon.svg` 裸路径永远返回原始二进制流，确保 `<img src>`、`curl` 与 `Range` 拖动稳定无冲突。

## 示例 5：静态站点 `static-site/`

```bash
cargo run -- -r examples/static-site
# 打开 http://localhost:8080/
```

目录下存在 `index.html` 时会直接作为站点首页托管（支持 `Range`/`ETag`），
访问 `/` 即打开站点。想看文件列表时用 `?listing=true` 绕过：
<http://localhost:8080/?listing=true>。

## 示例 6：网关、反向代理与开发工作流 `gateway/`

```bash
cargo run -- -c examples/gateway/gateway.json
# 或本地开发提效套件（Mock / SSE / Live Reload / /_d/inspect）
cargo run -- -c examples/gateway/dev.json
```

参见 [examples/gateway/README.md](./gateway/README.md) 了解 `/healthz`、`/readyz`、`/metrics`、`ext-ratelimit`、`ext-auth`、`ext-proxy` 及 `SIGHUP` 无中断热重载验证方式。

## 命令行玩法（curl）

启动任意示例后：

```bash
# 断点续传 / 部分下载（206 Partial Content & 后缀范围）
curl -r 0-1023 http://localhost:8080/data.bin
curl -r -16 http://localhost:8080/data.bin

# 利用 ETag 缓存（第二次返回 304 Not Modified）
curl -sI http://localhost:8080/data.bin | grep -i etag
curl -H "If-None-Match: \"<上面的ETag>\"" -v http://localhost:8080/data.bin

# 获取文档/代码原文或纯预览 HTML（不带顶栏）
curl "http://localhost:8080/README.md?view=raw"
curl "http://localhost:8080/guide.md?view=preview"

# 打开媒体白底查看器页面，或直接下载文件（RFC 5987 文件名编码）
curl "http://localhost:8080/logo.svg?view=viewer"
curl -OJ "http://localhost:8080/README.md?view=download"

# 验证目录无尾斜杠 301 重定向并保留查询参数
curl -i "http://localhost:8080/subdir?sort=size"

# 开启隐藏文件与跨域头（CORS 预检测试）
cargo run -- -r examples/demo --hidden --cors
curl -i -X OPTIONS -H "Origin: https://example.com" -H "Access-Control-Request-Method: GET" http://localhost:8080/README.md
```

## 验收测试

仓库自带自动化验收脚本（构建 + 启动 + 全套 HTTP/Markdown/UI 行为检查）：

```bash
scripts/acceptance.sh
```

全部通过时输出 `All acceptance checks passed ✔`，任何一项失败都会以红色 FAIL 标出并以非零状态码退出。

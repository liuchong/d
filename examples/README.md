# d 使用示例集

每个子目录都是一个可直接运行的示范。先构建：

```bash
cargo build          # 生成 target/debug/d
```

## 示例总览

| 目录 | 演示内容 | 启动命令 |
|------|----------|----------|
| `demo/` | 综合演示：目录列表、排序、各类文件预览 | `d -r examples/demo` |
| `code/` | 多语言语法高亮（Rust/Python/JS/Go/Shell/SQL） | `d -r examples/code` |
| `docs/` | 文档预览（Markdown/Org/TXT/CSV） | `d -r examples/docs` |
| `media/` | 图片预览（SVG） | `d -r examples/media` |
| `static-site/` | 静态网站（HTML+CSS+JS） | `d -r examples/static-site` |

启动后用浏览器打开 <http://localhost:8080>。

## 示例 1：综合演示 `demo/`

```bash
cargo run -- -r examples/demo
```

看点：

- 目录列表：文件类型图标、大小、修改时间
- 排序：点击 Name / Size / Time / Type
- `README.md` 渲染为排版页面；`hello.rs` 带语法高亮
- `subdir/` 演示面包屑导航与 `..` 返回

## 示例 2：语法高亮 `code/`

```bash
cargo run -- -r examples/code
```

依次点击 `app.py`、`server.js`、`main.go`、`build.sh`、`query.sql`，
观察不同语言的高亮效果。

## 示例 3：文档预览 `docs/`

```bash
cargo run -- -r examples/docs
```

`guide.md` 是完整的使用指南（也是 Markdown 渲染示例）；
`inventory.csv`、`release-notes.txt` 演示纯文本等宽预览。

## 示例 4：图片预览 `media/`

```bash
cargo run -- -r examples/media
```

点击 `logo.svg` / `icon.svg` 直接查看图片。

## 示例 5：静态站点 `static-site/`

```bash
cargo run -- -r examples/static-site
# 打开 http://localhost:8080/
```

目录下存在 `index.html` 时会直接作为站点首页 serve（支持 Range/ETag），
访问 `/` 即打开站点。想看文件列表时用 `?listing=true` 绕过：
<http://localhost:8080/?listing=true>。

## 命令行玩法（curl）

启动任意示例后：

```bash
# 断点续传 / 部分下载
curl -r 0-1023 http://localhost:8080/data.bin

# 利用 ETag 缓存（第二次返回 304）
curl -sI http://localhost:8080/data.bin | grep -i etag
curl -H "If-None-Match: \"<上面的ETag>\"" -v http://localhost:8080/data.bin

# 直接下载文本文件（而不是显示查看器）
curl -OJ "http://localhost:8080/README.md?view=download"
```

## 验收测试

仓库自带自动化验收脚本（构建 + 启动 + 全套 HTTP 行为检查）：

```bash
scripts/acceptance.sh
```

全部通过时输出 `All acceptance checks passed ✔`，任何一项失败都会
以红色 FAIL 标出并以非零状态码退出，可直接用于 CI 或发布前检查。

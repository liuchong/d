# `d-mcp` 可选 MCP 控制面与随项目演进 Skill 套件

> 相关文档：[使用手册](./usage.md) · [架构总览](./architecture.md) · [`frame` 微内核](./frame.md) · [`skills/d`](../skills/d/SKILL.md) · [`skills/d-dev`](../skills/d-dev/SKILL.md)

`d-mcp`（`crates/entries/mcp`）为 `d` 与 `d-minimal` 提供**可选、可配置、纯 Rust 零外部 JSON/MCP 库依赖**的模型上下文协议（MCP）控制面，让外部客户端或 AI 助手在运行时自发现插件与配置 Schema，并动态安全地调整服务状态与一切可配置参数。

---

## 一、编译开关与运行时安全边界

- **主装配体 `d`**：默认启用 `mcp` feature（可用 `cargo build -p d --no-default-features` 裁剪关闭）。
- **轻量装配体 `d-minimal`**：默认不编入 `d-mcp` 以保持最小静态体积；需要时可通过 `cargo build -p d-minimal --features mcp` 按需开启。
- **默认完全静默**：即使二进制内置了 `mcp` 特性，只要不显式传入 `--mcp-stdio` / `--mcp-http`（或对应 `D_MCP_*` 环境变量），就不会打开任何 MCP 端口或通道。
- **只读与关停隔离**：
  - `--mcp-read-only`（`D_MCP_READ_ONLY=true`）：仅暴露只读检视、试跑预检与 HTTP 探测工具，隐藏并拒绝一切变更接口。
  - `--mcp-allow-shutdown`（`D_MCP_ALLOW_SHUTDOWN=true`）：显式授权后才暴露 `d_shutdown` 进程级优雅退出工具。
  - `--mcp-token <TOKEN>`（`D_MCP_TOKEN`）：为 Streamable HTTP 模式强制执行常量时间比较的 `Authorization: Bearer <TOKEN>` 鉴权，且该字段在自省快照中自动脱敏为 `"***REDACTED***"`。

---

## 二、命令行参数与环境变量

| 开关 | 环境变量 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- |
| `--mcp-stdio` | `D_MCP_STDIO` | 关闭 | 通过 stdin/stdout 提供按行分隔的 JSON-RPC 2.0 MCP 服务，日志自动切往 `stderr` |
| `--mcp-http <ADDR>` | `D_MCP_HTTP` | 未开启 | 在独立 TCP 地址上绑定 Streamable HTTP JSON-RPC 2.0 端点 |
| `--mcp-path <PATH>` | `D_MCP_PATH` | `/mcp` | HTTP MCP `POST` 路由路径（须以 `/` 开头） |
| `--mcp-token <TOKEN>` | `D_MCP_TOKEN` | 无 | HTTP MCP 鉴权 Bearer Token |
| `--mcp-read-only` | `D_MCP_READ_ONLY` | 关闭 | 只读模式 |
| `--mcp-allow-shutdown` | `D_MCP_ALLOW_SHUTDOWN` | 关闭 | 允许调用 `d_shutdown` |
| `--no-http` | `D_NO_HTTP` | 关闭 | 启动阶段不立即绑定 HTTP TCP 端口（内存路由仍可由 `d_http_probe` 访问，后续可用 `d_set_plugin_config` 动态绑定 `proto-http.bind_addr`） |

---

## 三、启动示例与客户端接入

### 1. Stdio 模式（由客户端拉起专属实例）

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

### 2. Streamable HTTP 模式（挂载到长期运行实例）

```bash
d -H 127.0.0.1 -p 8080 -r ./public \
  --mcp-http 127.0.0.1:18081 \
  --mcp-path /mcp \
  --mcp-token "replace-with-strong-token" \
  --mcp-allow-shutdown
```

---

## 四、工具列表（12 项）与资源（4 项）

### 只读与自发现工具

1. `d_server_info`：返回当前装配体名（`d` / `d-minimal`）、版本、实时 HTTP `local_addr`、当前日志级别、MCP 策略与纤程状态统计。
2. `d_catalog`：实时读取 `LoaderService::catalog()`，列出当前装配体注册的全部插件名（主装配体 `d` 自动发现全部 17 个组件：含 `l4-tls`、`proto-ws`、`proto-tcp`、`ext-proxy`、`ext-observability`、`ext-ratelimit`、`ext-auth`、`ext-dev-kit` 等）、描述、必选/可选依赖与带类型的 `Plugin::schema()` 配置字段（新组件或新配置项自动出现，无需修改 `d-mcp`；`ext-auth.rules`、`l4-tls` 私钥等敏感字段自动标记 `sensitive`）。
3. `d_inspect`：返回完整实时拓扑、脱敏条目配置与脱敏 `FrameSnapshot`。
4. `d_validate_patches`：对 `EntryPatch` 变更清单执行无副作用 `preflight` 干跑校验，返回预期清单或结构化 `ValidationIssue`。
5. `d_http_probe`：向当前活的 `RouterService` 派发内存 HTTP 请求（支持自定义 `method`、`uri`、`headers`），返回状态码、响应头与正文预览（上限 16 KiB）。

### 动态状态与配置修改工具

6. `d_set_plugin_config`：对指定 `id` 合并（`merge=true`）或整块替换（`merge=false`）配置并触发对应插件/下游增量收敛；可选 `disabled` 与 `create_if_missing`。
7. `d_toggle_plugin`：按 `id` 启用、禁用或移除（`remove=true`）插件，不存在时自动从目录装配插入。
8. `d_restart_plugin`：就地重启指定 `id` 的插件纤程。
9. `d_apply_patches`：原子提交多条 `EntryPatch`（`insert` / `remove` / `replace_config`，支持锚点及 `Group` 嵌套）并等待收敛。
10. `d_apply_manifest`：原子整体替换受管插件清单并等待收敛（MCP 运行于独立控制面纤程，不会因清理数据面清单而断连）。
11. `d_set_log_level`：实时调整全局日志级别（`error` / `warn` / `info` / `debug` / `trace`）。
12. `d_shutdown`（需 `--mcp-allow-shutdown`）：触发优雅停机与两阶段连接排空。

### MCP 只读资源（`application/json`）

- `d://state` — 实时服务概要与地址状态
- `d://catalog` — 动态插件与 Schema 目录
- `d://entries` — 当前条目清单（敏感字段已脱敏）与 Loader 状态
- `d://fibers` — 脱敏纤程拓扑树

---

## 五、随项目一起演进的双 Skill 套件

仓库随源码同版维护两套结构化 Skill（由 `cargo xtask check` 校验入口与引用完整性）：

- **[`skills/d/SKILL.md`](../skills/d/SKILL.md)**（使用与运维侧）：指导使用者与 AI 严格按 `d_catalog` → `d_inspect` → `d_validate_patches` → 变更 → `d_http_probe` 闭环操作，永远以运行时 Schema 为准而非写死旧版本表。
- **[`skills/d-dev/SKILL.md`](../skills/d-dev/SKILL.md)**（开发与扩展侧）：规定新增组件必须同步实现 `Plugin::validate_config` 与 `Plugin::schema`（敏感字段标 `.sensitive()`），使 MCP 与 `skills/d` 随代码自然演进，并通过 `cargo xtask check/info/test` 与多特性门禁验证。

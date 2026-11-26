# 网关、流量治理与开发工作流示例 (`gateway/`)

> 相关文档：[示例总览](../README.md) · [使用手册](../../docs/usage.md) · [架构设计](../../docs/architecture.md)

本目录提供声明式 JSON 配置清单示例，通过 `d -c <PATH>`（或环境变量 `D_CONFIG`）直接装配启用，并支持修改文件后发送 `SIGHUP` 进行全量预检与增量热重载（不影响正在进行的无关 SSE / WebSocket 长连接）。

## 1. 网关 + 流量治理 + 静态文档配置（`gateway.json`）

```bash
cargo run -- -c examples/gateway/gateway.json
```

包含插件链路：

- `l4-transport` + `proto-http`（绑定 `127.0.0.1:8080`）+ `proto-ws`
- `ext-observability`：暴露 `/healthz`、`/readyz`（探针故障返回 `503`）与 `/metrics`（OpenMetrics 计数与延迟分桶），并输出 JSON 访问日志到 `stderr`
- `ext-ratelimit`：对 `/api/` 前缀按 IP 令牌桶限流（超限返回 `429` + `Retry-After`）
- `ext-auth`：对 `/admin/` 前缀启用 Bearer Token 校验（缺省返回 `401`）
- `ext-compression` + `app-files`（挂载 `./examples/docs`）+ `doc-render`（启用 `sanitize_html: true`）+ `viewer-*`

### 验证请求

```bash
curl -i http://127.0.0.1:8080/healthz
curl -i http://127.0.0.1:8080/readyz
curl -s http://127.0.0.1:8080/metrics | head -n 20
curl -i http://127.0.0.1:8080/admin/panel
curl -i -H "Authorization: Bearer replace-with-token" http://127.0.0.1:8080/admin/panel
```

### 叠加 L7/L4 反向代理（按需向 `"plugins"` 中添加）

```json
{
  "id": "ext-proxy",
  "config": {
    "upstreams": [
      {
        "name": "agent-backend",
        "lb": "least_conn",
        "backends": [{ "addr": "127.0.0.1:9000", "weight": 1 }],
        "active_health_check": { "interval_ms": 2000, "timeout_ms": 500, "path": "/healthz" }
      }
    ],
    "http_routes": [
      { "id": "api-proxy", "prefix": "/v1/", "strip_prefix": "/v1", "upstream": "agent-backend" }
    ]
  }
}
```

## 2. 本地开发工作流提效配置（`dev.json`）

```bash
cargo run -- -c examples/gateway/dev.json
```

开启 `ext-dev-kit`：

- 静态 JSON Mock `/mock/user` 与流式 SSE Mock `/mock/sse`（零缓冲逐块实时发送）
- 零外链 Live Reload（监听 `./examples/static-site` 变更并通过本机 WS `/_d/livereload` 推送刷新）
- 极简白底 `/_d/inspect`（含 SSE 分片到达时间线与请求一键重放）

## 3. `SIGHUP` 无中断热重载测试

运行任意 `-c` 清单时，修改 JSON 文件后向进程发送 `SIGHUP`：

```bash
kill -HUP <d_pid>
```

若新配置语法或字段校验失败，`d` 会记录错误并完整保留原有服务拓扑；校验通过则仅对变更节点执行局部重载，不中断正在进行的无关流式连接。

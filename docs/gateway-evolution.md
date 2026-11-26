# `d` 网关化、全协议插件生态与通用反代演进方案

> 基线提交：`master @ 095bcc140d86d621933563c883b055816188a48e`（21 个 Workspace 包 · 13 个零外依纯包 · `d` / `d-minimal` 双装配体 · 可选 `d-mcp` 控制面）。
> 原则：遵守单一演进、零历史兼容包袱（不保留失效双路径）、契约/实现隔离与 `xtask` 架构门禁。严禁引入重型第三方 Web 框架或前端外网脚本。

---

## 一、对标七大工程标准的现状与缺口总表

将问题清单作为唯一评判标尺，逐项盘点当前已具备能力、实际缺口与演进落点：

| 评估标准 | 当前已落地能力 | 核心缺口 | 本轮演进落点（阶段 `G1`–`G5`） |
| :--- | :--- | :--- | :--- |
| **1. 行业级 L4/L7 网关能力** | 第 0 层微内核（`Fiber`/`Effect`/`Scope`/`Loader`）、L4 两阶段连接排空、`proto-http` 无锁不可变快照路由、双向 `HttpFilter`、流式压缩、CORS、`d-mcp` 动态控管 | 缺 TLS/SNI/ALPN 传输终止；缺多监听地址与连接数上限；缺请求升级劫持与流式请求体；缺路径参数；缺 L7/L4 反向代理、限流/熔断、访问鉴权、结构化访问日志与指标；缺配置文件 + `SIGHUP` 热重载（HTTP/2 与 HTTP/3 划为后续独立协议里程碑，本轮保留 ALPN 扩展口） | `G1` 传输安全与多监听；`G2` 协议与路由增强；`G3` 通用反代；`G4` 观测/限流/鉴权与热重载；`G5` 门禁与交付 |
| **2. HTTP / WebSocket / Agent / TCP 插件式挂载** | HTTP 依赖 `RouterService` + `ScopedRouter::mount_route`；SSE 响应通过 `BodyKind::Stream { streaming: true }` 零缓冲直出；原生 TCP 可直接实现 `ConnectionHandler` 并由 `TransportService` 托管；Agent 控管已有 `d-mcp` | HTTP 缺路径参数 `:id`/`*rest` 与流式请求体；WebSocket 缺 `101 Switching Protocols` 连接升级劫持与 `ws-api` / `proto-ws` 引擎；TCP 缺声明式转发/装配插件 `proto-tcp`；缺一体化多协议 + Agent 联动端到端示范 | `G2` 补齐 `RouteMatch::Param`、请求体流式化、`HttpResponse::upgrade`、`ws-api`、`proto-ws`（纯 Rust RFC 6455）与 `proto-tcp`；`G5` 增加多协议示例 |
| **3. 通用反向代理** | `HttpRouter` 支持 `host` 匹配与 `Prefix` 路由；`BodyKind::Stream` 支持分块零缓冲响应写出 | 缺 `proxy-api` 与 `ext-proxy`；缺 HTTP/1.1 上游连接池、轮询/最少连接/加权负载均衡、主被动健康检查与熔断、头重写（`Forwarded`/`X-Forwarded-*`）与路径前缀剥离、SSE/WS 代理隧道及 L4 TCP 透传 | `G3` 落地 `proxy-api` + `ext-proxy`，全面支持 L7（HTTP + SSE + WebSocket 101）与 L4 代理并接入 `d-mcp` 热调整 |
| **4. 生产环境使用能力** | 两阶段优雅停机（短连接等待 + 长连接通知）、静态站点沙箱、编译与运行期可裁剪 `d-minimal`、`d-mcp` Token/只读开关 | 缺 `l4-tls`（HTTPS/TLS 终止与证书热更新）；缺 `-c/--config` 声明式配置文件与 `SIGHUP` 无损热重载；缺 `/healthz`、`/readyz`、`/metrics` 与访问日志（`ext-observability`）；缺限流 `ext-ratelimit` 与鉴权 `ext-auth`；`doc-render` 缺可配置 HTML 净化开关 | `G1` 落地 `l4-tls`；`G4` 落地配置与 `SIGHUP` 重载、三大治理扩展与 HTML 安全净化开关 |
| **5. 开发环境使用能力** | 白底离线预览、`--cors`/`--hidden`/环境变量、本地调试即可直接通过 `d-mcp` 动态切换参数与路径并做无网络回路 `d_http_probe` | 缺聚合反代 + 本地接口模拟（Mock 含 SSE）；缺零外链 Live Reload（自动重连 WebSocket 刷新）；缺白底实时请求 / SSE 分片时间线 / WS 双向帧检视器 | `G3` + `G4` 落地 `ext-dev-kit`（基于 `proto-ws` 与 `HttpFilter`，100% 离线白底、零外链） |
| **6. 目录式静态服务器深化** | `app-files` 已支持目录 `301` 补 `/`、`Range`/`ETag`/`304`/`HEAD`、RFC 5987、越界返回 `404`、白底离线查看页、动态切 `root` 预检 | `app-files` 固定占用 `CatchAll` 优先级 `-100` 且 `host: None`，不便于多根目录按虚拟主机或子前缀（如 `/docs`）挂载；缺对不可信 `.md` 内容配置化开启 HTML 净化 | `G4` 在 `app-files` 增加 `mount_prefix` 与 `host`，在 `doc-render` 实现 `sanitize_html` 内建纯 Rust 白名单净化 |
| **7. 纯 Rust 与极简依赖边界** | 21 个 crate 中 13 个纯 `std` 零外部依赖；`doc-render`、`proto-http`、`d-mcp`、`app-files` 核心协议与解析均已纯 Rust 自建；`d-minimal` 仅含 `tokio` | 新增 TLS、WebSocket、反代与治理插件后，必须以编译器 + `xtask check` 锁定纯包预算，防止引入重型 Web 框架或污染 `d-minimal` | 新增 `tls-api`/`ws-api`/`proxy-api` 与 `ext-{observability,ratelimit,auth}` 全部 **零外部依赖**；`proto-ws`/`proto-tcp`/`ext-proxy`/`ext-dev-kit` 仅依赖 `tokio`；纯 Rust TLS 协议栈隔离在可裁剪组件 `l4-tls`；`d-minimal` 零膨胀（仍仅 `tokio`） |

---

## 二、目标架构与工作区包规划

### 2.1 新增包与依赖预算（目标共 32 个 Workspace 包）

1. **新增契约包（3 个，层 `contracts`，全部零外部依赖 `zero_external_deps`）**
   - `crates/contracts/tls-api`：TLS 监听器/连接器契约、SNI 证书条目与证书热更新服务（`TlsAcceptorService`、`TlsConnectorService`）。
   - `crates/contracts/ws-api`：RFC 6455 抽象帧/消息/会话契约与路由注册表服务（`WsService`、`ScopedWsRegistry`、`WsHandler`）。
   - `crates/contracts/proxy-api`：上游目标、负载均衡策略、健康检查状态、熔断配置与上游池服务（`UpstreamPoolService`）。
2. **新增实现组件（8 个，层 `components`，严禁相互直连）**
   - `crates/components/l4-tls`（第 1 层）：纯 Rust TLS 协议栈实现 `tls-api`，提供证书预检与原子快照切换；**唯一新增外部密码学依赖所在包**，不进入 `d-minimal`。
   - `crates/components/proto-ws`（第 2 层）：零第三方 WS 库依赖（纯 `std` 手写 SHA-1、Base64、掩码与 RFC 6455 分帧状态机 + `tokio` 异步驱动），挂载 `101` Upgrade 与心跳/优雅关闭。
   - `crates/components/proto-tcp`（第 2 层）：仅依赖 `frame`、`net-api`、`tls-api`（可选）、`tokio`，支持多端口原始 TCP / TLS 透传与自定义字节处理插件。
   - `crates/components/ext-proxy`（第 3 层）：依赖 `frame`、`http-api`、`proxy-api`、`net-api`、`tls-api`（可选）、`tokio`，实现 L7 HTTP/1.1 + SSE + WebSocket 反代与 L4 TCP 上游转发、连接复用池、负载均衡及主被动健康检查。
   - `crates/components/ext-observability`（第 3 层）：**零外部依赖纯 `std`**，结构化访问日志（单行文本 / JSON）、原子指标与 `/metrics`、存活/就绪探针 `/healthz` 与 `/readyz`。
   - `crates/components/ext-ratelimit`（第 3 层）：**零外部依赖纯 `std`**，基于分桶令牌桶的按 IP / 前缀限流过滤器（`429 Too Many Requests` + `Retry-After`）。
   - `crates/components/ext-auth`（第 3 层）：**零外部依赖纯 `std`**，支持常量时间对比的 `Bearer` / `Basic` 认证与 IP CIDR 白黑名单过滤器（`401` / `403`）。
   - `crates/components/ext-dev-kit`（第 3 层）：仅依赖 `frame`、`http-api`、`ws-api`、`render-api`（可选）、`tokio`，提供白底 `/_d/inspect` 请求/SSE/WS 检视台、声明式 Mock 与零外链 Live Reload。

### 2.2 架构红线更新（`[workspace.metadata.architecture]`）

- `zero_external_deps` 从 **13 个扩展到 19 个**（新增 `tls-api`、`ws-api`、`proxy-api`、`ext-observability`、`ext-ratelimit`、`ext-auth`）。
- `minimal_forbidden` 增补：`l4-tls`、`proto-ws`、`proto-tcp`、`ext-proxy`、`ext-observability`、`ext-ratelimit`、`ext-auth`、`ext-dev-kit` 以及 TLS 密码学外部库，保证 `d-minimal` 依赖树除工作区纯包外仍**仅含 `tokio`**。
- `d-daemon` 与 `d-mcp` 继续维持**零 `crates/components/*` 直接依赖**，全部通过契约层与 `frame` 元数据驱动。

---

## 三、分模块详细设计

### 3.1 第 1 层传输、多监听与 TLS（`net-api` + `l4-transport` + `tls-api` + `l4-tls`）

1. **`crates/contracts/net-api` 与 `l4-transport`**
   - 新增 `ListenOptions`：
     - `pub addr: SocketAddr`
     - `pub max_connections: Option<usize>`（超过上限时立即关闭或礼貌拒绝，保护进程资源）
     - `pub tls_listener_id: Option<String>`（若指定且运行时存在 `TlsAcceptorService`，在握手阶段包装为加密 `BoxStreamIo`）
   - 新增 `TransportStatsSnapshot { active_connections: usize, total_accepted: u64, rejected_over_limit: u64 }`。
   - `Transport` 新增 `listen_with_options(&self, caller: &FrameContext, opts: ListenOptions, handler: Arc<dyn ConnectionHandler>)` 与 `stats(&self) -> TransportStatsSnapshot`；保留原有 `listen(caller, addr, handler)` 转发到默认 `ListenOptions`。
   - **关键不变量**：无论是普通 TCP 还是 TLS 封装流，连接接入时一律纳入统一 `Ledger` 与 `ConnGuard`，触发停机或监听重建时照常执行两阶段优雅排空。
2. **`crates/contracts/tls-api` 与 `l4-tls`**
   - `TlsAcceptor` 提供 `wrap_inbound(&self, listener_id: &str, raw: BoxStreamIo) -> BoxFuture<'static, io::Result<BoxStreamIo>>`（并记录协商后的 ALPN，供后续 HTTP/2 演进平滑识别，本阶段默认 `"http/1.1"`）。
   - `TlsConnector` 提供 `wrap_outbound(&self, server_name: &str, alpn: &[&str], raw: BoxStreamIo) -> BoxFuture<'static, io::Result<BoxStreamIo>>` 供 `ext-proxy` 连接 HTTPS 上游。
   - `TlsConfigPlugin` 在 `validate_config` 阶段完成 PEM 证书链与私钥解析及 SNI 主机名校验；热更新时通过原子快照交换新证书映射表，新连接立即使用新证书，已建立 TLS 会话持续运行不受影响。

### 3.2 第 2 层协议与路由底座（`http-api` + `proto-http` + `ws-api` + `proto-ws` + `proto-tcp`）

1. **`http-api` 与 `proto-http` 路径参数、流式请求与 Upgrade 劫持**
   - **路径参数匹配**：`RouteMatch` 扩充 `Param(String)`（例如 `"/api/v1/agents/:agent_id/runs/*tail"`）。`HttpRequest` 新增 `pub path_params: BTreeMap<String, String>`；不可变快照 `HttpRouter` 排序优先级保持 `Priority > Host > Exact > Param > Prefix > CatchAll`。
   - **大请求体与分块请求体流式化**：`proto-http/src/parser.rs` 当入站 `Transfer-Encoding: chunked` 或已知 `Content-Length > stream_body_threshold`（默认 `64 KiB`）时，不再整包攒入 `Vec<u8>`，而是通过 `http_api::stream_channel()` + 后台读泵喂入 `BodyKind::Stream`，并继续严格受 `max_body_bytes` 上限约束（防慢速/超大包内存打爆）。
   - **Upgrade 连接劫持出口**：
     - `http-api` 新增 `UpgradeHandler` trait：`fn on_upgrade(&self, io: BoxStreamIo, leftover: Vec<u8>, drain: Arc<dyn DrainSignal>) -> BoxFuture<'static, ()>`，以及 `HttpResponse::upgrade(status: StatusCode, headers: Headers, handler: Arc<dyn UpgradeHandler>)`。
     - `proto-http/src/writer.rs` 识别 `BodyKind::Upgrade(handler)` 时：立即调用 `drain.mark_long_lived()`，写出 `101 Switching Protocols`（或透传上游状态与握手头）并 flush，然后调用 `reader.take_io_and_leftover()` 将裸流及已预读缓冲字节整体移交 `handler.on_upgrade(...)`，退出 HTTP 请求循环（不重复关闭套接字）。
2. **`ws-api` 与 `proto-ws`（纯 Rust RFC 6455 引擎）**
   - 纯 Rust 实现 SHA-1 与 Base64（不引入额外依赖），严格校验 `Upgrade: websocket`、`Connection: Upgrade`、`Sec-WebSocket-Version: 13` 与 `Sec-WebSocket-Key`，生成 `Sec-WebSocket-Accept`。
   - 帧引擎严格处理 `FIN`/分片续传、`Text`/`Binary`/`Ping`/`Pong`/`Close`、客户端强制掩码解码、大帧长度上限校验（防 OOM）与写端有界通道 `mpsc`（背压满即显式关闭或返回错误）。
   - 注册形式：`WsRegistryService`（实现 `ScopedService`），任意组件通过 `ctx.use_service::<WsRegistryService>()?.mount_ws(spec, ws_handler)` 挂载，自动向 `RouterService` 注册握手路由，卸载纤程时自动摘除（已打开的长连接遵循两阶段排空通知 `Close` 帧）。
3. **`proto-tcp`（声明式 L4/TCP 引擎组件）**
   - 提供 `TcpService`（注册原生字节流处理器）与 `TcpEnginePlugin`，支持通过 `ConfigValue::List` 声明监听入口与端口转发目标，直接由 `TransportService::listen_with_options` 托管；接入自定义二进制协议插件或 L4 转发时无需经过 HTTP 状态机。

### 3.3 第 3 层通用反向代理与负载均衡（`proxy-api` + `ext-proxy`）

`ext-proxy` 同时支持 **L7（HTTP/1.1 + SSE + WebSocket 101 隧道）** 与 **L4（TCP 上游转发）**，完全融入 `frame` 控制面：

- **配置结构（均映射为 `ConfigValue` 并暴露完整 `Plugin::schema()`）**：
  - `upstreams`: `id -> { strategy: "round_robin" | "least_conn" | "weighted_round_robin", backends: [{ addr, weight, tls, sni }], max_idle_per_backend, connect_timeout_ms, response_header_timeout_ms, active_health_check: { interval_ms, timeout_ms, path, unhealthy_threshold, healthy_threshold }, passive_eject_threshold, passive_eject_ms }`
  - `http_routes`: `[{ id, host, match_prefix, strip_prefix, preserve_host, upstream, extra_headers, websocket, retry_idempotent }]`
  - `tcp_routes`: `[{ id, listen_addr, tls_listener_id, upstream }]`
- **关键工程特性**：
  1. **SSE 零缓冲穿透**：检测上游返回 `Content-Type: text/event-stream` 或 `Transfer-Encoding: chunked` 未知长度响应时，立即返回 `BodyKind::Stream { streaming: true, content_length: None }`，边读上游边写下游 `flush`。
  2. **WebSocket 升级反代**：当下游带 `Upgrade: websocket` 时，向上游建立 HTTP/1.1（或 TLS）连接、转发握手并收到 `101` 后返回 `HttpResponse::upgrade`，将上游连接作为双向隧道用 `tokio::io::copy_bidirectional` 拼接，双方均接入 `DrainSignal`。
  3. **连接池与健康摘除**：按上游节点维护 Keep-Alive 空闲连接池与活跃请求计数；主动探测 + 被动连续失败达到阈值立即熔断摘除，恢复后自动回池；配置热更新通过 `Loader::apply_patches` 原地更新上游池与路由，不杀在途长流或另一条无关路由的 WS 会话。

### 3.4 第 3 层生产治理与开发提效插件（`ext-observability`、`ext-ratelimit`、`ext-auth`、`ext-dev-kit`）

1. **`ext-observability`（纯 `std`，零外依）**
   - 高优先级双向 `HttpFilter`：记录请求方法、路径、Host、状态码、耗时微秒、下游与响应字节数及上游名称，支持 `format = "text" | "json"` 输出到日志或指定文件。
   - 维护无锁/细粒度原子计数器（`d_http_requests_total` 按状态段/路径前缀、`d_http_request_duration_ms_bucket` 固定桶直方图、`d_active_connections`、`d_proxy_upstream_healthy`）。
   - 注册内置端点：`/metrics`（OpenMetrics 文本格式）、`/healthz`（进程存活）、`/readyz`（读取 `ctx.inspect()` 与上游健康状态决定 `200` 或 `503`）。
2. **`ext-ratelimit`（纯 `std`，零外依）**
   - 令牌桶按规则（全局、按客户端 IP、按路径前缀）限流；周期性清理过期桶防内存泄漏；超限立即短回路返回 `429 Too Many Requests` + `Retry-After`。
3. **`ext-auth`（纯 `std`，零外依）**
   - 按路径前缀/Host 挂载规则：`Bearer` 令牌（常量时间比较，不出现在 `inspect`/日志中）、`Basic` 鉴权（纯 `std` Base64 解码与校验）、IPv4/IPv6 CIDR 白名单与黑名单。
4. **`ext-dev-kit`（开发工作流一等正式插件，零外网 CDN）**
   - **Mock 引擎**：按方法 + 路径挂载声明式静态 JSON/文本或分段延迟 **SSE 模拟流**，便于前端与 Agent 客户端脱机联调。
   - **Live Reload**：基于 `proto-ws` 暴露 `/_d/livereload`；若配置监听目录列表，采用纯 `std` / `tokio` 轮询或增量变更检测发出刷新消息；通过响应过滤器按需向 HTML 文档注入一段极短内联本机脚本，零外部脚本引用。
   - **白底请求与流式检视器（`/_d/inspect`）**：环形缓冲区保留最近 $N$ 条请求（默认 200 条），记录请求/响应头、耗时，专门记录 **SSE 分片逐块时间戳列表** 与 **WebSocket 双向帧摘要**，支持白底界面一键查看与重放（重放调用 `RouterService::dispatch`）。

### 3.5 第 4 层文件应用与渲染增强（`app-files` + `render-api` + `doc-render`）

- **`app-files`**：`FilesConfig` 增加 `pub mount_prefix: String`（默认 `"/"`）和 `pub host: Option<String>`。当设置 `mount_prefix = "/docs"` 时，自动挂 `RouteMatch::Prefix("/docs")`、内部剥离前缀后执行目录 301 重定向（跳转到 `/docs/sub/`）和沙箱解析，使同一进程可同时挂载多个静态根目录与多个反向代理路径，互不冲突。
- **`render-api` & `doc-render`**：`doc-render` 扩展配置字段 `sanitize_html: bool`（默认 `false` 保持本地可信目录完整能力）；当开启 `sanitize_html: true` 时，通过纯 Rust 内建白名单状态机过滤危险标签（`script`/`iframe`/`object`/`embed`/`style`/`form`）、事件属性（`on*`）及非安全协议链接（如 `javascript:`/`vbscript:`），适配公网/不可信 Markdown 场景。

### 3.6 声明式配置文件与 `SIGHUP` 热重载（`d-cli` + `d-daemon`）

- **单份声明式配置覆盖一切场景**：`d-cli` 新增 `-c, --config <PATH>`（环境变量 `D_CONFIG`）。配置文件采用规范 JSON（复用零外依纯 `std` 解析器将 JSON 无损映射为 `Vec<PluginEntry>`，直接送入 `Loader`）。
- **无停机 `SIGHUP` 热重载**：在 Unix 信号循环中监听 `SIGHUP`，重新读取并解析配置文件后先执行 `loader.validate_entries(&next_entries)` 全量预检；**预检失败则记录明确错误并 100% 保留原有运行态，决不部分卸载；预检通过再调用 `loader.apply(next_entries)` 执行增量 Diff**，实现新增路由、改配置、换 TLS 证书或增删反代节点均不断开无关长连接。
- **`d-mcp` 联动演进**：上述所有新组件全部实现完备的 `Plugin::schema()` 并将密钥字段标记 `secret: true`，无需改动 `d-mcp` 代码即可自动被 `d_catalog`/`d_inspect`/`d_set_plugin_config`/`d_apply_patches` 发现和热调控。

---

## 四、多协议插件挂载与双场景使用范式

### 4.1 四类业务插件在 `d` 上的标准挂载方式

| 业务形态 | 依赖契约服务 | 关键接口调用 | 资源自动回收保障 |
| :--- | :--- | :--- | :--- |
| **HTTP REST / API 插件** | `RouterService` | `router.mount_route(RouteSpec { matcher: RouteMatch::Param("/users/:id".into()), .. }, handler)` | 卸载插件纤程时对应路由从快照原子摘除 |
| **Agent SSE 流式插件** | `RouterService` | 返回 `HttpResponse::stream(200, "text/event-stream", rx, None, true)`；可选挂 `ext-proxy` 转发底层模型网关 | 流响应主动标记 `LongLived`，停机排空温和收尾，`ext-compression` 自动豁免 |
| **WebSocket IM / 实时插件** | `WsRegistryService` | `ws.mount_ws(spec, handler)`，通过 `WsSession::send`/`recv` 收发文本与二进制帧 | 控制帧 Ping/Pong/Close 自动托管；卸载时按排空窗口关闭长连接 |
| **原生 TCP / 自定义协议插件** | `TransportService` (+ `TlsAcceptorService`) | 实现 `ConnectionHandler`，调用 `transport.listen_with_options(opts, handler)` | `Listener` 与活跃连接台账绑定到调用方 `EffectHandle` |

### 4.2 生产环境与开发环境的统一落地方式

- **生产网关模式（推荐示例 `d -c /etc/d/gateway.json`）**：配置 `l4-tls`（证书路径 + SNI）、`proto-http`（443 TLS 监听）、`ext-auth`、`ext-ratelimit`、`ext-observability`（开启 JSON 访问日志与 `/metrics`、`/healthz`、`/readyz`）、`ext-proxy`（后端节点池与健康检查）以及可选的 `app-files`（开 `sanitize_html: true`）；如需内网动态运维，附加 `--mcp-http 127.0.0.1:9100 --mcp-token ...`；修改配置后 `kill -HUP <pid>` 或通过 MCP 热更新。
- **本地开发提效模式（推荐示例 `d -c ./dev.json --mcp-stdio`）**：同一份引擎仅在配置清单中组合 `app-files`（前端静态资源）、`ext-proxy`（`/api/` 转发到本地后端、`stream` 接口零缓冲透传）、`ext-dev-kit`（开启 `/_d/inspect` SSE/WS 检视器、未就绪接口 Mock、Live Reload）以及可选自签测试 `l4-tls`；编码期内修改 Mock 或切换后端目标时只重建对应分支纤程，**不中断进行中的 AI SSE 对话流与 WebSocket 会话**。

---

## 五、非阻塞范围与后续演进边界

以下能力明确不纳入本次 `G1`–`G5` 实施阻塞范围，但在契约设计中预留了清晰扩展点：
1. **HTTP/2 与 HTTP/3（QUIC）帧引擎**：通过 `l4-tls` 的 ALPN 回调与 `http_api::Version` / `HttpResponse::stream` 抽象已保持协议无关，后续可作为独立实现组件 `proto-h2` / `proto-h3` 并列接入 `http-api`。
2. **公网 ACME 自动签发**：目前聚焦证书加载与无损热替换，ACME 账户与挑战验证流程可作为调用 `Loader::apply_patches` 的独立应用插件演进。
3. **跨节点控制面集群同步**：单节点已具备声明式配置文件 + `LoaderService` + `d-mcp`，未来可通过 `d-control-sync` 插件拉取远端配置清单调用同一 `LoaderService`。

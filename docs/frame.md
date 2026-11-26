# frame 框架层接口与运行时契约设计

## 一、定位、边界与零外部依赖原则

`d-frame` 位于工作区 `crates/frame/`（对外发布 crate 名 `d-frame`），是整个系统的第 0 层通用运行时插件微内核。

- **独立边界**：`d-frame` 只负责模块之间的空间依赖组合、时间资源生命周期、事件通信、声明式装配与脱敏拓扑检视；**不包含任何网络协议、套接字、文件系统、配置文本格式解析或业务应用语义**。
- **零外部依赖**：`crates/frame/` **仅依赖 Rust 标准库（`std`）**，不引入任何第三方库，保证极快的编译速度、零供应链包袱与可随时拆仓开源发布的纯粹性。
- **异步执行器解耦**：内部异步等待原语、并发合流与在途任务追踪全部直接基于 `std::future::Future`、`std::task::{Context, Poll, Waker}` 与 `std::sync` 实现；宿主仅需通过极简 `Spawner` 接口注入异步任务调度能力。
- **配置格式解耦**：内置轻量 `ConfigValue` 中立配置值树与插件侧 `ConfigValidator` 校验接口，上层自由选择任意配置文件或命令行解析方式解码后交由 `frame` 编排与热更新。

---

## 二、包内模块划分

```text
crates/frame/             # crates.io 包名 d-frame，仅依赖 Rust std
├── Cargo.toml          # 无 [dependencies]，仅依赖 Rust std
└── src/
    ├── lib.rs          # 公开 API 重导出与包级文档
    ├── error.rs        # FrameError、ErrorKind 与 ValidationIssue
    ├── sync.rs         # BoxFuture、Spawner、内部一次性/屏障/任务计数原语
    ├── service.rs      # Service、ScopedService、ServiceKey、ScopeToken 与 merge_config_values
    ├── effect.rs       # EffectHandle、EffectScope、逆序/并发释放与回滚引擎
    ├── event.rs        # 内置生命周期事件（FiberStateChanged/ServiceChanged/RuntimeReady）与双向过滤派发
    ├── plugin.rs       # Deps（必选/可选依赖）、Plugin trait、插件运行时元数据
    ├── fiber.rs        # FiberState、ActivationKey、父子纤程树、驱动循环与级联刷新
    ├── context.rs      # Context 编程表面、extend/meta/select/intercept 与 use_service
    ├── loader.rs       # ConfigValue、EntryTarget::Group、嵌套增量 Loader 与 LoaderService/Plugin
    └── inspect.rs      # FrameSnapshot 与拓扑父子脱敏检视结构体
```

---

## 三、核心数据类型与 Trait 契约

### 1. 基础异步原语与错误模型（`sync` / `error`）

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 宿主异步执行器适配器：将就绪后的微内核状态机或后台释放任务提交至运行环境。
pub trait Spawner: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>);
}

pub struct FrameOptions {
    pub spawner: Arc<dyn Spawner>,
    /// 生命周期异步错误观察回调（仅用于观测告警，不改变状态机失败语义）。
    pub on_error: Option<Arc<dyn Fn(&FrameError) + Send + Sync>>,
}
```

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InactiveFiber,
    DuplicateService,
    MissingDependency,
    NotServiceOwner,
    ValidationFailed,
    UnknownEntry,
    DuplicateEntryId,
    PluginNotFound,
    LoaderClosed,
    ActivationFailed,
    CleanupFailed,
    Aggregate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct FrameError {
    pub kind: ErrorKind,
    pub message: String,
    pub issues: Vec<ValidationIssue>,
    pub causes: Vec<FrameError>,
}
```

---

### 2. 强类型服务与作用域隔离（`service`）

```rust
use std::any::TypeId;
use std::sync::Arc;

/// 可注册进上下文的服务标记接口。
pub trait Service: Send + Sync + 'static {
    const NAME: &'static str;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServiceKey {
    pub type_id: TypeId,
    pub name: &'static str,
}

impl ServiceKey {
    pub fn of<S: Service>() -> Self {
        Self {
            type_id: TypeId::of::<S>(),
            name: S::NAME,
        }
    }
}

/// 隔离作用域标识：根域为 ScopeToken(0)，每次 isolate::<S>() 分配单调递增令牌。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeToken(pub u64);

/// 可选服务就绪探针：仅当返回 true 时，该服务才对下游依赖方可见。
pub type ReadinessCheck<S> = Arc<dyn Fn(&S) -> bool + Send + Sync>;

/// 调用方绑定服务视图：将资源句柄寿命直接绑定到调用方 `Context` 的 `EffectScope`。
pub trait ScopedService: Service {
    type View: Send + Sync + 'static;
    fn bind(self: Arc<Self>, caller: Context) -> Self::View;
}

pub fn merge_config_values(base: &ConfigValue, patch: &ConfigValue) -> ConfigValue;
```

**服务作用域与所有权核心不变量**：

1. **作用域继承与同域父链继承**：每个 `Context` 持有一个不可变映射链 `Scope`（`ServiceKey -> ScopeToken`）及父纤程链。调用 `ctx.isolate::<S>()` 产生的新 `Context` 会为 `S` 绑定新的 `ScopeToken`；未显式隔离的其他服务沿同隔离域父链自动继承。
2. **调用方绑定寿命**：消费方通过 `ctx.use_service::<S>()` / `ctx.try_use_service::<S>()` 获取与自身 `Context` 绑定的视图；提供者在 `caller` 上登记 `Effect`，消费方卸载时自动回收关联资源，无需手工释放包装。
3. **单域唯一性与读写边界**：同一个 `(ServiceKey, ScopeToken)` 键上同时只允许存在一个提供者（重复返回 `DuplicateService`）；`update_service` 校验所有者（否则 `NotServiceOwner`）；当某依赖已声明在 `Deps` 中且处于卸载清理窗口时，`ctx.get::<S>()` 优先读取该纤程隔离的依存快照。

---

### 3. 异步资源效应（`effect`）

由于 Rust 的 `Drop` 不支持异步等待与错误回传，`Effect` 是 `frame` 最核心的运行时资源所有权原语。

```rust
#[derive(Clone)]
pub struct EffectHandle { /* 内部共享效应记录引用 */ }

impl EffectHandle {
    pub fn label(&self) -> &str;
    pub fn is_disposed(&self) -> bool;
    /// 幂等异步释放：若当前正在初始化则等待初始化与回滚收敛；若已有清理在途则直接 join。
    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>>;
}

/// 传入 effect setup 回调的作用域控制器，用于登记逆序清理步骤与子效应。
pub struct EffectScope { /* 内部状态指针与所有权标识 */ }

impl EffectScope {
    /// 登记同步清理动作（LIFO 逆序执行）。
    pub fn defer<F>(&self, cleanup: F)
    where
        F: FnOnce() + Send + 'static;

    /// 登记异步清理动作（LIFO 逆序依次 await）。
    pub fn defer_async<F>(&self, cleanup: F)
    where
        F: FnOnce() -> BoxFuture<'static, Result<(), FrameError>> + Send + 'static;

    /// 登记受当前效应托管的子效应；子效应自动纳入当前效应的逆序释放树。
    pub fn child_effect<F>(
        &self,
        label: impl Into<String>,
        setup: F,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>
    where
        F: FnOnce(EffectScope) -> BoxFuture<'static, Result<(), FrameError>> + Send + 'static;
}
```

**效应生命周期六条核心不变量**：

1. **所有权先于获取登记**：在 `setup` 回调执行之前，效应记录已写入所属纤程的活跃效应集合；即便获取中途触发重入释放，也能被准确追踪并等待。
2. **终态拒绝新登记**：当所属纤程处于 `FiberState::Unloading` 或 `FiberState::Disposed` 时，任何新发起的 `effect` 或 `defer` 请求直接返回 `ErrorKind::InactiveFiber`。
3. **部分初始化失败自动回滚**：若 `setup` 在登记了若干 `defer` / `child_effect` 后返回 `Err`，效应引擎在返回错误前自动按逆序执行已收集的清理步骤并等待其全部完成。
4. **重复释放幂等**：对同一 `EffectHandle` 多次调用 `dispose()` 不会重复执行清理动作；后发调用等待首轮清理完成后返回一致结果。
5. **结构化并发合流（Join）**：当效应仍在初始化阶段时收到外部 `dispose()` 请求，清理方必须与正在进行的获取过程合流等待，不得在初始化动作尚未退出时并发破坏资源状态。
6. **串并行清理边界与错误隔离**：
   - **同一效应内部**的多个清理步骤严格按后进先出（LIFO）逆序依次 `await` 执行；
   - **同一纤程名下相互独立的兄弟效应**在卸载时可并发推进清理；
   - 任一清理步骤或某个效应清理失败时，错误被收集并上报至 `on_error`，**绝不阻断其余兄弟效应或后续卸载流程的正常释放**。

---

### 4. 强类型事件总线（`event`）

`frame` 将事件划分为三类强类型接口，覆盖通知、短路拦截与洋葱流水线：

```rust
/// 广播型事件（emit / parallel）
pub trait Event: Send + Sync + 'static {
    const NAME: &'static str;
}

/// 短路拦截型事件（bail / serial）：首个返回 Some(Output) 的监听器中止后续执行。
pub trait RequestEvent: Send + Sync + 'static {
    const NAME: &'static str;
    type Output: Send + 'static;
}

/// 流水线型事件（waterfall）：允许监听器包裹延续函数、改写载荷或否决继续执行。
pub trait WaterfallEvent: Send + Sync + 'static {
    const NAME: &'static str;
    type Payload: Send + 'static;
    type Output: Send + 'static;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ListenerOptions {
    /// 为 true 时插入到同事件监听器列表头部，优先执行。
    pub prepend: bool,
    /// 为 true 时忽略派发方附带的上下文过滤谓词。
    pub global: bool,
}

/// 上下文过滤谓词：由派发方指定接收条件，返回 true 的监听器上下文才参与本次派发。
pub type EventFilter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;
```

**四种派发语义与不变量**：

| 模式 | 执行方式 | 返回值与错误处理 | 典型用途 |
| :--- | :--- | :--- | :--- |
| `emit` | 同步顺序调用监听器 | 无返回值；内部恐慌或记录异常不外溢 | 状态变更广播、内置生命周期通知 |
| `parallel` | 并发执行全部异步监听器 | 等待全部完成；聚合所有 `Err` 返回 `ErrorKind::Aggregate` | 并行刷新缓存、并发准备就绪 |
| `bail` / `serial` | 同步（`bail`）或异步（`serial`）按顺序调用 | 首个返回 `Ok(Some(v))` 即短路返回；发生 `Err` 立即中断并上抛 | 路由抢占匹配、展示器选择、请求拦截 |
| `waterfall` | 按注册顺序嵌套组合并环绕最终 `continuation` | 允许修改 `Payload` 后调用 `next(payload)`，或直接返回 `Err`/短路值否决后续执行 | 配置更新校验拦截、局部变更否决 |

**监听器托管不变量**：
- 每次调用 `on*` 或 `once*` 都会在当前上下文所属纤程上登记一条受管 `Effect`，返回 `EffectHandle`；纤程卸载时监听器自动摘除。
- **快照派发**：每次派发开始时对当前匹配监听器列表做浅拷贝快照；回调执行期间增删监听器不影响本轮快照遍历。
- **`once` 先摘除后执行**：一次性监听器在调用用户回调前先执行自身摘除逻辑，避免重入派发引发重复执行。
- **更新拦截作用域隔离**：针对特定纤程发起的配置热更新拦截，仅触发挂载在该纤程上下文作用域下的本地与显式全局更新钩子，**一个插件的更新钩子不会误拦截另一插件的配置更新**。

---

### 5. 插件与响应式纤程状态机（`plugin` / `fiber`）

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiberId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Unloading,
    Failed,
    Disposed,
}

/// 插件依赖集合（按注册顺序去重保存必选与可选 ServiceKey）。
#[derive(Clone, Debug, Default)]
pub struct Deps {
    required: Vec<ServiceKey>,
    optional: Vec<ServiceKey>,
}

impl Deps {
    pub fn empty() -> Self;
    pub fn with<S: Service>(mut self) -> Self;
    pub fn with_optional<S: Service>(mut self) -> Self;
    pub fn keys(&self) -> &[ServiceKey];
    pub fn optional_keys(&self) -> &[ServiceKey];
    pub fn contains_type(&self, type_id: std::any::TypeId) -> bool;
}

/// 配置字段类型与自描述元数据（供 Loader::catalog 与控制面自动发现）。
pub enum ConfigFieldType { Bool, Int, Float, Str, List, Map, Any }
pub struct ConfigFieldSchema { /* name, kind, required, nullable, sensitive, default, description */ }
pub struct PluginSchema { pub description: String, pub fields: Vec<ConfigFieldSchema> }
pub struct PluginDescriptor {
    pub name: String,
    pub description: String,
    pub required_deps: Vec<&'static str>,
    pub optional_deps: Vec<&'static str>,
    pub schema: PluginSchema,
}

/// 所有挂载到 frame 的应用、扩展或协议引擎均实现此接口。
pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    /// 声明插件说明与配置 Schema；控制面据此自动暴露参数描述与敏感字段脱敏。
    fn schema(&self) -> PluginSchema {
        PluginSchema::default()
    }

    /// 同步配置校验与默认值规整：在进入 apply 前执行；不可被更新钩子绕过。
    fn validate_config(&self, raw: &ConfigValue) -> Result<ConfigValue, FrameError> {
        Ok(raw.clone())
    }

    /// 激活入口：通过 ctx 注册效应、提供服务、挂载监听与子插件。
    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>>;
}
```

**纤程激活键（`ActivationKey`）与状态机规则**：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationKey {
    /// 每个声明依赖当前命中的 (服务类型, 提供者纤程 ID, 提供者代际号)
    pub dep_providers: Vec<(TypeId, FiberId, u64)>,
    /// 显式 restart 或已生效 update 递增的本地重启计数
    pub reload_seq: u64,
}
```

1. **驱动与收敛规则**：
   - `Pending`：缺少任一 `Deps` 中的服务，或提供者尚未达到 `Active` / 未通过就绪探针。缺依赖不是故障，`frame` 保持等待直到依赖出现。
   - `Loading`：所有依赖全部就绪且生成了新的 `ActivationKey`。驱动循环先按当前就绪依赖生成一份**只读依赖快照（`store`）**并写入纤程，随后执行 `validate_config` 与 `apply`。
   - `Active`：`apply` 成功返回且在异步执行期间纤程未被卸载、依赖快照未发生变更。此时本纤程提供的服务（若有）对外可见，通知作用域内下游消费者刷新依赖。
   - `Unloading`：因依赖丢失、依赖提供者替换、配置重启或显式 `dispose` 触发卸载。先将状态切为 `Unloading`，并发释放全部独立的根级 `Effect`（同时每个 `Effect` 内部逆序清理）；**在整个清理完成收敛之前，`store` 依赖快照保持有效，确保清理代码仍可安全引用被释放的依赖句柄（即便原提供者已先行下线）**；清理收敛后清空 `store` 与启动标记。
   - `Failed`：`validate_config` 或 `apply` 返回 `Err`，立即自动回滚本次已登记的全部效应并置为 `Failed`。**只要 `ActivationKey` 未改变，同一组合不会死循环重试**；仅当任一依赖变更或用户调用 `update`/`restart` 时才重新尝试激活。
   - `Disposed`：显式卸载或父纤程关闭后的不可逆终态，移出全局纤程表。

2. **纤程控制句柄（`FiberHandle`）**：

```rust
#[derive(Clone)]
pub struct FiberHandle { /* 内部共享引用 */ }

impl FiberHandle {
    pub fn id(&self) -> FiberId;
    pub fn name(&self) -> &str;
    pub fn state(&self) -> FiberState;
    /// 等待当前在途状态转换（激活/卸载/更新）收敛；若最终态为 Failed 则返回对应错误。
    pub fn wait(&self) -> BoxFuture<'static, Result<(), FrameError>>;
    /// 热更新配置：先保留 raw_config；若处于 Active，先走 validate_config 与更新拦截链，
    /// 被否决时保持原 effective_config 不变；若处于 Pending/Failed 则更新待生效配置并触发驱动。
    pub fn update(&self, raw_config: ConfigValue) -> BoxFuture<'static, Result<(), FrameError>>;
    /// 卸载旧效应树并递增 reload_seq 后重新激活。
    pub fn restart(&self) -> BoxFuture<'static, Result<(), FrameError>>;
    /// 永久卸载当前纤程及其全部子纤程与效应。
    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>>;
}
```

---

### 6. 上下文编程表面（`context`）

`Context` 是轻量 `Clone` 句柄，是应用与扩展插件唯一需要直接交互的入口：

```rust
#[derive(Clone)]
pub struct Context { /* engine, scope, fiber_id */ }

impl Context {
    pub fn new(options: FrameOptions) -> Self;

    // --- 作用域与上下文派生 ---
    pub fn isolate<S: Service>(&self) -> Self;
    pub fn isolate_key(&self, key: ServiceKey) -> Self;
    pub fn extend(&self, meta: BTreeMap<String, ConfigValue>) -> Self;
    pub fn with_meta(&self, key: impl Into<String>, val: ConfigValue) -> Self;
    pub fn meta(&self, key: &str) -> Option<&ConfigValue>;
    pub fn select(&self, filter: EventFilter) -> Self;
    pub fn fiber(&self) -> FiberHandle;
    pub fn parent_fiber(&self) -> Option<FiberHandle>;
    pub fn root_fiber(&self) -> FiberHandle;
    pub fn mark_ready(&self);

    // --- 配置局部拦截合并 ---
    pub fn intercept<S: Service>(&self, patch: ConfigValue) -> EffectHandle;
    pub fn resolve_intercept<S: Service>(&self, base: ConfigValue) -> ConfigValue;

    // --- 服务提供与消费 ---
    pub fn provide<S: Service>(
        &self,
        value: Arc<S>,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>;

    pub fn provide_with_check<S: Service>(
        &self,
        value: Arc<S>,
        check: ReadinessCheck<S>,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>;

    pub fn update_service<S: Service>(&self, value: Arc<S>) -> Result<(), FrameError>;
    pub fn get<S: Service>(&self) -> Option<Arc<S>>;
    pub fn require<S: Service>(&self) -> Result<Arc<S>, FrameError>;
    pub fn use_service<S: ScopedService>(&self) -> Result<S::View, FrameError>;
    pub fn try_use_service<S: ScopedService>(&self) -> Option<S::View>;

    // --- 资源效应与插件挂载 ---
    pub fn effect<F>(
        &self,
        label: impl Into<String>,
        setup: F,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>
    where
        F: FnOnce(EffectScope) -> BoxFuture<'static, Result<(), FrameError>> + Send + 'static;

    pub fn plugin<P: Plugin>(&self, plugin: P, config: ConfigValue) -> Result<FiberHandle, FrameError>;

    pub fn inject<F>(
        &self,
        name: impl Into<String>,
        deps: Deps,
        apply: F,
    ) -> Result<FiberHandle, FrameError>
    where
        F: Fn(Context) -> BoxFuture<'static, Result<(), FrameError>> + Send + Sync + 'static;

    // --- 事件订阅与派发 ---
    pub fn on<E: Event, F>(&self, handler: F, options: ListenerOptions) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) + Send + Sync + 'static;

    pub fn once<E: Event, F>(&self, handler: F, options: ListenerOptions) -> Result<EffectHandle, FrameError>
    where
        F: FnOnce(&Context, &E) + Send + 'static;

    pub fn on_async<E: Event, F>(&self, handler: F, options: ListenerOptions) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) -> BoxFuture<'static, Result<(), FrameError>> + Send + Sync + 'static;

    pub fn on_request<E: RequestEvent, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) -> BoxFuture<'static, Result<Option<E::Output>, FrameError>> + Send + Sync + 'static;

    pub fn on_waterfall<E: WaterfallEvent, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, E::Payload, WaterfallNext<E>) -> BoxFuture<'static, Result<E::Output, FrameError>>
            + Send
            + Sync
            + 'static;

    pub fn emit<E: Event>(&self, event: &E, filter: Option<&EventFilter>);
    pub fn parallel<E: Event>(&self, event: &E, filter: Option<&EventFilter>) -> BoxFuture<'static, Result<(), FrameError>>;
    pub fn serial<E: RequestEvent>(&self, event: &E, filter: Option<&EventFilter>) -> BoxFuture<'static, Result<Option<E::Output>, FrameError>>;
    pub fn waterfall<E: WaterfallEvent>(
        &self,
        payload: E::Payload,
        filter: Option<&EventFilter>,
        final_step: WaterfallFinal<E>,
    ) -> BoxFuture<'static, Result<E::Output, FrameError>>;

    // --- 收敛、检视与关停 ---
    /// 等待当前所有在途状态转换与卸载清理任务收敛（不无限等待缺失外部服务的 Pending 纤程）。
    pub fn settle(&self) -> BoxFuture<'static, ()>;
    pub fn inspect(&self) -> FrameSnapshot;
    /// 应用终态关闭：逆序卸载根下全部纤程与效应并等待彻底收敛。
    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>>;
}
```

**根生命周期边界说明**：
- `ctx.root_fiber().dispose()` 仅清空并重启根纤程自身挂载的效应与子插件，根 `Context` 仍保持 `Active` 可继续用于挂载下一轮拓扑（常用于测试复原或整体重载）；
- `ctx.dispose()` 是应用进程的最终停机操作，彻底将根置为 `Disposed` 并等待全部在途异步释放完成。

---

### 7. 声明式装配器与热更新（`loader`）

```rust
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ConfigValue>),
    Map(BTreeMap<String, ConfigValue>),
}

#[derive(Clone)]
pub enum EntryTarget {
    /// 通过工厂标识名称解析（由静态工厂表或可扩展解析器解析）。
    Name(String),
    /// 直接内嵌构造好的插件实例。
    Plugin(Arc<dyn Plugin>),
    /// 纯结构分组容器：管理作用域 `isolate` 与嵌套 `children` 增量 Diff。
    Group,
}

#[derive(Clone)]
pub struct PluginEntry {
    pub id: String,
    pub target: EntryTarget,
    pub config: ConfigValue,
    pub disabled: bool,
    pub isolate: Vec<ServiceKey>,
    pub children: Vec<PluginEntry>,
}

#[derive(Clone)]
pub enum EntryPatch {
    ReplaceConfig { id: String, config: ConfigValue, disabled: Option<bool> },
    Insert { anchor: Option<InsertAnchor>, entry: PluginEntry },
    Remove { id: String },
}

#[derive(Clone)]
pub enum InsertAnchor {
    Before(String),
    After(String),
}

pub trait Resolver: Send + Sync + 'static {
    fn resolve(&self, name: &str) -> Result<Arc<dyn Plugin>, FrameError>;
    fn list_names(&self) -> Vec<String> { Vec::new() }
}
```

> `Loader` 与 `LoaderService` 除 `apply`、`apply_patches`、`inspect`、`close` 外，亦提供只读快照/自省与就地控制接口：`entries()`、`catalog() -> Vec<PluginDescriptor>`、`validate_entries()`、`validate_patches()`（无副作用预检）及 `restart(id)`。

**装配与热更新流程契约**：

1. **全量预检（Preflight First）**：`Loader::apply(entries)` 与 `Loader::apply_patches(patches)` 在触碰任何运行中的纤程之前，必须完成：
   - 结构校验：`id` 非空且在同级作用域内唯一；补丁插删目标必须存在（多条补丁按顺序逐条应用，**前一条插入的 `id` 立即可作为后一条补丁的锚点**）；
   - 递归解析所有未禁用节点的 `Plugin` 实例并预验证配置结构；
   - 若任意节点校验或解析失败，直接返回错误，**运行中的现有插件树零变更、零副作用**。
2. **序列化变更与增量 Diff**：
   - 多次 `apply` / `apply_patches` 请求通过内置异步队列严格串行执行，避免并发交错篡改拓扑；
   - 先**逆序**卸载已移除、已禁用或实现体/隔离配置发生变化（以及非 `Group` 节点的 `children` 结构变化）的旧节点；
   - 再**正序**处理保留节点与新增节点：仅 `config` 变更的保留节点调用现有 `FiberHandle::update`（`Group` 节点当 `children` 变化时直接通过持久共享子 `Loader` 槽递归执行增量 Diff，绝不重建父分组或未受影响兄弟），新增节点派生目标隔离域后挂载新纤程（若启用 `LoaderPlugin`，上下文可通过 `LoaderService` 自举访问补丁与快照接口）；
   - 最后等待本轮变更涉及的纤程完成迁移收敛（不无限阻塞在缺少外部依赖的 `Pending` 纤程上）。
3. **关闭收尾**：`Loader::close()` 拒绝后续新增变更，等待当前在途变更排空后按逆序卸载全部受管节点。

---

### 8. 脱敏拓扑检视（`inspect`）

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub fibers: Vec<FiberSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiberSnapshot {
    pub id: FiberId,
    pub parent: Option<FiberId>,
    pub children: Vec<FiberId>,
    pub name: String,
    pub state: FiberState,
    pub dependencies: Vec<&'static str>,
    pub optional_dependencies: Vec<&'static str>,
    pub provided: Vec<&'static str>,
    pub effect_count: usize,
    /// 若最近一次激活或清理出错，保留截断后最多 512 字节的错误摘要；绝不包含配置原文或凭据。
    pub error_summary: Option<String>,
}
```

---

## 四、并发安全与防死锁不变量

`frame` 的所有内部状态均可在多线程异步执行器间安全并发调用，必须严格遵守以下三条锁与调度红线：

1. **绝不跨 `.await` 或用户回调持锁**：内核共享状态的互斥锁（`std::sync::Mutex`）仅用于纳秒级的内存字段读写、依赖表查询、状态字切换与待唤醒 `Waker` 收集。拿到数据与待调度动作后必须立即释放锁，再调用 `Spawner::spawn`、用户回调（`apply`、`setup`、`cleanup`、`validate_config`、就绪探针、事件回调、`on_error`）或执行 `.await`。
2. **重入天然安全**：由于持锁期间绝不回调外部代码，插件在自身的 `apply` 或清理回调内再次调用 `provide`、`plugin`、`update`、`effect`、`dispose` 均为合法重入，不存在可重入死锁风险。
3. **单纤程驱动串行化**：每条纤程拥有独立的驱动状态标记（`driving` / `dirty` 标志位）。同一时间每条纤程至多只有一个驱动循环在推进其生命周期转换；推进期间若收到新的依赖变化或重载请求，只标记 `dirty` 并在当前步完成后无缝续接下一轮收敛，绝不发生并发撕裂。

---

## 五、核心验收契约清单（`F01`–`F16`）

阶段 1 建立核心基线（`F01`–`F10`），M1 微内核完备化进一步补齐并实测验证 `F11`–`F16`（共 16 组独立契约测试 + 9 个单元测试）：

| 单元 | 验证边界与核心断言 |
| :--- | :--- |
| **F01 · 服务提供、所有权与就绪探针** | 同域重复 `provide` 返回 `DuplicateService`；非提供者调用 `update_service` 返回 `NotServiceOwner`；提供者 `Loading` 期间仅自身可读，就绪探针返回 `false` 时下游保持 `Pending`，探针切为 `true` 并激活后下游立即唤醒激活。 |
| **F02 · 依赖到达、丢失与替换级联** | 缺依赖纤程停在 `Pending` 且 `settle()` 不挂起；依赖提供者上线后消费方自动 `Active`；提供者卸载或替换时消费方自动先完成 `Unloading` 卸载，新提供者上线后再重新激活，绝不持有旧实现引用。 |
| **F03 · 作用域隔离与继承** | 子域 `isolate::<S>()` 后，同域内插件消费隔离实例，未隔离服务自动继承父域提供者；两隔离域各自挂载同类型服务互不冲突。 |
| **F04 · 效应竞态、部分回滚与结构化合流** | 效应在初始化尚未返回时收到并发 `dispose()`，能正确等待初始化完成再按逆序回滚；`setup` 中途返回错误时，已登记的 `defer` 与 `child_effect` 按 LIFO 逆序完整回滚；重复 `dispose()` 幂等。 |
| **F05 · 兄弟效应并发清理与故障隔离** | 同一纤程挂载多个独立效应时卸载阶段并发推进释放；某个清理动作返回错误或失败时记录错误，其余独立效应照常完成清理。 |
| **F06 · 卸载期依赖快照保护** | 消费方异步清理期间，即便提供者已先行下线，消费方通过既有的依赖快照仍可正常访问释放所需的句柄，直至清理彻底结束后快照才清除。 |
| **F07 · 四类事件语义与自动摘除** | `emit` / `parallel` / `bail`·`serial` / `waterfall` 语义与优先级（`prepend`）准确；派发使用快照（回调内删监听器不影响本轮）；`once` 先摘除后调用；上下文过滤器生效且 `global: true` 旁路过滤；纤程卸载后全部监听器自动摘除。 |
| **F08 · 配置校验、更新拦截与否决** | `validate_config` 不可被 `waterfall` 更新钩子绕过；更新钩子否决或校验失败时保留原有效配置而不应用错误输入；插件 A 的更新钩子不拦截插件 B 的更新；`Pending` 期间的配置更新在依赖就绪激活时自动生效。 |
| **F09 · 声明式 `Loader` 预检、补丁与嵌套树** | 候选清单中任一节点解析或校验失败，整体预检拒载且现存运行树不受任何影响；`EntryPatch` 插入的 `id` 立即可被同批后续补丁引用；仅配置变更复用原纤程走 `update`，变更实现或嵌套子树时逆序卸载后重建，停用/移除节点彻底卸载。 |
| **F10 · 脱敏检视、根重置与终态关停** | `inspect()` 准确反映纤程标识、状态、依赖名、效应计数及截断错误摘要，且不泄露配置原文；`root_fiber().dispose()` 清空挂载体后根仍可用；`ctx.dispose()` 彻底关停并将根置为 `Disposed`。 |
| **F11 · `ScopedService` 调用方绑定视图** | `ctx.use_service::<S>()` 将下游产生的 `Effect` 自动挂到调用方 `Context`；调用方单独卸载时关联路由/资源自动回收，提供者无残留。 |
| **F12 · 父子 `Fiber` 拓扑与同隔离域继承** | 显式建立 `parent` / `children` 父子树、`inspect()` 反映父子连接，隔离域下同域祖先服务可被 `require()` 继承而不串入跨隔离分支。 |
| **F13 · 可选依赖 `with_optional` 与卸载快照** | 可选依赖缺失时消费方照常激活，提供者到达/替换/下线时驱动消费方精确重组；可选依赖首轮卸载窗口内仍受卸载快照保护。 |
| **F14 · `extend` / `with_meta` / `select` / `intercept`** | 元信息派生与反向 `select` 过滤器准确；多组 `intercept::<S>` 按声明顺序与 `merge_config_values` 合并且在卸载后自动收回。 |
| **F15 · 内置锁外生命周期事件与重入安全** | `FiberStateChanged`、`ServiceChanged`、`RuntimeReady` 均在内核互斥锁之外派发，回调内直接调用 `inspect()` 与树操作不发生死锁。 |
| **F16 · 嵌套 `EntryTarget::Group` 增量 Diff 与 `LoaderService`** | 修改分组内单一子项仅重建该受影响子项（父分组与兄弟子项零重建）；通过 `LoaderPlugin` 与 `LoaderService` 支持上下文内自举查询与补丁热更。 |

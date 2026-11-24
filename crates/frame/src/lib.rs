//! Zero-dependency plugin microkernel with scoped services, reactive fibers,
//! structured async effects, typed events, and declarative loading.

pub mod context;
pub mod effect;
pub mod error;
pub mod event;
pub mod fiber;
pub mod inspect;
pub mod loader;
pub mod plugin;
pub mod service;
pub mod sync;

pub use context::Context;
pub use effect::{EffectHandle, EffectScope};
pub use error::{ErrorKind, FrameError, ValidationIssue};
pub use event::{
    Event, EventFilter, ListenerOptions, RequestEvent, UpdateConfig,
    UpdateNext, UpdatePayload, WaterfallEvent, WaterfallFinal, WaterfallNext,
};
pub use fiber::{ActivationKey, FiberHandle, FiberId, FiberState};
pub use inspect::{FiberSnapshot, FrameSnapshot};
pub use loader::{
    ConfigValue, EntryPatch, EntryTarget, InsertAnchor, Loader,
    LoaderEntrySnapshot, PluginEntry, Resolver, StaticResolver,
};
pub use plugin::{Deps, Plugin};
pub use service::{ReadinessCheck, Scope, ScopeToken, Service, ServiceKey};
pub use sync::{BoxFuture, FrameOptions, Spawner};

use crate::context::Context;
use crate::error::{ErrorKind, FrameError};
use crate::fiber::{FiberHandle, FiberState};
use crate::plugin::{Deps, Plugin};
use crate::service::{Service, ServiceKey};
use crate::sync::{BoxFuture, SerialQueue};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ConfigValue {
    #[default]
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
    Name(String),
    Plugin(Arc<dyn Plugin>),
    /// Pure structural container group holding `isolate` scopes and `children`.
    Group,
}

impl fmt::Debug for EntryTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.debug_tuple("Name").field(name).finish(),
            Self::Plugin(p) => {
                f.debug_tuple("Plugin").field(&p.name()).finish()
            }
            Self::Group => f.write_str("Group"),
        }
    }
}

impl PartialEq for EntryTarget {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Plugin(a), Self::Plugin(b)) => Arc::ptr_eq(a, b),
            (Self::Group, Self::Group) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginEntry {
    pub id: String,
    pub target: EntryTarget,
    pub config: ConfigValue,
    pub disabled: bool,
    pub isolate: Vec<ServiceKey>,
    pub children: Vec<PluginEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InsertAnchor {
    Before(String),
    After(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryPatch {
    ReplaceConfig {
        id: String,
        config: ConfigValue,
        disabled: Option<bool>,
    },
    Insert {
        anchor: Option<InsertAnchor>,
        entry: PluginEntry,
    },
    Remove {
        id: String,
    },
}

pub trait Resolver: Send + Sync + 'static {
    fn resolve(&self, name: &str) -> Result<Arc<dyn Plugin>, FrameError>;
}

#[derive(Default)]
pub struct StaticResolver {
    entries: BTreeMap<String, Arc<dyn Plugin>>,
}

impl StaticResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P: Plugin>(
        mut self,
        name: impl Into<String>,
        plugin: P,
    ) -> Self {
        self.entries.insert(name.into(), Arc::new(plugin));
        self
    }

    pub fn register_arc(
        mut self,
        name: impl Into<String>,
        plugin: Arc<dyn Plugin>,
    ) -> Self {
        self.entries.insert(name.into(), plugin);
        self
    }
}

impl Resolver for StaticResolver {
    fn resolve(&self, name: &str) -> Result<Arc<dyn Plugin>, FrameError> {
        self.entries.get(name).cloned().ok_or_else(|| {
            FrameError::new(
                ErrorKind::PluginNotFound,
                format!("plugin `{name}` is not registered"),
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderEntrySnapshot {
    pub id: String,
    pub state: FiberState,
}

#[derive(Clone)]
struct PreparedEntry {
    entry: PluginEntry,
    plugin: Option<Arc<dyn Plugin>>,
    inner_plugin: Option<Arc<dyn Plugin>>,
    children: Vec<PreparedEntry>,
    is_group: bool,
    latest_children: Option<Arc<Mutex<Vec<PreparedEntry>>>>,
    child_loader_slot: Option<Arc<Mutex<Option<Loader>>>>,
}

struct LoaderRecord {
    entry: PluginEntry,
    plugin: Arc<dyn Plugin>,
    inner_plugin: Option<Arc<dyn Plugin>>,
    is_group: bool,
    latest_children: Option<Arc<Mutex<Vec<PreparedEntry>>>>,
    child_loader_slot: Option<Arc<Mutex<Option<Loader>>>>,
    fiber: FiberHandle,
}

struct LoaderState {
    closed: bool,
    entries: Vec<PluginEntry>,
    records: Vec<LoaderRecord>,
}

struct LoaderInner {
    ctx: Context,
    resolver: Arc<dyn Resolver>,
    managed_children: bool,
    queue: SerialQueue,
    state: Mutex<LoaderState>,
}

#[derive(Clone)]
pub struct Loader {
    inner: Arc<LoaderInner>,
}

/// Frame service handle exposing a managed [`Loader`] for dynamic topology control.
#[derive(Clone)]
pub struct LoaderService(pub Loader);

impl LoaderService {
    pub fn new(loader: Loader) -> Self {
        Self(loader)
    }

    pub fn loader(&self) -> &Loader {
        &self.0
    }

    pub fn apply(
        &self,
        entries: Vec<PluginEntry>,
    ) -> BoxFuture<'static, Result<Vec<String>, FrameError>> {
        self.0.apply(entries)
    }

    pub fn apply_patches(
        &self,
        patches: Vec<EntryPatch>,
    ) -> BoxFuture<'static, Result<Vec<String>, FrameError>> {
        self.0.apply_patches(patches)
    }

    pub fn inspect(&self) -> Vec<LoaderEntrySnapshot> {
        self.0.inspect()
    }

    pub fn close(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        self.0.close()
    }
}

impl Service for LoaderService {
    const NAME: &'static str = "frame.loader";
}

/// Plugin mounting [`LoaderService`] onto a [`Context`] and closing it on disposal.
#[derive(Clone)]
pub struct LoaderPlugin {
    resolver: Arc<dyn Resolver>,
}

impl LoaderPlugin {
    pub fn new(resolver: Arc<dyn Resolver>) -> Self {
        Self { resolver }
    }
}

impl Plugin for LoaderPlugin {
    fn name(&self) -> &str {
        "frame-loader"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let resolver = Arc::clone(&self.resolver);
        Box::pin(async move {
            let loader = Loader::new(ctx.clone(), resolver);
            let closer = loader.clone();
            ctx.effect("loader-lifecycle", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || closer.close());
                    Ok(())
                })
            })
            .await?;
            ctx.provide(Arc::new(LoaderService::new(loader))).await?;
            Ok(())
        })
    }
}

struct GroupPlugin {
    name: String,
    inner_plugin: Option<Arc<dyn Plugin>>,
    latest_children: Arc<Mutex<Vec<PreparedEntry>>>,
    resolver: Arc<dyn Resolver>,
    child_loader_slot: Arc<Mutex<Option<Loader>>>,
}

impl Plugin for GroupPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn deps(&self) -> Deps {
        self.inner_plugin
            .as_ref()
            .map(|p| p.deps())
            .unwrap_or_default()
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        match &self.inner_plugin {
            Some(p) => p.validate_config(raw),
            None => Ok(raw.clone()),
        }
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let inner_plugin = self.inner_plugin.clone();
        let latest_children = Arc::clone(&self.latest_children);
        let resolver = Arc::clone(&self.resolver);
        let child_loader_slot = Arc::clone(&self.child_loader_slot);
        Box::pin(async move {
            if let Some(p) = inner_plugin {
                p.apply(ctx.clone(), config).await?;
            }
            let child_loader = Loader::new_managed(ctx.clone(), resolver);
            *child_loader_slot.lock().expect("child loader slot lock") =
                Some(child_loader.clone());
            let closer = child_loader.clone();
            let clear_slot = Arc::clone(&child_loader_slot);
            ctx.effect("child-loader", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || {
                        Box::pin(async move {
                            *clear_slot
                                .lock()
                                .expect("child loader slot lock") = None;
                            closer.close().await
                        })
                    });
                    Ok(())
                })
            })
            .await?;
            let initial = latest_children
                .lock()
                .expect("latest children lock")
                .clone();
            child_loader.apply_prepared(initial).await?;
            Ok(())
        })
    }
}

fn validate_entries(entries: &[PluginEntry]) -> Result<(), FrameError> {
    let mut ids = HashSet::with_capacity(entries.len());
    for entry in entries {
        if entry.id.is_empty() {
            return Err(FrameError::new(
                ErrorKind::ValidationFailed,
                "plugin entry id must not be empty",
            ));
        }
        if !ids.insert(entry.id.as_str()) {
            return Err(FrameError::new(
                ErrorKind::DuplicateEntryId,
                format!("duplicate plugin entry id `{}`", entry.id),
            ));
        }
        if !entry.children.is_empty() {
            validate_entries(&entry.children)?;
        }
    }
    Ok(())
}

fn resolve_target(
    target: &EntryTarget,
    resolver: &Arc<dyn Resolver>,
) -> Result<Option<Arc<dyn Plugin>>, FrameError> {
    match target {
        EntryTarget::Name(name) => resolver.resolve(name).map(Some),
        EntryTarget::Plugin(plugin) => Ok(Some(Arc::clone(plugin))),
        EntryTarget::Group => Ok(None),
    }
}

fn preflight_entries(
    entries: &[PluginEntry],
    resolver: &Arc<dyn Resolver>,
) -> Result<Vec<PreparedEntry>, FrameError> {
    validate_entries(entries)?;
    let mut prepared = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.disabled {
            prepared.push(PreparedEntry {
                entry: entry.clone(),
                plugin: None,
                inner_plugin: None,
                children: Vec::new(),
                is_group: false,
                latest_children: None,
                child_loader_slot: None,
            });
            continue;
        }
        let inner_plugin = resolve_target(&entry.target, resolver)?;
        if let Some(p) = &inner_plugin {
            let _ = p.validate_config(&entry.config)?;
        }
        let is_group = matches!(entry.target, EntryTarget::Group)
            || !entry.children.is_empty();
        if !is_group {
            prepared.push(PreparedEntry {
                entry: entry.clone(),
                plugin: inner_plugin.clone(),
                inner_plugin,
                children: Vec::new(),
                is_group: false,
                latest_children: None,
                child_loader_slot: None,
            });
        } else {
            let children = preflight_entries(&entry.children, resolver)?;
            let latest_children = Arc::new(Mutex::new(children.clone()));
            let child_loader_slot = Arc::new(Mutex::new(None));
            let wrapper: Arc<dyn Plugin> = Arc::new(GroupPlugin {
                name: entry.id.clone(),
                inner_plugin: inner_plugin.clone(),
                latest_children: Arc::clone(&latest_children),
                resolver: Arc::clone(resolver),
                child_loader_slot: Arc::clone(&child_loader_slot),
            });
            prepared.push(PreparedEntry {
                entry: entry.clone(),
                plugin: Some(wrapper),
                inner_plugin,
                children,
                is_group: true,
                latest_children: Some(latest_children),
                child_loader_slot: Some(child_loader_slot),
            });
        }
    }
    Ok(prepared)
}

fn same_opt_plugin(
    a: &Option<Arc<dyn Plugin>>,
    b: &Option<Arc<dyn Plugin>>,
) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}

fn apply_patches_to_entries(
    base: &[PluginEntry],
    patches: &[EntryPatch],
) -> Result<Vec<PluginEntry>, FrameError> {
    validate_entries(base)?;
    let mut result = base.to_vec();
    for patch in patches {
        match patch {
            EntryPatch::Insert { anchor, entry } => {
                let idx = match anchor {
                    Some(InsertAnchor::Before(target)) => result
                        .iter()
                        .position(|e| e.id == *target)
                        .ok_or_else(|| {
                            FrameError::new(
                                ErrorKind::UnknownEntry,
                                format!("unknown insert anchor `{target}`"),
                            )
                        })?,
                    Some(InsertAnchor::After(target)) => result
                        .iter()
                        .position(|e| e.id == *target)
                        .map(|i| i + 1)
                        .ok_or_else(|| {
                            FrameError::new(
                                ErrorKind::UnknownEntry,
                                format!("unknown insert anchor `{target}`"),
                            )
                        })?,
                    None => result.len(),
                };
                result.insert(idx, entry.clone());
            }
            EntryPatch::ReplaceConfig {
                id,
                config,
                disabled,
            } => {
                let item = result.iter_mut().find(|e| e.id == *id).ok_or_else(
                    || {
                        FrameError::new(
                            ErrorKind::UnknownEntry,
                            format!("unknown plugin entry `{id}`"),
                        )
                    },
                )?;
                item.config = config.clone();
                if let Some(d) = disabled {
                    item.disabled = *d;
                }
            }
            EntryPatch::Remove { id } => {
                let idx = result.iter().position(|e| e.id == *id).ok_or_else(
                    || {
                        FrameError::new(
                            ErrorKind::UnknownEntry,
                            format!("unknown plugin entry `{id}`"),
                        )
                    },
                )?;
                result.remove(idx);
            }
        }
        validate_entries(&result)?;
    }
    Ok(result)
}

impl Loader {
    pub fn new(ctx: Context, resolver: Arc<dyn Resolver>) -> Self {
        Self::with_mode(ctx, resolver, false)
    }

    fn new_managed(ctx: Context, resolver: Arc<dyn Resolver>) -> Self {
        Self::with_mode(ctx, resolver, true)
    }

    fn with_mode(
        ctx: Context,
        resolver: Arc<dyn Resolver>,
        managed_children: bool,
    ) -> Self {
        Self {
            inner: Arc::new(LoaderInner {
                ctx,
                resolver,
                managed_children,
                queue: SerialQueue::new(),
                state: Mutex::new(LoaderState {
                    closed: false,
                    entries: Vec::new(),
                    records: Vec::new(),
                }),
            }),
        }
    }

    pub fn apply(
        &self,
        entries: Vec<PluginEntry>,
    ) -> BoxFuture<'static, Result<Vec<String>, FrameError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _guard = inner.queue.acquire().await;
            if inner.state.lock().expect("loader lock").closed {
                return Err(FrameError::new(
                    ErrorKind::LoaderClosed,
                    "loader is closed",
                ));
            }
            let prepared = preflight_entries(&entries, &inner.resolver)?;
            inner.commit_prepared(entries, prepared).await
        })
    }

    pub fn apply_patches(
        &self,
        patches: Vec<EntryPatch>,
    ) -> BoxFuture<'static, Result<Vec<String>, FrameError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _guard = inner.queue.acquire().await;
            let current_entries = {
                let st = inner.state.lock().expect("loader lock");
                if st.closed {
                    return Err(FrameError::new(
                        ErrorKind::LoaderClosed,
                        "loader is closed",
                    ));
                }
                st.entries.clone()
            };
            let candidate =
                apply_patches_to_entries(&current_entries, &patches)?;
            let prepared = preflight_entries(&candidate, &inner.resolver)?;
            inner.commit_prepared(candidate, prepared).await
        })
    }

    fn apply_prepared(
        &self,
        prepared: Vec<PreparedEntry>,
    ) -> BoxFuture<'static, Result<Vec<String>, FrameError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _guard = inner.queue.acquire().await;
            if inner.state.lock().expect("loader lock").closed {
                return Err(FrameError::new(
                    ErrorKind::LoaderClosed,
                    "loader is closed",
                ));
            }
            let raw_entries: Vec<PluginEntry> =
                prepared.iter().map(|p| p.entry.clone()).collect();
            inner.commit_prepared(raw_entries, prepared).await
        })
    }

    pub fn inspect(&self) -> Vec<LoaderEntrySnapshot> {
        let st = self.inner.state.lock().expect("loader lock");
        st.records
            .iter()
            .map(|r| LoaderEntrySnapshot {
                id: r.entry.id.clone(),
                state: r.fiber.state(),
            })
            .collect()
    }

    pub fn close(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let _guard = inner.queue.acquire().await;
            let to_dispose = {
                let mut st = inner.state.lock().expect("loader lock");
                if st.closed {
                    return Ok(());
                }
                st.closed = true;
                st.entries.clear();
                std::mem::take(&mut st.records)
            };
            for rec in to_dispose.into_iter().rev() {
                rec.fiber.dispose().await?;
            }
            Ok(())
        })
    }
}

impl LoaderInner {
    async fn commit_prepared(
        &self,
        entries: Vec<PluginEntry>,
        prepared: Vec<PreparedEntry>,
    ) -> Result<Vec<String>, FrameError> {
        let wanted_map: BTreeMap<&str, &PreparedEntry> =
            prepared.iter().map(|p| (p.entry.id.as_str(), p)).collect();

        // Phase 1: reverse-dispose removed, disabled, or structurally changed entries.
        let mut surviving = Vec::new();
        let mut to_dispose = Vec::new();
        {
            let mut st = self.state.lock().expect("loader lock");
            for rec in std::mem::take(&mut st.records) {
                let keep =
                    wanted_map.get(rec.entry.id.as_str()).is_some_and(|cand| {
                        !cand.entry.disabled
                            && rec.entry.target == cand.entry.target
                            && rec.entry.isolate == cand.entry.isolate
                            && rec.is_group == cand.is_group
                            && if rec.is_group {
                                same_opt_plugin(
                                    &rec.inner_plugin,
                                    &cand.inner_plugin,
                                )
                            } else {
                                cand.plugin.as_ref().is_some_and(|p| {
                                    Arc::ptr_eq(&rec.plugin, p)
                                })
                            }
                    });
                if keep {
                    surviving.push(rec);
                } else {
                    to_dispose.push(rec);
                }
            }
        }
        for rec in to_dispose.into_iter().rev() {
            rec.fiber.dispose().await?;
        }

        // Phase 2: forward-apply updates, recursive group diffs, & new mounts.
        let mut next_records = Vec::with_capacity(prepared.len());
        let mut active_ids = Vec::new();
        for row in prepared {
            if row.entry.disabled {
                continue;
            }
            let id = row.entry.id.clone();
            let plugin = row.plugin.expect("enabled entry resolved");
            if let Some(pos) = surviving.iter().position(|r| r.entry.id == id) {
                let mut rec = surviving.swap_remove(pos);
                if rec.is_group
                    && let Some(latest_slot) = &rec.latest_children
                {
                    *latest_slot.lock().expect("latest children lock") =
                        row.children.clone();
                }

                if rec.entry.config != row.entry.config {
                    rec.fiber.update(row.entry.config.clone()).await?;
                } else {
                    rec.fiber.wait().await?;
                    if rec.is_group && rec.entry.children != row.entry.children
                    {
                        let child_loader =
                            rec.child_loader_slot.as_ref().and_then(|s| {
                                s.lock()
                                    .expect("child loader slot lock")
                                    .clone()
                            });
                        if let Some(loader) = child_loader {
                            loader.apply_prepared(row.children).await?;
                        }
                    }
                }
                rec.entry = row.entry;
                active_ids.push(id);
                next_records.push(rec);
            } else {
                let mut scope_ctx = self.ctx.clone();
                for key in &row.entry.isolate {
                    scope_ctx = scope_ctx.isolate_key(*key);
                }
                let fiber = if self.managed_children {
                    scope_ctx.plugin_arc_detached(
                        Arc::clone(&plugin),
                        row.entry.config.clone(),
                    )?
                } else {
                    scope_ctx.plugin_arc(
                        Arc::clone(&plugin),
                        row.entry.config.clone(),
                    )?
                };
                fiber.wait().await?;
                active_ids.push(id);
                next_records.push(LoaderRecord {
                    entry: row.entry,
                    plugin,
                    inner_plugin: row.inner_plugin,
                    is_group: row.is_group,
                    latest_children: row.latest_children,
                    child_loader_slot: row.child_loader_slot,
                    fiber,
                });
            }
        }

        {
            let mut st = self.state.lock().expect("loader lock");
            st.entries = entries;
            st.records = next_records;
        }
        Ok(active_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyPlugin;
    impl Plugin for DummyPlugin {
        fn name(&self) -> &str {
            "dummy"
        }

        fn apply(
            &self,
            _ctx: Context,
            _config: ConfigValue,
        ) -> BoxFuture<'static, Result<(), FrameError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn config_value_nested_equality() {
        let mut m1 = BTreeMap::new();
        m1.insert("a".to_string(), ConfigValue::Int(1));
        m1.insert(
            "b".to_string(),
            ConfigValue::List(vec![ConfigValue::Bool(true)]),
        );
        let mut m2 = BTreeMap::new();
        m2.insert(
            "b".to_string(),
            ConfigValue::List(vec![ConfigValue::Bool(true)]),
        );
        m2.insert("a".to_string(), ConfigValue::Int(1));
        assert_eq!(ConfigValue::Map(m1), ConfigValue::Map(m2));
    }

    #[test]
    fn validate_entries_rejects_empty_and_duplicate_ids() {
        let empty = vec![PluginEntry {
            id: "".to_string(),
            target: EntryTarget::Plugin(Arc::new(DummyPlugin)),
            config: ConfigValue::Null,
            disabled: false,
            isolate: Vec::new(),
            children: Vec::new(),
        }];
        assert_eq!(
            validate_entries(&empty).unwrap_err().kind,
            ErrorKind::ValidationFailed
        );

        let dup = vec![
            PluginEntry {
                id: "same".to_string(),
                target: EntryTarget::Plugin(Arc::new(DummyPlugin)),
                config: ConfigValue::Null,
                disabled: false,
                isolate: Vec::new(),
                children: Vec::new(),
            },
            PluginEntry {
                id: "same".to_string(),
                target: EntryTarget::Plugin(Arc::new(DummyPlugin)),
                config: ConfigValue::Null,
                disabled: false,
                isolate: Vec::new(),
                children: Vec::new(),
            },
        ];
        assert_eq!(
            validate_entries(&dup).unwrap_err().kind,
            ErrorKind::DuplicateEntryId
        );
    }

    #[test]
    fn patches_reject_unknown_entry_or_anchor() {
        let err = apply_patches_to_entries(
            &[],
            &[EntryPatch::Remove {
                id: "missing".to_string(),
            }],
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnknownEntry);
    }
}

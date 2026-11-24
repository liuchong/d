use crate::effect::{
    EffectHandle, EffectScope, create_effect, create_effect_sync,
};
use crate::error::{ErrorKind, FrameError};
use crate::event::{
    AsyncListener, BailListener, Event, EventFilter, EventKey, ListenerKind,
    ListenerOptions, OnceListener, RequestEvent, RuntimeReady, SerialListener,
    ServiceAction, ServiceChanged, SyncListener, UpdateConfig, UpdateNext,
    UpdatePayload, WaterfallEvent, WaterfallFinal, WaterfallListener,
    dispatch_bail, dispatch_emit, dispatch_parallel, dispatch_serial,
    dispatch_waterfall, register_hook,
};
use crate::fiber::{
    Engine, FiberHandle, FiberId, FiberInner, FiberRecord, FiberState,
    dispose_root_permanently, notify_scope, request_drive,
};
use crate::inspect::FrameSnapshot;
use crate::loader::ConfigValue;
use crate::plugin::{Deps, FnPlugin, Plugin};
use crate::service::{
    ErasedReadinessCheck, ReadinessCheck, Scope, ScopeToken, ScopedService,
    Service, ServiceKey, ServiceRecord, merge_config_values,
};
use crate::sync::{BoxFuture, FrameOptions, join_all};
use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Context {
    engine: Arc<Engine>,
    scope: Scope,
    fiber_id: FiberId,
    meta: Arc<BTreeMap<String, ConfigValue>>,
    intercepts: Arc<BTreeMap<TypeId, Vec<ConfigValue>>>,
    selector: Option<EventFilter>,
}

impl Context {
    pub fn new(options: FrameOptions) -> Self {
        let engine = Engine::new(options);
        let root_id = FiberId(0);
        let meta = Arc::new(BTreeMap::new());
        let intercepts = Arc::new(BTreeMap::new());
        let root = Arc::new(FiberRecord {
            id: root_id,
            parent: None,
            name: "root".to_string(),
            engine: Arc::clone(&engine),
            scope: Scope::root(),
            meta: Arc::clone(&meta),
            intercepts: Arc::clone(&intercepts),
            selector: None,
            plugin: None,
            deps: Deps::empty(),
            inner: Mutex::new(FiberInner {
                state: FiberState::Active,
                disposing: false,
                dirty: false,
                driving: false,
                reload_seq: 0,
                raw_config: ConfigValue::Null,
                effective_config: ConfigValue::Null,
                available: HashMap::new(),
                store: HashMap::new(),
                last_attempt: None,
                active_key: None,
                started: true,
                effects: BTreeMap::new(),
                provided: Vec::new(),
                local_update_hooks: Vec::new(),
                last_error: None,
            }),
            transition_notify: crate::sync::Notify::new(),
        });
        engine
            .state
            .lock()
            .expect("engine lock")
            .fibers
            .insert(root_id, root);

        Self {
            engine,
            scope: Scope::root(),
            fiber_id: root_id,
            meta,
            intercepts,
            selector: None,
        }
    }

    pub(crate) fn from_parts(
        engine: Arc<Engine>,
        scope: Scope,
        fiber_id: FiberId,
        meta: Arc<BTreeMap<String, ConfigValue>>,
        intercepts: Arc<BTreeMap<TypeId, Vec<ConfigValue>>>,
        selector: Option<EventFilter>,
    ) -> Self {
        Self {
            engine,
            scope,
            fiber_id,
            meta,
            intercepts,
            selector,
        }
    }

    pub(crate) fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    pub(crate) fn fiber_id(&self) -> FiberId {
        self.fiber_id
    }

    pub(crate) fn selector(&self) -> Option<&EventFilter> {
        self.selector.as_ref()
    }

    pub fn isolate<S: Service>(&self) -> Self {
        self.isolate_key(ServiceKey::of::<S>())
    }

    pub fn isolate_key(&self, key: ServiceKey) -> Self {
        let token = {
            let mut eng = self.engine.state.lock().expect("engine lock");
            let id = eng.next_scope_token;
            eng.next_scope_token += 1;
            ScopeToken(id)
        };
        Self {
            engine: Arc::clone(&self.engine),
            scope: self.scope.with_isolated(key.type_id, token),
            fiber_id: self.fiber_id,
            meta: Arc::clone(&self.meta),
            intercepts: Arc::clone(&self.intercepts),
            selector: self.selector.clone(),
        }
    }

    /// Derive a new context merging `meta` keys over the current context metadata.
    pub fn extend(&self, meta: BTreeMap<String, ConfigValue>) -> Self {
        if meta.is_empty() {
            return self.clone();
        }
        let mut next = (*self.meta).clone();
        for (k, v) in meta {
            next.insert(k, v);
        }
        Self {
            engine: Arc::clone(&self.engine),
            scope: self.scope.clone(),
            fiber_id: self.fiber_id,
            meta: Arc::new(next),
            intercepts: Arc::clone(&self.intercepts),
            selector: self.selector.clone(),
        }
    }

    /// Convenience helper adding a single metadata key to a derived context.
    pub fn with_meta(
        &self,
        key: impl Into<String>,
        value: ConfigValue,
    ) -> Self {
        let mut next = (*self.meta).clone();
        next.insert(key.into(), value);
        Self {
            engine: Arc::clone(&self.engine),
            scope: self.scope.clone(),
            fiber_id: self.fiber_id,
            meta: Arc::new(next),
            intercepts: Arc::clone(&self.intercepts),
            selector: self.selector.clone(),
        }
    }

    /// Read a metadata value attached to this context.
    pub fn meta(&self, key: &str) -> Option<&ConfigValue> {
        self.meta.get(key)
    }

    /// Borrow all metadata keys attached to this context.
    pub fn all_meta(&self) -> &BTreeMap<String, ConfigValue> {
        self.meta.as_ref()
    }

    /// Restrict listeners registered on the derived context so they only receive
    /// events emitted from contexts matching `pred` (composed with parent selectors).
    pub fn select<F>(&self, pred: F) -> Self
    where
        F: Fn(&Context) -> bool + Send + Sync + 'static,
    {
        let next_selector: EventFilter = match &self.selector {
            Some(prev) => {
                let prev = Arc::clone(prev);
                Arc::new(move |em| prev(em) && pred(em))
            }
            None => Arc::new(pred),
        };
        Self {
            engine: Arc::clone(&self.engine),
            scope: self.scope.clone(),
            fiber_id: self.fiber_id,
            meta: Arc::clone(&self.meta),
            intercepts: Arc::clone(&self.intercepts),
            selector: Some(next_selector),
        }
    }

    /// Attach a scoped configuration override for `S` along this context branch.
    pub fn intercept<S: Service>(&self, patch: ConfigValue) -> Self {
        let mut next = (*self.intercepts).clone();
        next.entry(TypeId::of::<S>()).or_default().push(patch);
        Self {
            engine: Arc::clone(&self.engine),
            scope: self.scope.clone(),
            fiber_id: self.fiber_id,
            meta: Arc::clone(&self.meta),
            intercepts: Arc::new(next),
            selector: self.selector.clone(),
        }
    }

    /// Merge base configuration with all scoped `intercept::<S>` layers in order.
    pub fn resolve_intercept<S: Service>(
        &self,
        base: ConfigValue,
    ) -> ConfigValue {
        let Some(patches) = self.intercepts.get(&TypeId::of::<S>()) else {
            return base;
        };
        patches.iter().cloned().fold(base, merge_config_values)
    }

    pub fn fiber(&self) -> FiberHandle {
        let eng = self.engine.state.lock().expect("engine lock");
        let record = eng
            .fibers
            .get(&self.fiber_id)
            .cloned()
            .expect("context fiber exists");
        FiberHandle { record }
    }

    /// Return the parent fiber handle when this context does not point to root.
    pub fn parent_fiber(&self) -> Option<FiberHandle> {
        let eng = self.engine.state.lock().expect("engine lock");
        let record = eng.fibers.get(&self.fiber_id)?;
        let parent_id = record.parent?;
        let parent = eng.fibers.get(&parent_id).cloned()?;
        Some(FiberHandle { record: parent })
    }

    pub fn root_fiber(&self) -> FiberHandle {
        let eng = self.engine.state.lock().expect("engine lock");
        let record = eng
            .fibers
            .get(&FiberId(0))
            .cloned()
            .expect("root fiber exists");
        FiberHandle { record }
    }

    pub fn provide<S: Service>(
        &self,
        value: Arc<S>,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>> {
        self.provide_internal(value, None)
    }

    pub fn provide_with_check<S: Service>(
        &self,
        value: Arc<S>,
        check: ReadinessCheck<S>,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>> {
        let erased: ErasedReadinessCheck = Arc::new(move |any_ref| {
            any_ref
                .downcast_ref::<S>()
                .is_some_and(|typed| check(typed))
        });
        self.provide_internal(value, Some(erased))
    }

    fn provide_internal<S: Service>(
        &self,
        value: Arc<S>,
        check: Option<ErasedReadinessCheck>,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>> {
        let engine = Arc::clone(&self.engine);
        let owner_id = self.fiber_id;
        let scope_token = self.scope.token_for(TypeId::of::<S>());
        let key = ServiceKey::of::<S>();
        let emitter_ctx = self.clone();

        create_effect(
            Arc::clone(&engine),
            owner_id,
            format!("provide:{}", S::NAME),
            move |scope| {
                Box::pin(async move {
                    let record = {
                        let mut eng = engine.state.lock().expect("engine lock");
                        let map_key = (key.type_id, scope_token);
                        if eng.services.contains_key(&map_key) {
                            return Err(FrameError::new(
                                ErrorKind::DuplicateService,
                                format!(
                                    "service `{}` is already provided in this scope",
                                    key.name
                                ),
                            ));
                        }
                        let seq = eng.next_service_seq;
                        eng.next_service_seq += 1;
                        let rec = Arc::new(ServiceRecord {
                            key,
                            scope: scope_token,
                            owner: owner_id,
                            instance_seq: seq,
                            value,
                            check,
                        });
                        eng.services.insert(map_key, Arc::clone(&rec));
                        if let Some(owner) = eng.fibers.get(&owner_id) {
                            let mut st =
                                owner.inner.lock().expect("fiber lock");
                            st.provided.push((
                                key.type_id,
                                scope_token,
                                key.name,
                            ));
                        }
                        rec
                    };

                    dispatch_emit(
                        &engine,
                        Some(&emitter_ctx),
                        &ServiceChanged {
                            service: key.name,
                            scope: scope_token,
                            owner: owner_id,
                            action: ServiceAction::Provided,
                        },
                        None,
                    );

                    let owner_active = {
                        let eng = engine.state.lock().expect("engine lock");
                        eng.fibers.get(&owner_id).is_some_and(|owner| {
                            owner.inner.lock().expect("fiber lock").state
                                == FiberState::Active
                        })
                    };
                    if owner_active {
                        notify_scope(
                            &engine,
                            &[(key.type_id, scope_token, key.name)],
                        );
                    }

                    let clean_engine = Arc::clone(&engine);
                    let clean_record = Arc::clone(&record);
                    let clean_emitter = emitter_ctx.clone();
                    scope.defer_async(move || {
                        Box::pin(async move {
                            let map_key = (key.type_id, scope_token);
                            let removed_actual = {
                                let mut eng = clean_engine
                                    .state
                                    .lock()
                                    .expect("engine lock");
                                let same =
                                    eng.services.get(&map_key).is_some_and(
                                        |cur| Arc::ptr_eq(cur, &clean_record),
                                    );
                                if same {
                                    eng.services.remove(&map_key);
                                }
                                if let Some(owner) = eng.fibers.get(&owner_id) {
                                    let mut st =
                                        owner.inner.lock().expect("fiber lock");
                                    st.provided.retain(|p| {
                                        !(p.0 == key.type_id
                                            && p.1 == scope_token)
                                    });
                                }
                                same
                            };
                            if removed_actual {
                                dispatch_emit(
                                    &clean_engine,
                                    Some(&clean_emitter),
                                    &ServiceChanged {
                                        service: key.name,
                                        scope: scope_token,
                                        owner: owner_id,
                                        action: ServiceAction::Removed,
                                    },
                                    None,
                                );
                            }
                            let affected = notify_scope(
                                &clean_engine,
                                &[(key.type_id, scope_token, key.name)],
                            );
                            if !affected.is_empty() {
                                let waits: Vec<_> = affected
                                    .into_iter()
                                    .map(|f| f.wait_settled())
                                    .collect();
                                let _ = join_all(waits).await;
                            }
                            Ok(())
                        })
                    });

                    Ok(())
                })
            },
        )
    }

    pub fn update_service<S: Service>(
        &self,
        value: Arc<S>,
    ) -> Result<(), FrameError> {
        let scope_token = self.scope.token_for(TypeId::of::<S>());
        let map_key = (TypeId::of::<S>(), scope_token);
        {
            let mut eng = self.engine.state.lock().expect("engine lock");
            let existing = eng.services.get(&map_key).cloned();
            match existing {
                Some(rec) if rec.owner == self.fiber_id => {
                    let seq = eng.next_service_seq;
                    eng.next_service_seq += 1;
                    let next = Arc::new(ServiceRecord {
                        key: rec.key,
                        scope: rec.scope,
                        owner: rec.owner,
                        instance_seq: seq,
                        value,
                        check: rec.check.clone(),
                    });
                    eng.services.insert(map_key, next);
                }
                _ => {
                    return Err(FrameError::new(
                        ErrorKind::NotServiceOwner,
                        format!(
                            "only the owning fiber may update service `{}`",
                            S::NAME
                        ),
                    ));
                }
            }
        }
        dispatch_emit(
            &self.engine,
            Some(self),
            &ServiceChanged {
                service: S::NAME,
                scope: scope_token,
                owner: self.fiber_id,
                action: ServiceAction::Updated,
            },
            None,
        );
        notify_scope(
            &self.engine,
            &[(TypeId::of::<S>(), scope_token, S::NAME)],
        );
        Ok(())
    }

    pub fn notify_service<S: Service>(&self) -> Result<(), FrameError> {
        let scope_token = self.scope.token_for(TypeId::of::<S>());
        notify_scope(
            &self.engine,
            &[(TypeId::of::<S>(), scope_token, S::NAME)],
        );
        Ok(())
    }

    pub fn get<S: Service>(&self) -> Option<Arc<S>> {
        let type_id = TypeId::of::<S>();
        let scope_token = self.scope.token_for(type_id);
        let (store_hit, declared_on_fiber, live_candidate) = {
            let eng = self.engine.state.lock().expect("engine lock");
            if let Some(fiber) = eng.fibers.get(&self.fiber_id) {
                let declared =
                    fiber.plugin.is_some() && fiber.deps.contains_type(type_id);
                let st = fiber.inner.lock().expect("fiber lock");
                if let Some(rec) = st.store.get(&type_id)
                    && rec.scope == scope_token
                {
                    (Some(Arc::clone(rec)), declared, None)
                } else {
                    drop(st);
                    let live =
                        eng.services.get(&(type_id, scope_token)).map(|rec| {
                            let owner_active = rec.owner == self.fiber_id
                                || eng.fibers.get(&rec.owner).is_some_and(
                                    |owner| {
                                        owner
                                            .inner
                                            .lock()
                                            .expect("fiber lock")
                                            .state
                                            == FiberState::Active
                                    },
                                );
                            (Arc::clone(rec), owner_active)
                        });
                    (None, declared, live)
                }
            } else {
                (None, false, None)
            }
        };

        if let Some(rec) = store_hit {
            return rec.downcast_arc::<S>();
        }
        if let Some((rec, owner_active)) = live_candidate {
            // Declared dependencies stay bound to this fiber's activation snapshot
            // except for a service owned by the fiber itself.
            if declared_on_fiber && rec.owner != self.fiber_id {
                return None;
            }
            let visible =
                rec.owner == self.fiber_id || (owner_active && rec.is_ready());
            if visible {
                return rec.downcast_arc::<S>();
            }
        }
        None
    }

    pub fn require<S: Service>(&self) -> Result<Arc<S>, FrameError> {
        let type_id = TypeId::of::<S>();
        let target_scope = self.scope.token_for(type_id);
        let (store_hit, own_or_inherited_live) = {
            let eng = self.engine.state.lock().expect("engine lock");
            let mut found_store: Option<Arc<ServiceRecord>> = None;
            let mut eligible_owners: Vec<(FiberId, bool)> = Vec::new();
            let mut cur_id = Some(self.fiber_id);

            while let Some(fid) = cur_id {
                let Some(fiber) = eng.fibers.get(&fid) else {
                    break;
                };
                if fid != self.fiber_id
                    && fiber.scope.token_for(type_id) != target_scope
                {
                    break;
                }
                let st = fiber.inner.lock().expect("fiber lock");
                if let Some(rec) = st.store.get(&type_id)
                    && rec.scope == target_scope
                {
                    found_store = Some(Arc::clone(rec));
                    break;
                }
                let is_self = fid == self.fiber_id;
                if is_self || st.state == FiberState::Active {
                    eligible_owners.push((fid, is_self));
                }
                cur_id = fiber.parent;
            }

            if found_store.is_some() {
                (found_store, None)
            } else {
                let live = eng.services.get(&(type_id, target_scope)).and_then(
                    |rec| {
                        eligible_owners
                            .iter()
                            .find(|&&(owner, _)| owner == rec.owner)
                            .map(|&(_, is_self)| (Arc::clone(rec), is_self))
                    },
                );
                (None, live)
            }
        };

        if let Some(rec) = store_hit
            && let Some(typed) = rec.downcast_arc::<S>()
        {
            return Ok(typed);
        }
        if let Some((rec, is_self)) = own_or_inherited_live
            && (is_self || rec.is_ready())
            && let Some(typed) = rec.downcast_arc::<S>()
        {
            return Ok(typed);
        }

        Err(FrameError::new(
            ErrorKind::MissingDependency,
            format!("required service `{}` is not available", S::NAME),
        ))
    }

    /// Resolve a required [`ScopedService`] and bind its handle to this caller [`Context`].
    pub fn use_service<S: ScopedService>(&self) -> Result<S::View, FrameError> {
        let svc = self.require::<S>()?;
        Ok(svc.bind(self.clone()))
    }

    /// Resolve an optional or live-ready [`ScopedService`] bound to this caller [`Context`].
    pub fn try_use_service<S: ScopedService>(&self) -> Option<S::View> {
        self.get::<S>().map(|svc| svc.bind(self.clone()))
    }

    pub fn effect<F>(
        &self,
        label: impl Into<String>,
        setup: F,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>
    where
        F: FnOnce(EffectScope) -> BoxFuture<'static, Result<(), FrameError>>
            + Send
            + 'static,
    {
        create_effect(Arc::clone(&self.engine), self.fiber_id, label, setup)
    }

    pub fn effect_sync<F>(
        &self,
        label: impl Into<String>,
        setup: F,
    ) -> Result<EffectHandle, FrameError>
    where
        F: FnOnce(EffectScope) -> Result<(), FrameError>,
    {
        create_effect_sync(
            Arc::clone(&self.engine),
            self.fiber_id,
            label,
            setup,
        )
    }

    pub(crate) fn register_sync_effect<F>(
        &self,
        label: impl Into<String>,
        cleanup: F,
    ) -> Result<EffectHandle, FrameError>
    where
        F: FnOnce() -> Result<(), FrameError> + Send + 'static,
    {
        self.effect_sync(label, move |scope| {
            scope.defer_result(cleanup);
            Ok(())
        })
    }

    pub fn plugin<P: Plugin>(
        &self,
        plugin: P,
        config: ConfigValue,
    ) -> Result<FiberHandle, FrameError> {
        self.plugin_arc(Arc::new(plugin), config)
    }

    pub fn plugin_arc(
        &self,
        plugin: Arc<dyn Plugin>,
        config: ConfigValue,
    ) -> Result<FiberHandle, FrameError> {
        self.plugin_arc_linked(plugin, config, true)
    }

    pub(crate) fn plugin_arc_detached(
        &self,
        plugin: Arc<dyn Plugin>,
        config: ConfigValue,
    ) -> Result<FiberHandle, FrameError> {
        self.plugin_arc_linked(plugin, config, false)
    }

    fn plugin_arc_linked(
        &self,
        plugin: Arc<dyn Plugin>,
        config: ConfigValue,
        link_parent_effect: bool,
    ) -> Result<FiberHandle, FrameError> {
        let record = {
            let mut eng = self.engine.state.lock().expect("engine lock");
            let parent = eng
                .fibers
                .get(&self.fiber_id)
                .cloned()
                .ok_or_else(FrameError::inactive_fiber)?;
            {
                let pst = parent.inner.lock().expect("fiber lock");
                if pst.disposing
                    || matches!(
                        pst.state,
                        FiberState::Unloading | FiberState::Disposed
                    )
                {
                    return Err(FrameError::inactive_fiber());
                }
            }

            let id = FiberId(eng.next_fiber_id);
            eng.next_fiber_id += 1;
            let name = plugin.name().to_string();
            let deps = plugin.deps();
            let record = Arc::new(FiberRecord {
                id,
                parent: Some(self.fiber_id),
                name,
                engine: Arc::clone(&self.engine),
                scope: self.scope.clone(),
                meta: Arc::clone(&self.meta),
                intercepts: Arc::clone(&self.intercepts),
                selector: self.selector.clone(),
                plugin: Some(plugin),
                deps,
                inner: Mutex::new(FiberInner {
                    state: FiberState::Pending,
                    disposing: false,
                    dirty: false,
                    driving: false,
                    reload_seq: 0,
                    raw_config: config,
                    effective_config: ConfigValue::Null,
                    available: HashMap::new(),
                    store: HashMap::new(),
                    last_attempt: None,
                    active_key: None,
                    started: false,
                    effects: BTreeMap::new(),
                    provided: Vec::new(),
                    local_update_hooks: Vec::new(),
                    last_error: None,
                }),
                transition_notify: crate::sync::Notify::new(),
            });
            eng.fibers.insert(id, Arc::clone(&record));
            record
        };

        let child_handle = FiberHandle {
            record: Arc::clone(&record),
        };
        if link_parent_effect {
            let dispose_child = child_handle.clone();
            let link_effect = self.register_child_plugin_effect(dispose_child);
            if let Err(err) = link_effect {
                let mut eng = self.engine.state.lock().expect("engine lock");
                eng.fibers.remove(&record.id);
                return Err(err);
            }
        }

        request_drive(&record);
        Ok(child_handle)
    }

    fn register_child_plugin_effect(
        &self,
        child: FiberHandle,
    ) -> Result<EffectHandle, FrameError> {
        let label = format!("plugin:{}", child.name());
        self.effect_sync(label, move |scope| {
            scope.defer_async(move || child.dispose());
            Ok(())
        })
    }

    pub fn inject<F>(
        &self,
        name: impl Into<String>,
        deps: Deps,
        apply: F,
    ) -> Result<FiberHandle, FrameError>
    where
        F: Fn(Context) -> BoxFuture<'static, Result<(), FrameError>>
            + Send
            + Sync
            + 'static,
    {
        self.plugin(
            FnPlugin {
                name: name.into(),
                deps,
                apply: Arc::new(apply),
            },
            ConfigValue::Null,
        )
    }

    pub fn on<E: Event, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) + Send + Sync + 'static,
    {
        let cb: SyncListener<E> = Arc::new(handler);
        register_hook(
            self,
            EventKey::of::<E>(E::NAME),
            options,
            Arc::new(ListenerKind::Sync(cb)),
        )
    }

    pub fn once<E: Event, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: FnOnce(&Context, &E) + Send + 'static,
    {
        let cb: OnceListener<E> = Arc::new(Mutex::new(Some(Box::new(handler))));
        register_hook(
            self,
            EventKey::of::<E>(E::NAME),
            options,
            Arc::new(ListenerKind::Once(cb)),
        )
    }

    pub fn on_async<E: Event, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) -> BoxFuture<'static, Result<(), FrameError>>
            + Send
            + Sync
            + 'static,
    {
        let cb: AsyncListener<E> = Arc::new(handler);
        register_hook(
            self,
            EventKey::of::<E>(E::NAME),
            options,
            Arc::new(ListenerKind::Async(cb)),
        )
    }

    pub fn on_request<E: RequestEvent, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(
                &Context,
                &E,
            )
                -> BoxFuture<'static, Result<Option<E::Output>, FrameError>>
            + Send
            + Sync
            + 'static,
    {
        let cb: SerialListener<E> = Arc::new(handler);
        register_hook(self, EventKey::of::<E>(E::NAME), options, Arc::new(cb))
    }

    pub fn on_bail<E: RequestEvent, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(&Context, &E) -> Result<Option<E::Output>, FrameError>
            + Send
            + Sync
            + 'static,
    {
        let cb: BailListener<E> = Arc::new(handler);
        register_hook(self, EventKey::of::<E>(E::NAME), options, Arc::new(cb))
    }

    pub fn on_waterfall<E: WaterfallEvent, F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(
                &Context,
                E::Payload,
                crate::event::WaterfallNext<E>,
            ) -> BoxFuture<'static, Result<E::Output, FrameError>>
            + Send
            + Sync
            + 'static,
    {
        let cb: WaterfallListener<E> = Arc::new(handler);
        register_hook(self, EventKey::of::<E>(E::NAME), options, Arc::new(cb))
    }

    pub fn on_update<F>(
        &self,
        handler: F,
        options: ListenerOptions,
    ) -> Result<EffectHandle, FrameError>
    where
        F: Fn(
                &Context,
                UpdatePayload,
                UpdateNext,
            )
                -> BoxFuture<'static, Result<ConfigValue, FrameError>>
            + Send
            + Sync
            + 'static,
    {
        self.on_waterfall::<UpdateConfig, F>(handler, options)
    }

    pub fn emit<E: Event>(&self, event: &E, filter: Option<&EventFilter>) {
        dispatch_emit(&self.engine, Some(self), event, filter);
    }

    pub fn parallel<E: Event>(
        &self,
        event: &E,
        filter: Option<&EventFilter>,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        dispatch_parallel(&self.engine, Some(self), event, filter)
    }

    pub fn bail<E: RequestEvent>(
        &self,
        event: &E,
        filter: Option<&EventFilter>,
    ) -> Result<Option<E::Output>, FrameError> {
        dispatch_bail(&self.engine, Some(self), event, filter)
    }

    pub fn serial<'a, E: RequestEvent>(
        &self,
        event: &'a E,
        filter: Option<&EventFilter>,
    ) -> BoxFuture<'a, Result<Option<E::Output>, FrameError>> {
        dispatch_serial(&self.engine, Some(self), event, filter)
    }

    pub fn waterfall<E: WaterfallEvent>(
        &self,
        payload: E::Payload,
        filter: Option<&EventFilter>,
        final_step: WaterfallFinal<E>,
    ) -> BoxFuture<'static, Result<E::Output, FrameError>> {
        dispatch_waterfall::<E>(
            &self.engine,
            Some(self),
            payload,
            filter,
            final_step,
        )
    }

    /// Emit [`RuntimeReady`] once top-level startup converges.
    pub fn mark_ready(&self) {
        self.emit(&RuntimeReady, None);
    }

    pub fn settle(&self) -> BoxFuture<'static, ()> {
        self.engine.settle_arc()
    }

    pub fn inspect(&self) -> FrameSnapshot {
        self.engine.inspect()
    }

    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        let root = self.root_fiber().record;
        Box::pin(async move { dispose_root_permanently(&root).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::Spawner;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Poll, Waker};

    struct InlineSpawner;
    impl Spawner for InlineSpawner {
        fn spawn(&self, fut: BoxFuture<'static, ()>) {
            let mut fut = fut;
            let waker = Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            let _ = fut.as_mut().poll(&mut cx);
        }
    }

    struct ReentrantSvc(u32);
    impl Service for ReentrantSvc {
        const NAME: &'static str = "reentrant";
    }

    struct SyncBailEvent;
    impl RequestEvent for SyncBailEvent {
        const NAME: &'static str = "sync-bail";
        type Output = u32;
    }

    #[test]
    fn reentrant_provide_and_sync_bail_work_without_deadlock() {
        let ctx = Context::new(FrameOptions::new(Arc::new(InlineSpawner)));
        let seen = Arc::new(AtomicBool::new(false));
        let seen_c = Arc::clone(&seen);

        let fiber = ctx
            .inject("reentrant", Deps::empty(), move |c| {
                let seen_inner = Arc::clone(&seen_c);
                Box::pin(async move {
                    c.provide(Arc::new(ReentrantSvc(42))).await?;
                    let got = c.require::<ReentrantSvc>()?;
                    seen_inner.store(got.0 == 42, Ordering::Release);
                    Ok(())
                })
            })
            .expect("mount");

        assert_eq!(fiber.state(), FiberState::Active);
        assert!(seen.load(Ordering::Acquire));

        ctx.on_bail::<SyncBailEvent, _>(
            |_, _| Ok(Some(7)),
            ListenerOptions::default(),
        )
        .expect("bail 1");
        ctx.on_bail::<SyncBailEvent, _>(
            |_, _| panic!("second bail handler must not execute"),
            ListenerOptions::default(),
        )
        .expect("bail 2");
        assert_eq!(ctx.bail(&SyncBailEvent, None).expect("bail"), Some(7));

        let rolled_back = Arc::new(AtomicBool::new(false));
        let rolled_back_clone = Arc::clone(&rolled_back);
        let sync_err = ctx
            .effect_sync("sync-fail", move |scope| {
                scope.defer(move || {
                    rolled_back_clone.store(true, Ordering::Release);
                });
                Err(FrameError::new(ErrorKind::ActivationFailed, "sync boom"))
            })
            .expect_err("sync effect error");
        assert_eq!(sync_err.kind, ErrorKind::ActivationFailed);
        assert!(rolled_back.load(Ordering::Acquire));

        let mut cx = std::task::Context::from_waker(Waker::noop());
        let mut d = ctx.dispose();
        assert!(matches!(
            Pin::new(&mut d).poll(&mut cx),
            Poll::Ready(Ok(()))
        ));
    }
}

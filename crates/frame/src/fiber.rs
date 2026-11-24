use crate::context::Context;
use crate::effect::{EffectHandle, EffectInner};
use crate::error::{ErrorKind, FrameError};
use crate::event::{
    EventFilter, EventKey, FiberStateChanged, HookEntry, UpdateConfig,
    UpdatePayload, dispatch_emit, run_waterfall_hooks, snapshot_update_hooks,
};
use crate::inspect::{FiberSnapshot, FrameSnapshot, truncate_summary};
use crate::loader::ConfigValue;
use crate::plugin::Plugin;
use crate::service::{Scope, ScopeToken, ServiceRecord};
use crate::sync::{BoxFuture, FrameOptions, Notify, join_all};
use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationKey {
    pub dep_providers: Vec<(TypeId, FiberId, u64)>,
    pub optional_providers: Vec<(TypeId, Option<(FiberId, u64)>)>,
    pub reload_seq: u64,
}

pub(crate) struct EngineState {
    pub(crate) next_fiber_id: u64,
    pub(crate) next_scope_token: u64,
    pub(crate) next_effect_id: u64,
    pub(crate) next_service_seq: u64,
    pub(crate) next_listener_id: u64,
    pub(crate) fibers: BTreeMap<FiberId, Arc<FiberRecord>>,
    pub(crate) services: HashMap<(TypeId, ScopeToken), Arc<ServiceRecord>>,
    pub(crate) hooks: HashMap<EventKey, Vec<Arc<HookEntry>>>,
}

pub(crate) struct Engine {
    pub(crate) options: FrameOptions,
    pub(crate) state: Mutex<EngineState>,
    pub(crate) active_tasks: AtomicUsize,
    pub(crate) settle_notify: Notify,
}

struct TaskGuard {
    engine: Arc<Engine>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if self.engine.active_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.engine.settle_notify.notify_all();
        }
    }
}

impl Engine {
    pub(crate) fn new(options: FrameOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            state: Mutex::new(EngineState {
                next_fiber_id: 1,
                next_scope_token: 1,
                next_effect_id: 1,
                next_service_seq: 1,
                next_listener_id: 1,
                fibers: BTreeMap::new(),
                services: HashMap::new(),
                hooks: HashMap::new(),
            }),
            active_tasks: AtomicUsize::new(0),
            settle_notify: Notify::new(),
        })
    }

    pub(crate) fn report_error(&self, err: &FrameError) {
        if let Some(cb) = &self.options.on_error {
            cb(err);
        }
    }

    pub(crate) fn spawn_tracked(self: &Arc<Self>, fut: BoxFuture<'static, ()>) {
        self.active_tasks.fetch_add(1, Ordering::AcqRel);
        let guard = TaskGuard {
            engine: Arc::clone(self),
        };
        self.options.spawner.spawn(Box::pin(async move {
            let _guard = guard;
            fut.await;
        }));
    }

    pub(crate) fn settle_arc(self: &Arc<Self>) -> BoxFuture<'static, ()> {
        let engine = Arc::clone(self);
        Box::pin(async move {
            loop {
                let seq = engine.settle_notify.seq();
                if engine.active_tasks.load(Ordering::Acquire) == 0 {
                    break;
                }
                engine.settle_notify.wait_change(seq).await;
            }
        })
    }

    pub(crate) fn inspect(&self) -> FrameSnapshot {
        let records: Vec<Arc<FiberRecord>> = {
            let eng = self.state.lock().expect("engine lock");
            eng.fibers.values().cloned().collect()
        };
        let mut children_map: HashMap<FiberId, Vec<FiberId>> = HashMap::new();
        for record in &records {
            if let Some(pid) = record.parent {
                children_map.entry(pid).or_default().push(record.id);
            }
        }
        let mut fibers = Vec::with_capacity(records.len());
        for record in records {
            let dependencies =
                record.deps.keys().iter().map(|k| k.name).collect();
            let optional_dependencies =
                record.deps.optional_keys().iter().map(|k| k.name).collect();
            let children = children_map.remove(&record.id).unwrap_or_default();
            let st = record.inner.lock().expect("fiber lock");
            let provided = st.provided.iter().map(|p| p.2).collect();
            let error_summary =
                st.last_error.as_ref().map(|e| truncate_summary(&e.message));
            fibers.push(FiberSnapshot {
                id: record.id,
                parent: record.parent,
                children,
                name: record.name.clone(),
                state: st.state,
                dependencies,
                optional_dependencies,
                provided,
                effect_count: st.effects.len(),
                error_summary,
            });
        }
        FrameSnapshot { fibers }
    }
}

pub(crate) struct FiberInner {
    pub(crate) state: FiberState,
    pub(crate) disposing: bool,
    pub(crate) dirty: bool,
    pub(crate) driving: bool,
    pub(crate) reload_seq: u64,
    pub(crate) raw_config: ConfigValue,
    pub(crate) effective_config: ConfigValue,
    pub(crate) available: HashMap<TypeId, Arc<ServiceRecord>>,
    pub(crate) store: HashMap<TypeId, Arc<ServiceRecord>>,
    pub(crate) last_attempt: Option<ActivationKey>,
    pub(crate) active_key: Option<ActivationKey>,
    pub(crate) started: bool,
    pub(crate) effects: BTreeMap<u64, Arc<EffectInner>>,
    pub(crate) provided: Vec<(TypeId, ScopeToken, &'static str)>,
    pub(crate) local_update_hooks: Vec<Arc<HookEntry>>,
    pub(crate) last_error: Option<FrameError>,
}

pub(crate) struct FiberRecord {
    pub(crate) id: FiberId,
    pub(crate) parent: Option<FiberId>,
    pub(crate) name: String,
    pub(crate) engine: Arc<Engine>,
    pub(crate) scope: Scope,
    pub(crate) meta: Arc<BTreeMap<String, ConfigValue>>,
    pub(crate) intercepts: Arc<BTreeMap<TypeId, Vec<ConfigValue>>>,
    pub(crate) selector: Option<EventFilter>,
    pub(crate) plugin: Option<Arc<dyn Plugin>>,
    pub(crate) deps: crate::plugin::Deps,
    pub(crate) inner: Mutex<FiberInner>,
    pub(crate) transition_notify: Notify,
}

impl FiberRecord {
    pub(crate) fn context(&self) -> Context {
        Context::from_parts(
            Arc::clone(&self.engine),
            self.scope.clone(),
            self.id,
            Arc::clone(&self.meta),
            Arc::clone(&self.intercepts),
            self.selector.clone(),
        )
    }
}

fn emit_state_changed(
    fiber: &FiberRecord,
    from: FiberState,
    to: FiberState,
    error: Option<String>,
) {
    if from == to {
        return;
    }
    let emitter = fiber.context();
    let ev = FiberStateChanged {
        fiber_id: fiber.id,
        parent_id: fiber.parent,
        name: fiber.name.clone(),
        from,
        to,
        error,
    };
    dispatch_emit(&fiber.engine, Some(&emitter), &ev, None);
}

#[derive(Clone)]
pub struct FiberHandle {
    pub(crate) record: Arc<FiberRecord>,
}

impl FiberHandle {
    pub fn id(&self) -> FiberId {
        self.record.id
    }

    pub fn name(&self) -> &str {
        &self.record.name
    }

    pub fn state(&self) -> FiberState {
        self.record.inner.lock().expect("fiber lock").state
    }

    pub fn wait(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            loop {
                let seq = record.transition_notify.seq();
                let status = {
                    let st = record.inner.lock().expect("fiber lock");
                    if !st.driving && !st.dirty {
                        Some(match st.state {
                            FiberState::Failed => Err(st
                                .last_error
                                .clone()
                                .unwrap_or_else(|| {
                                    FrameError::new(
                                        ErrorKind::ActivationFailed,
                                        "plugin activation failed",
                                    )
                                })),
                            _ => Ok(()),
                        })
                    } else {
                        None
                    }
                };
                if let Some(res) = status {
                    return res;
                }
                record.transition_notify.wait_change(seq).await;
            }
        })
    }

    pub(crate) fn wait_settled(&self) -> BoxFuture<'static, ()> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            loop {
                let seq = record.transition_notify.seq();
                let settled = {
                    let st = record.inner.lock().expect("fiber lock");
                    !st.driving && !st.dirty
                };
                if settled {
                    break;
                }
                record.transition_notify.wait_change(seq).await;
            }
        })
    }

    pub fn update(
        &self,
        raw_config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            if record.plugin.is_none() {
                return Ok(());
            }
            let cur_state = {
                let mut st = record.inner.lock().expect("fiber lock");
                if st.disposing || st.state == FiberState::Disposed {
                    return Err(FrameError::inactive_fiber());
                }
                st.raw_config = raw_config.clone();
                st.state
            };

            if cur_state != FiberState::Active {
                {
                    let mut st = record.inner.lock().expect("fiber lock");
                    st.reload_seq += 1;
                    st.last_error = None;
                    st.last_attempt = None;
                }
                request_drive(&record);
                return FiberHandle { record }.wait().await;
            }

            let plugin = record.plugin.as_ref().expect("plugin present");
            let validated = plugin.validate_config(&raw_config)?;
            let hooks = snapshot_update_hooks(&record.engine, record.id);
            let payload = UpdatePayload {
                fiber_id: record.id,
                plugin_name: record.name.clone(),
                config: validated,
            };
            let after_hooks = run_waterfall_hooks::<UpdateConfig>(
                hooks,
                payload,
                Box::new(|final_payload| {
                    Box::pin(async move { Ok(final_payload.config) })
                }),
            )
            .await?;
            let accepted = plugin.validate_config(&after_hooks)?;

            {
                let mut st = record.inner.lock().expect("fiber lock");
                if st.disposing || st.state == FiberState::Disposed {
                    return Err(FrameError::inactive_fiber());
                }
                st.raw_config = accepted;
                st.reload_seq += 1;
                st.last_error = None;
                st.last_attempt = None;
            }
            request_drive(&record);
            FiberHandle { record }.wait().await
        })
    }

    pub fn restart(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            if record.plugin.is_none() {
                return reset_root_fiber(&record).await;
            }
            {
                let mut st = record.inner.lock().expect("fiber lock");
                if st.disposing || st.state == FiberState::Disposed {
                    return Err(FrameError::inactive_fiber());
                }
                st.reload_seq += 1;
                st.last_error = None;
                st.last_attempt = None;
            }
            request_drive(&record);
            FiberHandle { record }.wait().await
        })
    }

    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            if record.plugin.is_none() {
                return reset_root_fiber(&record).await;
            }
            let (inflight_effects, transition): (
                Vec<Arc<EffectInner>>,
                Option<(FiberState, FiberState)>,
            ) = {
                let mut st = record.inner.lock().expect("fiber lock");
                if st.state == FiberState::Disposed && !st.driving && !st.dirty
                {
                    return Ok(());
                }
                st.disposing = true;
                let transition = if st.state == FiberState::Loading {
                    st.state = FiberState::Unloading;
                    Some((FiberState::Loading, FiberState::Unloading))
                } else {
                    None
                };
                (st.effects.values().cloned().collect(), transition)
            };
            record.transition_notify.notify_all();
            if let Some((from, to)) = transition {
                emit_state_changed(&record, from, to, None);
            }
            request_drive(&record);

            if !inflight_effects.is_empty() {
                let futs: Vec<_> =
                    inflight_effects.into_iter().map(|e| e.dispose()).collect();
                let _ = join_all(futs).await;
            }

            FiberHandle { record }.wait_settled().await;
            Ok(())
        })
    }
}

pub(crate) fn request_drive(fiber: &Arc<FiberRecord>) {
    if fiber.plugin.is_none() {
        return;
    }
    let should_spawn = {
        let mut st = fiber.inner.lock().expect("fiber lock");
        st.dirty = true;
        if !st.driving {
            st.driving = true;
            true
        } else {
            false
        }
    };
    if should_spawn {
        let fiber = Arc::clone(fiber);
        let engine = Arc::clone(&fiber.engine);
        engine.spawn_tracked(Box::pin(async move {
            drive_fiber_loop(fiber).await;
        }));
    }
}

fn refresh_available_for_fiber(fiber: &Arc<FiberRecord>) {
    let candidates: Vec<(TypeId, Arc<ServiceRecord>)> = {
        let eng = fiber.engine.state.lock().expect("engine lock");
        let mut list = Vec::new();
        for dep in fiber
            .deps
            .keys()
            .iter()
            .chain(fiber.deps.optional_keys().iter())
        {
            let scope_token = fiber.scope.token_for(dep.type_id);
            if let Some(rec) = eng.services.get(&(dep.type_id, scope_token)) {
                list.push((dep.type_id, Arc::clone(rec)));
            }
        }
        list
    };

    // Check readiness & provider Active status outside all locks.
    let mut ready_map = HashMap::new();
    for (type_id, rec) in candidates {
        let provider_active = {
            let eng = fiber.engine.state.lock().expect("engine lock");
            eng.fibers.get(&rec.owner).is_some_and(|owner| {
                owner.inner.lock().expect("fiber lock").state
                    == FiberState::Active
            })
        };
        if provider_active && rec.is_ready() {
            ready_map.insert(type_id, rec);
        }
    }

    let mut st = fiber.inner.lock().expect("fiber lock");
    st.available = ready_map;
}

fn compute_desired_key(
    fiber: &FiberRecord,
    st: &FiberInner,
) -> Option<ActivationKey> {
    if st.disposing || fiber.plugin.is_none() {
        return None;
    }
    let mut dep_providers = Vec::with_capacity(fiber.deps.keys().len());
    for dep in fiber.deps.keys() {
        let rec = st.available.get(&dep.type_id)?;
        dep_providers.push((dep.type_id, rec.owner, rec.instance_seq));
    }
    let mut optional_providers =
        Vec::with_capacity(fiber.deps.optional_keys().len());
    for opt in fiber.deps.optional_keys() {
        let found = st
            .available
            .get(&opt.type_id)
            .map(|rec| (rec.owner, rec.instance_seq));
        optional_providers.push((opt.type_id, found));
    }
    Some(ActivationKey {
        dep_providers,
        optional_providers,
        reload_seq: st.reload_seq,
    })
}

async fn unload_fiber(fiber: &Arc<FiberRecord>) {
    let (effects, prev): (Vec<Arc<EffectInner>>, FiberState) = {
        let mut st = fiber.inner.lock().expect("fiber lock");
        let prev = st.state;
        if prev != FiberState::Disposed {
            st.state = FiberState::Unloading;
        }
        (st.effects.values().cloned().collect(), prev)
    };
    fiber.transition_notify.notify_all();
    if prev != FiberState::Disposed && prev != FiberState::Unloading {
        emit_state_changed(fiber, prev, FiberState::Unloading, None);
    }

    if !effects.is_empty() {
        let futs: Vec<_> = effects.into_iter().map(|e| e.dispose()).collect();
        let _ = join_all(futs).await;
    }

    {
        let mut st = fiber.inner.lock().expect("fiber lock");
        st.store.clear();
        st.active_key = None;
        st.started = false;
        st.local_update_hooks.clear();
    }
    fiber.transition_notify.notify_all();
}

async fn drive_fiber_loop(fiber: Arc<FiberRecord>) {
    loop {
        refresh_available_for_fiber(&fiber);

        enum Step {
            UnloadFirst,
            FinishDisposed,
            StayPending,
            StayFailed,
            Activate(ActivationKey, ConfigValue),
        }

        let (step, pending_transition) = {
            let mut st = fiber.inner.lock().expect("fiber lock");
            st.dirty = false;
            let desired = compute_desired_key(&fiber, &st);

            if st.started && desired.as_ref() != st.active_key.as_ref() {
                (Step::UnloadFirst, None)
            } else if st.disposing {
                (Step::FinishDisposed, None)
            } else if desired.is_none() {
                let prev = st.state;
                st.state = FiberState::Pending;
                st.last_attempt = None;
                st.last_error = None;
                (Step::StayPending, Some((prev, FiberState::Pending)))
            } else if st.state == FiberState::Failed
                && st.last_attempt.as_ref() == desired.as_ref()
            {
                (Step::StayFailed, None)
            } else if let Some(key) = desired {
                let prev = st.state;
                st.last_attempt = Some(key.clone());
                st.active_key = Some(key.clone());
                st.started = true;
                st.store = st.available.clone();
                st.state = FiberState::Loading;
                (
                    Step::Activate(key, st.raw_config.clone()),
                    Some((prev, FiberState::Loading)),
                )
            } else {
                let prev = st.state;
                (Step::StayPending, Some((prev, FiberState::Pending)))
            }
        };

        if let Some((from, to)) = pending_transition {
            emit_state_changed(&fiber, from, to, None);
        }

        match step {
            Step::UnloadFirst => {
                unload_fiber(&fiber).await;
                continue;
            }
            Step::FinishDisposed => {
                let has_effects = {
                    let st = fiber.inner.lock().expect("fiber lock");
                    !st.effects.is_empty()
                };
                if has_effects {
                    unload_fiber(&fiber).await;
                }
                let (prev, done) = {
                    let mut eng = fiber.engine.state.lock().expect("engine");
                    let mut st = fiber.inner.lock().expect("fiber lock");
                    let prev = st.state;
                    st.state = FiberState::Disposed;
                    st.store.clear();
                    st.provided.clear();
                    eng.fibers.remove(&fiber.id);
                    let done = if !st.dirty {
                        st.driving = false;
                        true
                    } else {
                        false
                    };
                    (prev, done)
                };
                fiber.transition_notify.notify_all();
                if prev != FiberState::Disposed {
                    emit_state_changed(
                        &fiber,
                        prev,
                        FiberState::Disposed,
                        None,
                    );
                }
                if done {
                    break;
                }
            }
            Step::StayPending | Step::StayFailed => {
                let done = {
                    let mut st = fiber.inner.lock().expect("fiber lock");
                    if !st.dirty {
                        st.driving = false;
                        true
                    } else {
                        false
                    }
                };
                fiber.transition_notify.notify_all();
                if done {
                    break;
                }
            }
            Step::Activate(key, raw_cfg) => {
                fiber.transition_notify.notify_all();
                let plugin =
                    Arc::clone(fiber.plugin.as_ref().expect("non-root plugin"));
                let apply_res = match plugin.validate_config(&raw_cfg) {
                    Ok(validated) => {
                        {
                            let mut st =
                                fiber.inner.lock().expect("fiber lock");
                            st.effective_config = validated.clone();
                        }
                        let ctx = fiber.context();
                        plugin.apply(ctx, validated).await
                    }
                    Err(e) => Err(e),
                };

                match apply_res {
                    Err(err) => {
                        fiber.engine.report_error(&err);
                        unload_fiber(&fiber).await;
                        let err_msg = err.message.clone();
                        let (prev, next) = {
                            let mut st =
                                fiber.inner.lock().expect("fiber lock");
                            let prev = st.state;
                            st.last_error = Some(err);
                            let next = if st.disposing {
                                FiberState::Disposed
                            } else {
                                FiberState::Failed
                            };
                            st.state = next;
                            (prev, next)
                        };
                        fiber.transition_notify.notify_all();
                        emit_state_changed(&fiber, prev, next, Some(err_msg));
                        continue;
                    }
                    Ok(()) => {
                        refresh_available_for_fiber(&fiber);
                        let valid_still = {
                            let st = fiber.inner.lock().expect("fiber lock");
                            !st.disposing
                                && compute_desired_key(&fiber, &st).as_ref()
                                    == Some(&key)
                        };
                        if !valid_still {
                            unload_fiber(&fiber).await;
                            continue;
                        }

                        let (prev, provided) = {
                            let mut st =
                                fiber.inner.lock().expect("fiber lock");
                            let prev = st.state;
                            st.last_error = None;
                            st.state = FiberState::Active;
                            (prev, st.provided.clone())
                        };
                        fiber.transition_notify.notify_all();
                        emit_state_changed(
                            &fiber,
                            prev,
                            FiberState::Active,
                            None,
                        );
                        if !provided.is_empty() {
                            notify_scope(&fiber.engine, &provided);
                        }

                        let done = {
                            let mut st =
                                fiber.inner.lock().expect("fiber lock");
                            if !st.dirty {
                                st.driving = false;
                                true
                            } else {
                                false
                            }
                        };
                        fiber.transition_notify.notify_all();
                        if done {
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn notify_scope(
    engine: &Arc<Engine>,
    services: &[(TypeId, ScopeToken, &'static str)],
) -> Vec<FiberHandle> {
    let candidates: Vec<Arc<FiberRecord>> = {
        let eng = engine.state.lock().expect("engine lock");
        eng.fibers
            .values()
            .filter(|f| {
                f.plugin.is_some()
                    && services.iter().any(|(tid, tok, _)| {
                        f.deps.contains_type(*tid)
                            && f.scope.token_for(*tid) == *tok
                    })
            })
            .cloned()
            .collect()
    };

    let mut affected = Vec::new();
    for fiber in candidates {
        refresh_available_for_fiber(&fiber);
        let changed = {
            let st = fiber.inner.lock().expect("fiber lock");
            let desired = compute_desired_key(&fiber, &st);
            desired != st.active_key || st.state == FiberState::Failed
        };
        if changed {
            request_drive(&fiber);
            affected.push(FiberHandle { record: fiber });
        }
    }
    affected
}

pub(crate) async fn reset_root_fiber(
    root: &Arc<FiberRecord>,
) -> Result<(), FrameError> {
    {
        let st = root.inner.lock().expect("fiber lock");
        if st.disposing || st.state == FiberState::Disposed {
            return Err(FrameError::inactive_fiber());
        }
    }
    unload_fiber(root).await;
    let prev = {
        let mut st = root.inner.lock().expect("fiber lock");
        let prev = st.state;
        if !st.disposing {
            st.state = FiberState::Active;
            st.provided.clear();
        }
        prev
    };
    root.transition_notify.notify_all();
    emit_state_changed(root, prev, FiberState::Active, None);
    root.engine.settle_arc().await;
    Ok(())
}

pub(crate) async fn dispose_root_permanently(
    root: &Arc<FiberRecord>,
) -> Result<(), FrameError> {
    let prev = {
        let mut st = root.inner.lock().expect("fiber lock");
        if st.state == FiberState::Disposed {
            return Ok(());
        }
        let prev = st.state;
        st.disposing = true;
        st.state = FiberState::Unloading;
        prev
    };
    root.transition_notify.notify_all();
    emit_state_changed(root, prev, FiberState::Unloading, None);
    unload_fiber(root).await;
    let prev2 = {
        let mut st = root.inner.lock().expect("fiber lock");
        let prev2 = st.state;
        st.state = FiberState::Disposed;
        st.provided.clear();
        prev2
    };
    root.transition_notify.notify_all();
    emit_state_changed(root, prev2, FiberState::Disposed, None);
    root.engine.settle_arc().await;
    Ok(())
}

#[allow(dead_code)]
fn _keep_effect_handle_in_scope(_h: &EffectHandle) {}

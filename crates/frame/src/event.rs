use crate::context::Context;
use crate::effect::EffectHandle;
use crate::error::FrameError;
use crate::fiber::{Engine, FiberId, FiberState};
use crate::loader::ConfigValue;
use crate::sync::{BoxFuture, join_all};
use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub trait Event: Send + Sync + 'static {
    const NAME: &'static str;
}

pub trait RequestEvent: Send + Sync + 'static {
    const NAME: &'static str;
    type Output: Send + 'static;
}

pub trait WaterfallEvent: Send + Sync + 'static {
    const NAME: &'static str;
    type Payload: Send + 'static;
    type Output: Send + 'static;
}

pub type WaterfallFinal<E> = Box<
    dyn FnOnce(
            <E as WaterfallEvent>::Payload,
        ) -> BoxFuture<
            'static,
            Result<<E as WaterfallEvent>::Output, FrameError>,
        > + Send,
>;

pub struct WaterfallNext<E: WaterfallEvent> {
    pub(crate) inner: Option<WaterfallFinal<E>>,
    _marker: PhantomData<E>,
}

impl<E: WaterfallEvent> WaterfallNext<E> {
    pub(crate) fn new(f: WaterfallFinal<E>) -> Self {
        Self {
            inner: Some(f),
            _marker: PhantomData,
        }
    }

    pub fn call(
        mut self,
        payload: E::Payload,
    ) -> BoxFuture<'static, Result<E::Output, FrameError>> {
        let f = self.inner.take().expect("waterfall next consumed once");
        f(payload)
    }
}

#[derive(Clone, Debug)]
pub struct UpdatePayload {
    pub fiber_id: FiberId,
    pub plugin_name: String,
    pub config: ConfigValue,
}

pub struct UpdateConfig;

impl WaterfallEvent for UpdateConfig {
    const NAME: &'static str = "internal/update";
    type Payload = UpdatePayload;
    type Output = ConfigValue;
}

pub type UpdateNext = WaterfallNext<UpdateConfig>;

#[derive(Clone, Copy, Debug, Default)]
pub struct ListenerOptions {
    pub prepend: bool,
    pub global: bool,
}

pub type EventFilter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;

pub(crate) type SyncListener<E> =
    Arc<dyn Fn(&Context, &E) + Send + Sync + 'static>;
pub(crate) type OnceListener<E> =
    Arc<Mutex<Option<Box<dyn FnOnce(&Context, &E) + Send + 'static>>>>;
pub(crate) type AsyncListener<E> = Arc<
    dyn Fn(&Context, &E) -> BoxFuture<'static, Result<(), FrameError>>
        + Send
        + Sync
        + 'static,
>;
pub(crate) type BailListener<E> = Arc<
    dyn Fn(
            &Context,
            &E,
        ) -> Result<Option<<E as RequestEvent>::Output>, FrameError>
        + Send
        + Sync
        + 'static,
>;
pub(crate) type SerialListener<E> = Arc<
    dyn Fn(
            &Context,
            &E,
        ) -> BoxFuture<
            'static,
            Result<Option<<E as RequestEvent>::Output>, FrameError>,
        > + Send
        + Sync
        + 'static,
>;
pub(crate) type WaterfallListener<E> = Arc<
    dyn Fn(
            &Context,
            <E as WaterfallEvent>::Payload,
            WaterfallNext<E>,
        ) -> BoxFuture<
            'static,
            Result<<E as WaterfallEvent>::Output, FrameError>,
        > + Send
        + Sync
        + 'static,
>;

pub(crate) enum ListenerKind<E: 'static> {
    Sync(SyncListener<E>),
    Once(OnceListener<E>),
    Async(AsyncListener<E>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EventKey {
    pub(crate) name: &'static str,
    pub(crate) type_id: TypeId,
}

impl EventKey {
    pub(crate) fn of<E: 'static>(name: &'static str) -> Self {
        Self {
            name,
            type_id: TypeId::of::<E>(),
        }
    }
}

pub(crate) struct HookEntry {
    pub(crate) id: u64,
    pub(crate) ctx: Context,
    pub(crate) global: bool,
    pub(crate) removed: AtomicBool,
    pub(crate) callback: Arc<dyn Any + Send + Sync>,
    pub(crate) effect: Mutex<Option<EffectHandle>>,
}

fn assert_fiber_active(
    engine: &Arc<Engine>,
    fiber_id: FiberId,
) -> Result<(), FrameError> {
    let eng = engine.state.lock().expect("engine lock");
    let fiber = eng
        .fibers
        .get(&fiber_id)
        .ok_or_else(FrameError::inactive_fiber)?;
    let st = fiber.inner.lock().expect("fiber lock");
    if st.disposing
        || matches!(st.state, FiberState::Unloading | FiberState::Disposed)
    {
        Err(FrameError::inactive_fiber())
    } else {
        Ok(())
    }
}

pub(crate) fn register_hook(
    ctx: &Context,
    key: EventKey,
    options: ListenerOptions,
    callback: Arc<dyn Any + Send + Sync>,
) -> Result<EffectHandle, FrameError> {
    let engine = ctx.engine();
    assert_fiber_active(engine, ctx.fiber_id())?;

    let is_local_update =
        key.type_id == TypeId::of::<UpdateConfig>() && !options.global;

    let (hook_id, entry) = {
        let mut eng = engine.state.lock().expect("engine lock");
        let id = eng.next_listener_id;
        eng.next_listener_id += 1;
        let entry = Arc::new(HookEntry {
            id,
            ctx: ctx.clone(),
            global: options.global,
            removed: AtomicBool::new(false),
            callback,
            effect: Mutex::new(None),
        });

        if is_local_update {
            let fiber = eng
                .fibers
                .get(&ctx.fiber_id())
                .cloned()
                .ok_or_else(FrameError::inactive_fiber)?;
            let mut inner = fiber.inner.lock().expect("fiber lock");
            if options.prepend {
                inner.local_update_hooks.insert(0, Arc::clone(&entry));
            } else {
                inner.local_update_hooks.push(Arc::clone(&entry));
            }
        } else {
            let list = eng.hooks.entry(key).or_default();
            if options.prepend {
                list.insert(0, Arc::clone(&entry));
            } else {
                list.push(Arc::clone(&entry));
            }
        }
        (id, entry)
    };

    let eng_for_clean = Arc::clone(engine);
    let fiber_id = ctx.fiber_id();
    let entry_for_clean = Arc::clone(&entry);
    let handle = ctx.register_sync_effect(
        format!("listener:{}", key.name),
        move || {
            entry_for_clean.removed.store(true, Ordering::Release);
            let mut eng = eng_for_clean.state.lock().expect("engine lock");
            if is_local_update {
                if let Some(fiber) = eng.fibers.get(&fiber_id) {
                    let mut inner = fiber.inner.lock().expect("fiber lock");
                    inner.local_update_hooks.retain(|h| h.id != hook_id);
                }
            } else if let Some(list) = eng.hooks.get_mut(&key) {
                list.retain(|h| h.id != hook_id);
            }
            Ok(())
        },
    )?;

    *entry.effect.lock().expect("hook effect lock") = Some(handle.clone());
    Ok(handle)
}

pub(crate) fn snapshot_hooks(
    engine: &Arc<Engine>,
    key: EventKey,
    filter: Option<&EventFilter>,
) -> Vec<Arc<HookEntry>> {
    let list = {
        let eng = engine.state.lock().expect("engine lock");
        eng.hooks.get(&key).cloned().unwrap_or_default()
    };
    list.into_iter()
        .filter(|h| {
            !h.removed.load(Ordering::Acquire)
                && (h.global || filter.is_none_or(|pred| pred(&h.ctx)))
        })
        .collect()
}

pub(crate) fn snapshot_update_hooks(
    engine: &Arc<Engine>,
    fiber_id: FiberId,
) -> Vec<Arc<HookEntry>> {
    let key = EventKey::of::<UpdateConfig>(UpdateConfig::NAME);
    let (local, global) = {
        let eng = engine.state.lock().expect("engine lock");
        let local = eng
            .fibers
            .get(&fiber_id)
            .map(|f| {
                f.inner
                    .lock()
                    .expect("fiber lock")
                    .local_update_hooks
                    .clone()
            })
            .unwrap_or_default();
        let global = eng.hooks.get(&key).cloned().unwrap_or_default();
        (local, global)
    };

    let mut out = Vec::with_capacity(local.len() + global.len());
    for h in local {
        if !h.removed.load(Ordering::Acquire) {
            out.push(h);
        }
    }
    for h in global {
        if !h.removed.load(Ordering::Acquire) {
            out.push(h);
        }
    }
    out
}

fn consume_once(hook: &Arc<HookEntry>) -> bool {
    if hook.removed.swap(true, Ordering::AcqRel) {
        return false;
    }
    let handle = hook.effect.lock().expect("hook effect lock").take();
    if let Some(h) = handle {
        drop(h.dispose());
    }
    true
}

pub(crate) fn dispatch_emit<E: Event>(
    engine: &Arc<Engine>,
    event: &E,
    filter: Option<&EventFilter>,
) {
    let key = EventKey::of::<E>(E::NAME);
    let hooks = snapshot_hooks(engine, key, filter);
    for hook in hooks {
        if let Some(kind) = hook.callback.downcast_ref::<ListenerKind<E>>() {
            match kind {
                ListenerKind::Sync(cb) => cb(&hook.ctx, event),
                ListenerKind::Once(cell) => {
                    if consume_once(&hook)
                        && let Some(cb) = cell.lock().expect("once lock").take()
                    {
                        cb(&hook.ctx, event);
                    }
                }
                ListenerKind::Async(_) => {}
            }
        }
    }
}

pub(crate) fn dispatch_parallel<E: Event>(
    engine: &Arc<Engine>,
    event: &E,
    filter: Option<&EventFilter>,
) -> BoxFuture<'static, Result<(), FrameError>> {
    let key = EventKey::of::<E>(E::NAME);
    let hooks = snapshot_hooks(engine, key, filter);
    let mut futs = Vec::new();
    for hook in hooks {
        if let Some(kind) = hook.callback.downcast_ref::<ListenerKind<E>>() {
            match kind {
                ListenerKind::Sync(cb) => cb(&hook.ctx, event),
                ListenerKind::Once(cell) => {
                    if consume_once(&hook)
                        && let Some(cb) = cell.lock().expect("once lock").take()
                    {
                        cb(&hook.ctx, event);
                    }
                }
                ListenerKind::Async(cb) => {
                    futs.push(cb(&hook.ctx, event));
                }
            }
        }
    }
    Box::pin(async move {
        let results = join_all(futs).await;
        let errors: Vec<FrameError> =
            results.into_iter().filter_map(Result::err).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(FrameError::aggregate(
                "parallel event dispatch failed",
                errors,
            ))
        }
    })
}

pub(crate) fn dispatch_bail<E: RequestEvent>(
    engine: &Arc<Engine>,
    event: &E,
    filter: Option<&EventFilter>,
) -> Result<Option<E::Output>, FrameError> {
    let key = EventKey::of::<E>(E::NAME);
    let hooks = snapshot_hooks(engine, key, filter);
    for hook in hooks {
        if let Some(cb) = hook.callback.downcast_ref::<BailListener<E>>()
            && let Some(out) = cb(&hook.ctx, event)?
        {
            return Ok(Some(out));
        }
    }
    Ok(None)
}

pub(crate) fn dispatch_serial<'a, E: RequestEvent>(
    engine: &Arc<Engine>,
    event: &'a E,
    filter: Option<&EventFilter>,
) -> BoxFuture<'a, Result<Option<E::Output>, FrameError>> {
    let key = EventKey::of::<E>(E::NAME);
    let hooks = snapshot_hooks(engine, key, filter);
    Box::pin(async move {
        for hook in hooks {
            if let Some(cb) = hook.callback.downcast_ref::<SerialListener<E>>()
            {
                if let Some(out) = cb(&hook.ctx, event).await? {
                    return Ok(Some(out));
                }
            } else if let Some(cb) =
                hook.callback.downcast_ref::<BailListener<E>>()
                && let Some(out) = cb(&hook.ctx, event)?
            {
                return Ok(Some(out));
            }
        }
        Ok(None)
    })
}

pub(crate) fn run_waterfall_hooks<E: WaterfallEvent>(
    hooks: Vec<Arc<HookEntry>>,
    payload: E::Payload,
    final_step: WaterfallFinal<E>,
) -> BoxFuture<'static, Result<E::Output, FrameError>> {
    let mut next: WaterfallFinal<E> = final_step;
    for hook in hooks.into_iter().rev() {
        if let Some(cb) = hook.callback.downcast_ref::<WaterfallListener<E>>() {
            let cb = Arc::clone(cb);
            let hook_ctx = hook.ctx.clone();
            let prev = next;
            next = Box::new(move |p: E::Payload| {
                cb(&hook_ctx, p, WaterfallNext::new(prev))
            });
        }
    }
    next(payload)
}

pub(crate) fn dispatch_waterfall<E: WaterfallEvent>(
    engine: &Arc<Engine>,
    payload: E::Payload,
    filter: Option<&EventFilter>,
    final_step: WaterfallFinal<E>,
) -> BoxFuture<'static, Result<E::Output, FrameError>> {
    let key = EventKey::of::<E>(E::NAME);
    let hooks = snapshot_hooks(engine, key, filter);
    run_waterfall_hooks::<E>(hooks, payload, final_step)
}

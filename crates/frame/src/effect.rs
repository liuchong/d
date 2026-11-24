use crate::error::{ErrorKind, FrameError};
use crate::fiber::{Engine, FiberId, FiberRecord, FiberState};
use crate::sync::{BoxFuture, SignalValue};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectPhase {
    Initializing,
    Ready,
    Cleaning,
    Disposed,
}

pub(crate) enum CleanupStep {
    Sync(Box<dyn FnOnce() -> Result<(), FrameError> + Send>),
    Async(
        Box<dyn FnOnce() -> BoxFuture<'static, Result<(), FrameError>> + Send>,
    ),
}

struct EffectState {
    phase: EffectPhase,
    dispose_requested: bool,
    steps: Vec<CleanupStep>,
}

pub(crate) struct EffectInner {
    pub(crate) id: u64,
    label: String,
    owner: Arc<FiberRecord>,
    engine: Arc<Engine>,
    state: Mutex<EffectState>,
    dispose_done: SignalValue<Result<(), FrameError>>,
}

#[derive(Clone)]
pub struct EffectHandle {
    pub(crate) inner: Arc<EffectInner>,
}

impl EffectHandle {
    pub fn label(&self) -> &str {
        &self.inner.label
    }

    pub fn is_disposed(&self) -> bool {
        let st = self.inner.state.lock().expect("effect lock");
        matches!(st.phase, EffectPhase::Disposed)
    }

    pub fn dispose(&self) -> BoxFuture<'static, Result<(), FrameError>> {
        self.inner.dispose()
    }
}

impl std::fmt::Debug for EffectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectHandle")
            .field("id", &self.inner.id)
            .field("label", &self.inner.label)
            .finish()
    }
}

#[derive(Clone)]
pub struct EffectScope {
    inner: Arc<EffectInner>,
}

impl EffectScope {
    pub fn defer<F>(&self, cleanup: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.defer_result(move || {
            cleanup();
            Ok(())
        });
    }

    pub fn defer_result<F>(&self, cleanup: F)
    where
        F: FnOnce() -> Result<(), FrameError> + Send + 'static,
    {
        let mut st = self.inner.state.lock().expect("effect lock");
        if matches!(st.phase, EffectPhase::Initializing) {
            st.steps.push(CleanupStep::Sync(Box::new(cleanup)));
        }
    }

    pub fn defer_async<F>(&self, cleanup: F)
    where
        F: FnOnce() -> BoxFuture<'static, Result<(), FrameError>>
            + Send
            + 'static,
    {
        let mut st = self.inner.state.lock().expect("effect lock");
        if matches!(st.phase, EffectPhase::Initializing) {
            st.steps.push(CleanupStep::Async(Box::new(cleanup)));
        }
    }

    pub fn child_effect<F>(
        &self,
        label: impl Into<String>,
        setup: F,
    ) -> BoxFuture<'static, Result<EffectHandle, FrameError>>
    where
        F: FnOnce(EffectScope) -> BoxFuture<'static, Result<(), FrameError>>
            + Send
            + 'static,
    {
        let parent = Arc::clone(&self.inner);
        let label = label.into();
        Box::pin(async move {
            let child = create_effect(
                Arc::clone(&parent.engine),
                parent.owner.id,
                label,
                setup,
            )
            .await?;

            // Detach from top-level sibling set so parent owns its LIFO order.
            {
                let mut owner_inner =
                    parent.owner.inner.lock().expect("fiber lock");
                owner_inner.effects.remove(&child.inner.id);
            }

            let child_for_cleanup = child.clone();
            let mut st = parent.state.lock().expect("effect lock");
            if matches!(st.phase, EffectPhase::Initializing) {
                st.steps.push(CleanupStep::Async(Box::new(move || {
                    child_for_cleanup.dispose()
                })));
            }
            drop(st);

            Ok(child)
        })
    }
}

impl EffectInner {
    pub(crate) fn dispose(
        self: &Arc<Self>,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        enum Action {
            AwaitDone,
            RunCleanup(Vec<CleanupStep>),
        }

        let action = {
            let mut st = self.state.lock().expect("effect lock");
            match st.phase {
                EffectPhase::Initializing => {
                    st.dispose_requested = true;
                    Action::AwaitDone
                }
                EffectPhase::Ready => {
                    st.phase = EffectPhase::Cleaning;
                    Action::RunCleanup(std::mem::take(&mut st.steps))
                }
                EffectPhase::Cleaning | EffectPhase::Disposed => {
                    Action::AwaitDone
                }
            }
        };

        let inner = Arc::clone(self);
        match action {
            Action::AwaitDone => inner.dispose_done.wait(),
            Action::RunCleanup(steps) => {
                if steps.iter().all(|s| matches!(s, CleanupStep::Sync(_))) {
                    let res =
                        run_sync_cleanup_steps(&inner.engine, steps, true);
                    inner.finish_disposal(res.clone());
                    Box::pin(async move { res })
                } else {
                    let task_inner = Arc::clone(&inner);
                    inner.engine.spawn_tracked(Box::pin(async move {
                        let res =
                            run_cleanup_steps(&task_inner.engine, steps, true)
                                .await;
                        task_inner.finish_disposal(res);
                    }));
                    inner.dispose_done.wait()
                }
            }
        }
    }

    fn finish_disposal(&self, res: Result<(), FrameError>) {
        {
            let mut owner_inner = self.owner.inner.lock().expect("fiber lock");
            owner_inner.effects.remove(&self.id);
        }
        {
            let mut st = self.state.lock().expect("effect lock");
            st.phase = EffectPhase::Disposed;
        }
        self.dispose_done.set(res);
    }
}

fn fold_cleanup_errors(errors: Vec<FrameError>) -> Result<(), FrameError> {
    if errors.is_empty() {
        Ok(())
    } else if errors.len() == 1 {
        Err(errors.into_iter().next().expect("single error"))
    } else {
        Err(FrameError::aggregate(
            "multiple effect cleanup steps failed",
            errors,
        ))
    }
}

fn run_sync_cleanup_steps(
    engine: &Arc<Engine>,
    mut steps: Vec<CleanupStep>,
    report_errors: bool,
) -> Result<(), FrameError> {
    steps.reverse();
    let mut errors = Vec::new();
    for step in steps {
        if let CleanupStep::Sync(f) = step
            && let Err(err) = f()
        {
            if report_errors {
                engine.report_error(&err);
            }
            errors.push(err);
        }
    }
    fold_cleanup_errors(errors)
}

async fn run_cleanup_steps(
    engine: &Arc<Engine>,
    mut steps: Vec<CleanupStep>,
    report_errors: bool,
) -> Result<(), FrameError> {
    steps.reverse();
    let mut errors = Vec::new();
    for step in steps {
        let step_res = match step {
            CleanupStep::Sync(f) => f(),
            CleanupStep::Async(f) => f().await,
        };
        if let Err(err) = step_res {
            if report_errors {
                engine.report_error(&err);
            }
            errors.push(err);
        }
    }
    fold_cleanup_errors(errors)
}

fn allocate_effect(
    engine: &Arc<Engine>,
    owner_id: FiberId,
    label: String,
) -> Result<Arc<EffectInner>, FrameError> {
    let mut eng = engine.state.lock().expect("engine lock");
    let owner = eng
        .fibers
        .get(&owner_id)
        .cloned()
        .ok_or_else(FrameError::inactive_fiber)?;
    let id = eng.next_effect_id;
    eng.next_effect_id += 1;

    let mut owner_inner = owner.inner.lock().expect("fiber lock");
    if owner_inner.disposing
        || matches!(
            owner_inner.state,
            FiberState::Unloading | FiberState::Disposed
        )
    {
        return Err(FrameError::new(
            ErrorKind::InactiveFiber,
            "cannot register effect on unloading or disposed fiber",
        ));
    }

    let inner = Arc::new(EffectInner {
        id,
        label,
        owner: Arc::clone(&owner),
        engine: Arc::clone(engine),
        state: Mutex::new(EffectState {
            phase: EffectPhase::Initializing,
            dispose_requested: false,
            steps: Vec::new(),
        }),
        dispose_done: SignalValue::new(),
    });
    owner_inner.effects.insert(id, Arc::clone(&inner));
    Ok(inner)
}

fn take_rollback_steps(effect: &EffectInner) -> Vec<CleanupStep> {
    let mut st = effect.state.lock().expect("effect lock");
    st.phase = EffectPhase::Cleaning;
    std::mem::take(&mut st.steps)
}

fn finish_ready_or_raced(effect: &EffectInner) -> Option<Vec<CleanupStep>> {
    let mut st = effect.state.lock().expect("effect lock");
    if st.dispose_requested {
        st.phase = EffectPhase::Cleaning;
        Some(std::mem::take(&mut st.steps))
    } else {
        st.phase = EffectPhase::Ready;
        None
    }
}

pub(crate) fn create_effect_sync<F>(
    engine: Arc<Engine>,
    owner_id: FiberId,
    label: impl Into<String>,
    setup: F,
) -> Result<EffectHandle, FrameError>
where
    F: FnOnce(EffectScope) -> Result<(), FrameError>,
{
    let effect = allocate_effect(&engine, owner_id, label.into())?;
    let scope = EffectScope {
        inner: Arc::clone(&effect),
    };
    if let Err(setup_err) = setup(scope) {
        let steps = take_rollback_steps(&effect);
        if steps.iter().all(|s| matches!(s, CleanupStep::Sync(_))) {
            let cleanup_res =
                run_sync_cleanup_steps(&effect.engine, steps, true);
            effect.finish_disposal(cleanup_res);
        } else {
            let eff = Arc::clone(&effect);
            effect.engine.spawn_tracked(Box::pin(async move {
                let cleanup_res =
                    run_cleanup_steps(&eff.engine, steps, true).await;
                eff.finish_disposal(cleanup_res);
            }));
        }
        return Err(setup_err);
    }

    if let Some(steps) = finish_ready_or_raced(&effect) {
        if steps.iter().all(|s| matches!(s, CleanupStep::Sync(_))) {
            let cleanup_res =
                run_sync_cleanup_steps(&effect.engine, steps, true);
            effect.finish_disposal(cleanup_res);
        } else {
            let eff = Arc::clone(&effect);
            effect.engine.spawn_tracked(Box::pin(async move {
                let cleanup_res =
                    run_cleanup_steps(&eff.engine, steps, true).await;
                eff.finish_disposal(cleanup_res);
            }));
        }
    }
    Ok(EffectHandle { inner: effect })
}

pub(crate) fn create_effect<F>(
    engine: Arc<Engine>,
    owner_id: FiberId,
    label: impl Into<String>,
    setup: F,
) -> BoxFuture<'static, Result<EffectHandle, FrameError>>
where
    F: FnOnce(EffectScope) -> BoxFuture<'static, Result<(), FrameError>>
        + Send
        + 'static,
{
    let label = label.into();
    Box::pin(async move {
        let effect = allocate_effect(&engine, owner_id, label)?;
        let scope = EffectScope {
            inner: Arc::clone(&effect),
        };
        let setup_res = setup(scope).await;

        match setup_res {
            Err(setup_err) => {
                let steps = take_rollback_steps(&effect);
                let cleanup_res =
                    run_cleanup_steps(&effect.engine, steps, true).await;
                effect.finish_disposal(cleanup_res);
                Err(setup_err)
            }
            Ok(()) => {
                if let Some(steps) = finish_ready_or_raced(&effect) {
                    let cleanup_res =
                        run_cleanup_steps(&effect.engine, steps, true).await;
                    effect.finish_disposal(cleanup_res);
                }
                Ok(EffectHandle { inner: effect })
            }
        }
    })
}

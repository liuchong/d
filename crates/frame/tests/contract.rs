use frame::{
    ConfigValue, Context, Deps, EntryPatch, EntryTarget, ErrorKind, Event,
    EventFilter, FiberId, FiberState, FiberStateChanged, FrameError,
    FrameOptions, InsertAnchor, ListenerOptions, Loader, LoaderPlugin,
    LoaderService, Plugin, PluginEntry, RequestEvent, RuntimeReady,
    ScopedService, Service, ServiceAction, ServiceChanged, ServiceKey, Spawner,
    StaticResolver, UpdateConfig, UpdateNext, UpdatePayload, ValidationIssue,
    WaterfallEvent, WaterfallNext,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context as TaskContext, Poll, Wake, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Minimal pure-std async runtime for contract tests
// ---------------------------------------------------------------------------

struct TaskState {
    future: Option<BoxFut<'static, ()>>,
    polling: bool,
    woken_while_polling: bool,
    scheduled: bool,
}

struct Task {
    state: Mutex<TaskState>,
    sender: mpsc::Sender<Arc<Task>>,
}

impl Task {
    fn schedule(self: &Arc<Self>) {
        let mut st = self.state.lock().expect("task lock");
        if st.future.is_none() && !st.polling {
            return;
        }
        if st.polling {
            st.woken_while_polling = true;
            return;
        }
        if !st.scheduled {
            st.scheduled = true;
            let cloned = Arc::clone(self);
            let tx = self.sender.clone();
            drop(st);
            let _ = tx.send(cloned);
        }
    }
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        self.schedule();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.schedule();
    }
}

struct TestSpawner {
    sender: Option<mpsc::Sender<Arc<Task>>>,
    workers: Vec<JoinHandle<()>>,
}

impl TestSpawner {
    fn new(threads: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Arc<Task>>();
        let rx = Arc::new(Mutex::new(rx));
        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            let rx = Arc::clone(&rx);
            workers.push(thread::spawn(move || {
                loop {
                    let task = {
                        let guard = rx.lock().expect("rx lock");
                        match guard.recv() {
                            Ok(t) => t,
                            Err(_) => break,
                        }
                    };
                    let mut st = task.state.lock().expect("task lock");
                    let fut = st.future.take();
                    st.scheduled = false;
                    st.polling = true;
                    st.woken_while_polling = false;
                    drop(st);

                    if let Some(mut fut) = fut {
                        let waker = Waker::from(Arc::clone(&task));
                        let mut cx = TaskContext::from_waker(&waker);
                        match fut.as_mut().poll(&mut cx) {
                            Poll::Ready(()) => {
                                let mut st =
                                    task.state.lock().expect("task lock");
                                st.polling = false;
                                drop(st);
                                drop(fut);
                            }
                            Poll::Pending => {
                                let mut st =
                                    task.state.lock().expect("task lock");
                                st.polling = false;
                                st.future = Some(fut);
                                let rewake =
                                    std::mem::take(&mut st.woken_while_polling);
                                if rewake && !st.scheduled {
                                    st.scheduled = true;
                                    let cloned = Arc::clone(&task);
                                    let sender = task.sender.clone();
                                    drop(st);
                                    let _ = sender.send(cloned);
                                }
                            }
                        }
                    } else {
                        let mut st = task.state.lock().expect("task lock");
                        st.polling = false;
                    }
                }
            }));
        }
        Arc::new(Self {
            sender: Some(tx),
            workers,
        })
    }
}

impl Drop for TestSpawner {
    fn drop(&mut self) {
        self.sender.take();
        if !thread::panicking() {
            for handle in self.workers.drain(..) {
                let _ = handle.join();
            }
        }
    }
}

impl Spawner for TestSpawner {
    fn spawn(&self, fut: frame::BoxFuture<'static, ()>) {
        if let Some(tx) = &self.sender {
            let task = Arc::new(Task {
                state: Mutex::new(TaskState {
                    future: Some(fut),
                    polling: false,
                    woken_while_polling: false,
                    scheduled: true,
                }),
                sender: tx.clone(),
            });
            let _ = tx.send(task);
        }
    }
}

struct ThreadWake {
    thread: thread::Thread,
    notified: AtomicBool,
}

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.notified.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

fn block_on<F: Future>(mut fut: F) -> F::Output {
    let wake = Arc::new(ThreadWake {
        thread: thread::current(),
        notified: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&wake));
    let mut cx = TaskContext::from_waker(&waker);
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        wake.notified.store(false, Ordering::Release);
        if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
            return output;
        }
        while !wake.notified.load(Ordering::Acquire) {
            let now = Instant::now();
            if now >= deadline {
                panic!("contract test future exceeded 5s hard deadline");
            }
            let wait = (deadline - now).min(Duration::from_millis(50));
            thread::park_timeout(wait);
        }
    }
}

#[derive(Clone, Default)]
struct AsyncLatch {
    state: Arc<Mutex<(bool, Vec<Waker>)>>,
}

impl AsyncLatch {
    fn new() -> Self {
        Self::default()
    }

    fn open(&self) {
        let wakers = {
            let mut st = self.state.lock().expect("latch lock");
            st.0 = true;
            std::mem::take(&mut st.1)
        };
        for w in wakers {
            w.wake();
        }
    }

    fn wait(&self) -> BoxFut<'static, ()> {
        struct WaitLatch(Arc<Mutex<(bool, Vec<Waker>)>>);
        impl Future for WaitLatch {
            type Output = ();
            fn poll(
                self: Pin<&mut Self>,
                cx: &mut TaskContext<'_>,
            ) -> Poll<()> {
                let mut st = self.0.lock().expect("latch lock");
                if st.0 {
                    Poll::Ready(())
                } else {
                    if !st.1.iter().any(|w| w.will_wake(cx.waker())) {
                        st.1.push(cx.waker().clone());
                    }
                    Poll::Pending
                }
            }
        }
        Box::pin(WaitLatch(Arc::clone(&self.state)))
    }
}

fn yield_once() -> BoxFut<'static, ()> {
    struct Yield(bool);
    impl Future for Yield {
        type Output = ();
        fn poll(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    Box::pin(Yield(false))
}

fn join2<A, B, FA, FB>(fa: FA, fb: FB) -> BoxFut<'static, (A, B)>
where
    A: Send + Unpin + 'static,
    B: Send + Unpin + 'static,
    FA: Future<Output = A> + Send + 'static,
    FB: Future<Output = B> + Send + 'static,
{
    struct Join2<A, B, FA, FB> {
        fa: Option<Pin<Box<FA>>>,
        fb: Option<Pin<Box<FB>>>,
        a: Option<A>,
        b: Option<B>,
    }
    impl<A, B, FA, FB> Future for Join2<A, B, FA, FB>
    where
        A: Unpin,
        B: Unpin,
        FA: Future<Output = A>,
        FB: Future<Output = B>,
    {
        type Output = (A, B);
        fn poll(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<Self::Output> {
            if let Some(mut fa) = self.fa.take() {
                match fa.as_mut().poll(cx) {
                    Poll::Ready(v) => self.a = Some(v),
                    Poll::Pending => self.fa = Some(fa),
                }
            }
            if let Some(mut fb) = self.fb.take() {
                match fb.as_mut().poll(cx) {
                    Poll::Ready(v) => self.b = Some(v),
                    Poll::Pending => self.fb = Some(fb),
                }
            }
            if self.a.is_some() && self.b.is_some() {
                Poll::Ready((
                    self.a.take().expect("a"),
                    self.b.take().expect("b"),
                ))
            } else {
                Poll::Pending
            }
        }
    }
    Box::pin(Join2 {
        fa: Some(Box::pin(fa)),
        fb: Some(Box::pin(fb)),
        a: None,
        b: None,
    })
}

fn new_ctx() -> (Context, Arc<Mutex<Vec<FrameError>>>, Arc<TestSpawner>) {
    let spawner = TestSpawner::new(4);
    let errors = Arc::new(Mutex::new(Vec::new()));
    let errors_cb = Arc::clone(&errors);
    let opts = FrameOptions::new(spawner.clone()).with_on_error(Arc::new(
        move |err| {
            errors_cb.lock().expect("err lock").push(err.clone());
        },
    ));
    (Context::new(opts), errors, spawner)
}

fn map_i64(key: &str, val: i64) -> ConfigValue {
    let mut map = BTreeMap::new();
    map.insert(key.to_string(), ConfigValue::Int(val));
    ConfigValue::Map(map)
}

// ---------------------------------------------------------------------------
// F01: Service ownership & readiness probe
// ---------------------------------------------------------------------------

struct DbService {
    generation: u64,
    ready: Arc<AtomicBool>,
}

impl Service for DbService {
    const NAME: &'static str = "db";
}

struct DbProvider {
    generation: u64,
    ready: Arc<AtomicBool>,
    seen_self_during_load: Arc<AtomicBool>,
}

impl Plugin for DbProvider {
    fn name(&self) -> &str {
        "db-provider"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let generation = self.generation;
        let ready = Arc::clone(&self.ready);
        let seen = Arc::clone(&self.seen_self_during_load);
        Box::pin(async move {
            let check = Arc::clone(&ready);
            ctx.provide_with_check(
                Arc::new(DbService {
                    generation,
                    ready: Arc::clone(&ready),
                }),
                Arc::new(move |svc: &DbService| {
                    let _ = svc.generation;
                    check.load(Ordering::Acquire)
                }),
            )
            .await?;
            // Provider sees its own provided service during Loading.
            seen.store(ctx.get::<DbService>().is_some(), Ordering::Release);
            Ok(())
        })
    }
}

struct DbConsumer {
    activated: Arc<AtomicUsize>,
}

impl Plugin for DbConsumer {
    fn name(&self) -> &str {
        "db-consumer"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<DbService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let activated = Arc::clone(&self.activated);
        Box::pin(async move {
            let db = ctx.require::<DbService>()?;
            assert!(db.ready.load(Ordering::Acquire));
            activated.fetch_add(1, Ordering::AcqRel);
            Ok(())
        })
    }
}

#[test]
fn f01_service_ownership_and_readiness() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let ready = Arc::new(AtomicBool::new(false));
        let seen_self = Arc::new(AtomicBool::new(false));
        let p = ctx
            .plugin(
                DbProvider {
                    generation: 1,
                    ready: Arc::clone(&ready),
                    seen_self_during_load: Arc::clone(&seen_self),
                },
                ConfigValue::Null,
            )
            .expect("mount provider");
        p.wait().await.expect("provider active");
        assert!(seen_self.load(Ordering::Acquire));

        // Duplicate provider in same scope fails.
        let dup_err = ctx
            .provide(Arc::new(DbService {
                generation: 99,
                ready: Arc::new(AtomicBool::new(true)),
            }))
            .await
            .expect_err("duplicate provide should fail");
        assert_eq!(dup_err.kind, ErrorKind::DuplicateService);

        // Non-owner update fails.
        let update_err = ctx
            .update_service(Arc::new(DbService {
                generation: 2,
                ready: Arc::new(AtomicBool::new(true)),
            }))
            .expect_err("non-owner update must fail");
        assert_eq!(update_err.kind, ErrorKind::NotServiceOwner);

        // Consumer stays Pending while readiness probe returns false.
        let activated = Arc::new(AtomicUsize::new(0));
        let consumer = ctx
            .plugin(
                DbConsumer {
                    activated: Arc::clone(&activated),
                },
                ConfigValue::Null,
            )
            .expect("mount consumer");
        ctx.settle().await;
        assert_eq!(consumer.state(), FiberState::Pending);
        assert_eq!(activated.load(Ordering::Acquire), 0);
        assert!(ctx.get::<DbService>().is_none());

        // Flip readiness and notify; consumer wakes and activates.
        ready.store(true, Ordering::Release);
        ctx.notify_service::<DbService>().expect("notify");
        consumer.wait().await.expect("consumer active");
        assert_eq!(consumer.state(), FiberState::Active);
        assert_eq!(activated.load(Ordering::Acquire), 1);

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F02: Dependency arrival, loss, and replacement cascade
// ---------------------------------------------------------------------------

struct SeqService(u64);
impl Service for SeqService {
    const NAME: &'static str = "seq";
}

struct SeqProvider(u64);
impl Plugin for SeqProvider {
    fn name(&self) -> &str {
        "seq-provider"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let generation = self.0;
        Box::pin(async move {
            ctx.provide(Arc::new(SeqService(generation))).await?;
            Ok(())
        })
    }
}

struct SeqConsumer {
    log: Arc<Mutex<Vec<String>>>,
}

impl Plugin for SeqConsumer {
    fn name(&self) -> &str {
        "seq-consumer"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<SeqService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let log = Arc::clone(&self.log);
        Box::pin(async move {
            let svc = ctx.require::<SeqService>()?;
            let generation = svc.0;
            log.lock().expect("log").push(format!("load:{generation}"));
            let log_drop = Arc::clone(&log);
            ctx.effect("seq-consumer-res", move |scope| {
                Box::pin(async move {
                    scope.defer(move || {
                        log_drop
                            .lock()
                            .expect("log")
                            .push(format!("unload:{generation}"));
                    });
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f02_dependency_cascade_lifecycle() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let log = Arc::new(Mutex::new(Vec::new()));
        let consumer = ctx
            .plugin(
                SeqConsumer {
                    log: Arc::clone(&log),
                },
                ConfigValue::Null,
            )
            .expect("mount consumer");
        ctx.settle().await;
        assert_eq!(consumer.state(), FiberState::Pending);

        // Provider v1 arrives -> consumer activates.
        let p1 = ctx.plugin(SeqProvider(1), ConfigValue::Null).expect("p1");
        ctx.settle().await;
        assert_eq!(consumer.state(), FiberState::Active);

        // Provider v1 removed -> consumer unloads cleanly back to Pending.
        p1.dispose().await.expect("dispose p1");
        ctx.settle().await;
        assert_eq!(consumer.state(), FiberState::Pending);

        // Provider v2 arrives -> consumer activates against v2.
        let p2 = ctx.plugin(SeqProvider(2), ConfigValue::Null).expect("p2");
        ctx.settle().await;
        assert_eq!(consumer.state(), FiberState::Active);

        let events = log.lock().expect("log").clone();
        assert_eq!(events, vec!["load:1", "unload:1", "load:2"]);

        p2.dispose().await.expect("dispose p2");
        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F03: Scope isolation and inheritance
// ---------------------------------------------------------------------------

struct TagService(&'static str);
impl Service for TagService {
    const NAME: &'static str = "tag";
}

struct SharedService(&'static str);
impl Service for SharedService {
    const NAME: &'static str = "shared";
}

struct TagReader(Arc<Mutex<Option<(&'static str, &'static str)>>>);
impl Plugin for TagReader {
    fn name(&self) -> &str {
        "tag-reader"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<TagService>().with::<SharedService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let out = Arc::clone(&self.0);
        Box::pin(async move {
            let t = ctx.require::<TagService>()?.0;
            let s = ctx.require::<SharedService>()?.0;
            *out.lock().expect("out") = Some((t, s));
            Ok(())
        })
    }
}

#[test]
fn f03_scope_isolation_and_inheritance() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        ctx.provide(Arc::new(TagService("root")))
            .await
            .expect("tag");
        ctx.provide(Arc::new(SharedService("global")))
            .await
            .expect("shared");

        let root_seen = Arc::new(Mutex::new(None));
        ctx.plugin(TagReader(Arc::clone(&root_seen)), ConfigValue::Null)
            .expect("root reader");

        let iso = ctx.isolate::<TagService>();
        iso.provide(Arc::new(TagService("isolated")))
            .await
            .expect("iso tag");
        let iso_seen = Arc::new(Mutex::new(None));
        iso.plugin(TagReader(Arc::clone(&iso_seen)), ConfigValue::Null)
            .expect("iso reader");

        ctx.settle().await;
        assert_eq!(*root_seen.lock().expect("r"), Some(("root", "global")));
        assert_eq!(*iso_seen.lock().expect("i"), Some(("isolated", "global")));

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F04: Effect initialization race, rollback, and idempotent join
// ---------------------------------------------------------------------------

struct SlowSetupPlugin {
    entered: AsyncLatch,
    release: AsyncLatch,
    log: Arc<Mutex<Vec<String>>>,
}

impl Plugin for SlowSetupPlugin {
    fn name(&self) -> &str {
        "slow-setup"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let log = Arc::clone(&self.log);
        Box::pin(async move {
            ctx.effect("raced", move |scope| {
                Box::pin(async move {
                    let l1 = Arc::clone(&log);
                    scope.defer(move || {
                        l1.lock().expect("log").push("s1".to_string());
                    });
                    let l2 = Arc::clone(&log);
                    scope.defer_async(move || {
                        Box::pin(async move {
                            yield_once().await;
                            l2.lock().expect("log").push("s2".to_string());
                            Ok(())
                        })
                    });
                    entered.open();
                    release.wait().await;
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f04_effect_race_rollback_and_join() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        // Sub-case A: dispose races with suspended setup, joined concurrently.
        let entered = AsyncLatch::new();
        let release = AsyncLatch::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let fiber = ctx
            .plugin(
                SlowSetupPlugin {
                    entered: entered.clone(),
                    release: release.clone(),
                    log: Arc::clone(&log),
                },
                ConfigValue::Null,
            )
            .expect("mount slow");
        entered.wait().await;

        let d1 = fiber.dispose();
        let d2 = fiber.dispose();
        release.open();
        let (r1, r2) = join2(d1, d2).await;
        r1.expect("d1");
        r2.expect("d2");
        assert_eq!(*log.lock().expect("log"), vec!["s2", "s1"]);

        // Sub-case B: concurrent handle.dispose() on async cleanup runs once.
        let cleanup_runs = Arc::new(AtomicUsize::new(0));
        let clean_gate = AsyncLatch::new();
        let handle = ctx
            .effect("async-clean", {
                let runs = Arc::clone(&cleanup_runs);
                let gate = clean_gate.clone();
                move |scope| {
                    Box::pin(async move {
                        scope.defer_async(move || {
                            Box::pin(async move {
                                runs.fetch_add(1, Ordering::AcqRel);
                                gate.wait().await;
                                Ok(())
                            })
                        });
                        Ok(())
                    })
                }
            })
            .await
            .expect("effect ready");
        let hd1 = handle.dispose();
        let hd2 = handle.dispose();
        clean_gate.open();
        let (hr1, hr2) = join2(hd1, hd2).await;
        hr1.expect("hd1");
        hr2.expect("hd2");
        assert_eq!(cleanup_runs.load(Ordering::Acquire), 1);
        assert!(handle.is_disposed());

        // Sub-case C: setup failure rolls back defer + child_effect in LIFO.
        let rb_log = Arc::new(Mutex::new(Vec::new()));
        let err = ctx
            .effect("failing-setup", {
                let rb = Arc::clone(&rb_log);
                move |scope| {
                    Box::pin(async move {
                        let rb1 = Arc::clone(&rb);
                        scope.defer(move || {
                            rb1.lock().expect("rb").push("r1".to_string());
                        });
                        let rbc = Arc::clone(&rb);
                        scope
                            .child_effect("c1", move |cscope| {
                                Box::pin(async move {
                                    cscope.defer(move || {
                                        rbc.lock()
                                            .expect("rb")
                                            .push("c1".to_string());
                                    });
                                    Ok(())
                                })
                            })
                            .await?;
                        Err(FrameError::new(
                            ErrorKind::ActivationFailed,
                            "boom",
                        ))
                    })
                }
            })
            .await
            .expect_err("setup must fail");
        assert_eq!(err.kind, ErrorKind::ActivationFailed);
        assert_eq!(*rb_log.lock().expect("rb"), vec!["c1", "r1"]);

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F05: Sibling effect concurrent cleanup & failure isolation
// ---------------------------------------------------------------------------

struct MultiEffectPlugin {
    done1: Arc<AtomicBool>,
    done3: Arc<AtomicBool>,
}

impl Plugin for MultiEffectPlugin {
    fn name(&self) -> &str {
        "multi-effect"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let d1 = Arc::clone(&self.done1);
        let d3 = Arc::clone(&self.done3);
        Box::pin(async move {
            ctx.effect("e1", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || {
                        Box::pin(async move {
                            yield_once().await;
                            d1.store(true, Ordering::Release);
                            Ok(())
                        })
                    });
                    Ok(())
                })
            })
            .await?;
            ctx.effect("e2-fail", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || {
                        Box::pin(async move {
                            Err(FrameError::new(
                                ErrorKind::CleanupFailed,
                                "cleanup boom",
                            ))
                        })
                    });
                    Ok(())
                })
            })
            .await?;
            ctx.effect("e3", move |scope| {
                Box::pin(async move {
                    scope.defer(move || {
                        d3.store(true, Ordering::Release);
                    });
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f05_sibling_effect_isolation() {
    let (ctx, errors, _spawner) = new_ctx();
    block_on(async {
        let done1 = Arc::new(AtomicBool::new(false));
        let done3 = Arc::new(AtomicBool::new(false));
        let fiber = ctx
            .plugin(
                MultiEffectPlugin {
                    done1: Arc::clone(&done1),
                    done3: Arc::clone(&done3),
                },
                ConfigValue::Null,
            )
            .expect("mount");
        fiber.wait().await.expect("active");
        let _ = fiber.dispose().await;
        ctx.settle().await;

        assert!(done1.load(Ordering::Acquire));
        assert!(done3.load(Ordering::Acquire));
        {
            let errs = errors.lock().expect("errs");
            assert!(
                errs.iter().any(|e| e.message.contains("cleanup boom")),
                "expected cleanup failure logged in on_error, got: {errs:?}"
            );
        }

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F06: Unload retains dependency snapshot until cleanup completes
// ---------------------------------------------------------------------------

struct StorageService {
    flushes: AtomicUsize,
}

impl Service for StorageService {
    const NAME: &'static str = "storage";
}

struct StorageProvider(Arc<StorageService>);
impl Plugin for StorageProvider {
    fn name(&self) -> &str {
        "storage-provider"
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let svc = Arc::clone(&self.0);
        Box::pin(async move {
            ctx.provide(svc).await?;
            Ok(())
        })
    }
}

struct StorageConsumer;
impl Plugin for StorageConsumer {
    fn name(&self) -> &str {
        "storage-consumer"
    }

    fn deps(&self) -> Deps {
        Deps::empty().with::<StorageService>()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let ctx_cleanup = ctx.clone();
            ctx.effect("flush-on-close", move |scope| {
                Box::pin(async move {
                    scope.defer_async(move || {
                        Box::pin(async move {
                            // Yield so provider unpublish happens first;
                            // consumer's store snapshot must remain usable.
                            yield_once().await;
                            let storage =
                                ctx_cleanup.require::<StorageService>()?;
                            storage.flushes.fetch_add(1, Ordering::AcqRel);
                            Ok(())
                        })
                    });
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f06_unload_retains_dependency_snapshot() {
    let (ctx, errors, _spawner) = new_ctx();
    block_on(async {
        let storage = Arc::new(StorageService {
            flushes: AtomicUsize::new(0),
        });
        let p = ctx
            .plugin(StorageProvider(Arc::clone(&storage)), ConfigValue::Null)
            .expect("provider");
        let c = ctx
            .plugin(StorageConsumer, ConfigValue::Null)
            .expect("cons");
        ctx.settle().await;
        assert_eq!(c.state(), FiberState::Active);

        // Dispose provider directly: consumer must flush via retained store.
        p.dispose().await.expect("dispose provider");
        ctx.settle().await;
        assert_eq!(storage.flushes.load(Ordering::Acquire), 1);
        assert!(errors.lock().expect("errs").is_empty());
        assert_eq!(c.state(), FiberState::Pending);

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F07: Four event modes, snapshot dispatch, once, filter & auto-teardown
// ---------------------------------------------------------------------------

struct PingEvent(&'static str);
impl Event for PingEvent {
    const NAME: &'static str = "test/ping";
}

struct MatchRequest(i32);
impl RequestEvent for MatchRequest {
    const NAME: &'static str = "test/match";
    type Output = &'static str;
}

struct PipeEvent;
impl WaterfallEvent for PipeEvent {
    const NAME: &'static str = "test/pipe";
    type Payload = i32;
    type Output = i32;
}

#[test]
fn f07_events_dispatch_and_teardown() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let order = Arc::new(Mutex::new(Vec::new()));

        // Listener order + prepend + snapshot removal during emit.
        let o1 = Arc::clone(&order);
        let removed_handle: Arc<Mutex<Option<frame::EffectHandle>>> =
            Arc::new(Mutex::new(None));
        let rh_clone = Arc::clone(&removed_handle);
        ctx.on::<PingEvent, _>(
            move |_, ev| {
                o1.lock().expect("o").push(format!("second:{}", ev.0));
            },
            ListenerOptions::default(),
        )
        .expect("on second");

        let h_victim = ctx
            .on::<PingEvent, _>(
                {
                    let ov = Arc::clone(&order);
                    move |_, _| {
                        ov.lock().expect("o").push("victim".to_string());
                    }
                },
                ListenerOptions::default(),
            )
            .expect("on victim");
        *removed_handle.lock().expect("rh") = Some(h_victim);

        let o0 = Arc::clone(&order);
        ctx.on::<PingEvent, _>(
            move |_, ev| {
                o0.lock().expect("o").push(format!("first:{}", ev.0));
                if let Some(h) = rh_clone.lock().expect("rh").take() {
                    drop(h.dispose());
                }
            },
            ListenerOptions {
                prepend: true,
                global: false,
            },
        )
        .expect("on first");

        // Emit without filter first: snapshot order + deletion inside emit.
        ctx.emit(&PingEvent("a"), None);
        ctx.settle().await;
        assert_eq!(
            *order.lock().expect("o"),
            vec!["first:a", "second:a", "victim"]
        );

        // Victim is now removed on subsequent emits.
        ctx.emit(&PingEvent("b"), None);
        assert_eq!(
            *order.lock().expect("o"),
            vec!["first:a", "second:a", "victim", "first:b", "second:b"]
        );

        // Context filter + global bypass.
        let allowed = Arc::new(Mutex::new(Vec::new()));
        let sub_ctx = ctx.extend(BTreeMap::new());
        sub_ctx
            .on::<PingEvent, _>(
                {
                    let a = Arc::clone(&allowed);
                    move |_, _| a.lock().expect("a").push("filtered")
                },
                ListenerOptions::default(),
            )
            .expect("sub filtered");
        sub_ctx
            .on::<PingEvent, _>(
                {
                    let a = Arc::clone(&allowed);
                    move |_, _| a.lock().expect("a").push("global")
                },
                ListenerOptions {
                    prepend: false,
                    global: true,
                },
            )
            .expect("sub global");

        let filter: EventFilter = Arc::new(|_target_ctx| false);
        ctx.emit(&PingEvent("f"), Some(&filter));
        assert_eq!(*allowed.lock().expect("a"), vec!["global"]);

        // Once fires only once.
        let once_count = Arc::new(AtomicUsize::new(0));
        ctx.once::<PingEvent, _>(
            {
                let oc = Arc::clone(&once_count);
                move |_, _| {
                    oc.fetch_add(1, Ordering::AcqRel);
                }
            },
            ListenerOptions::default(),
        )
        .expect("once");
        ctx.emit(&PingEvent("1"), None);
        ctx.emit(&PingEvent("2"), None);
        assert_eq!(once_count.load(Ordering::Acquire), 1);

        // Parallel aggregates errors.
        ctx.on_async::<PingEvent, _>(
            |_, _| {
                Box::pin(async {
                    Err(FrameError::new(ErrorKind::ActivationFailed, "p-err-1"))
                })
            },
            ListenerOptions::default(),
        )
        .expect("p1");
        let p_err = ctx
            .parallel(&PingEvent("x"), None)
            .await
            .expect_err("parallel error");
        assert_eq!(p_err.kind, ErrorKind::Aggregate);

        // Serial short-circuits before third listener.
        let third_hit = Arc::new(AtomicBool::new(false));
        ctx.on_request::<MatchRequest, _>(
            |_, _| Box::pin(async { Ok(None) }),
            ListenerOptions::default(),
        )
        .expect("s1");
        ctx.on_request::<MatchRequest, _>(
            |_, req| {
                let code = req.0;
                Box::pin(async move {
                    if code == 7 { Ok(Some("hit")) } else { Ok(None) }
                })
            },
            ListenerOptions::default(),
        )
        .expect("s2");
        ctx.on_request::<MatchRequest, _>(
            {
                let hit = Arc::clone(&third_hit);
                move |_, _| {
                    hit.store(true, Ordering::Release);
                    Box::pin(async { Ok(Some("late")) })
                }
            },
            ListenerOptions::default(),
        )
        .expect("s3");
        let matched = ctx.serial(&MatchRequest(7), None).await.expect("serial");
        assert_eq!(matched, Some("hit"));
        assert!(!third_hit.load(Ordering::Acquire));

        // Waterfall onion composition: (1 + 10) * 2 = 22.
        ctx.on_waterfall::<PipeEvent, _>(
            |_, val, next: WaterfallNext<PipeEvent>| {
                Box::pin(async move {
                    let out = next.call(val + 10).await?;
                    Ok(out * 2)
                })
            },
            ListenerOptions::default(),
        )
        .expect("wf");
        let wf_out = ctx
            .waterfall::<PipeEvent>(
                1,
                None,
                Box::new(|v| Box::pin(async move { Ok(v) })),
            )
            .await
            .expect("wf result");
        assert_eq!(wf_out, 22);

        // Child fiber auto-removes its listeners on dispose.
        let child_hits = Arc::new(AtomicUsize::new(0));
        let ch = Arc::clone(&child_hits);
        let child = ctx
            .inject("listener-owner", Deps::empty(), move |c| {
                let ch = Arc::clone(&ch);
                Box::pin(async move {
                    c.on::<PingEvent, _>(
                        move |_, _| {
                            ch.fetch_add(1, Ordering::AcqRel);
                        },
                        ListenerOptions::default(),
                    )?;
                    Ok(())
                })
            })
            .expect("inject listener owner");
        child.wait().await.expect("child active");
        ctx.emit(&PingEvent("live"), None);
        assert_eq!(child_hits.load(Ordering::Acquire), 1);

        child.dispose().await.expect("dispose child");
        ctx.emit(&PingEvent("after"), None);
        assert_eq!(child_hits.load(Ordering::Acquire), 1);

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F08: Config validation, scoped update interception, and veto
// ---------------------------------------------------------------------------

struct PortPlugin {
    name: &'static str,
    effective: Arc<Mutex<i64>>,
    veto_port_9999: bool,
}

impl Plugin for PortPlugin {
    fn name(&self) -> &str {
        self.name
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        let port = match raw {
            ConfigValue::Map(m) => match m.get("port") {
                Some(ConfigValue::Int(p)) => *p,
                _ => -1,
            },
            _ => -1,
        };
        if port <= 0 {
            return Err(FrameError::validation(vec![ValidationIssue {
                path: "port".to_string(),
                message: "must be positive".to_string(),
            }]));
        }
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let effective = Arc::clone(&self.effective);
        let veto = self.veto_port_9999;
        Box::pin(async move {
            let port = match &config {
                ConfigValue::Map(m) => match m.get("port") {
                    Some(ConfigValue::Int(p)) => *p,
                    _ => 0,
                },
                _ => 0,
            };
            *effective.lock().expect("eff") = port;
            if veto {
                ctx.on_update(
                    |_, payload: UpdatePayload, next: UpdateNext| {
                        Box::pin(async move {
                            if let ConfigValue::Map(m) = &payload.config
                                && m.get("port")
                                    == Some(&ConfigValue::Int(9999))
                            {
                                return Err(FrameError::new(
                                    ErrorKind::ValidationFailed,
                                    "vetoed port 9999",
                                ));
                            }
                            next.call(payload).await
                        })
                    },
                    ListenerOptions::default(),
                )?;
            }
            Ok(())
        })
    }
}

#[test]
fn f08_config_validation_and_update_veto() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let eff_a = Arc::new(Mutex::new(0));
        let eff_b = Arc::new(Mutex::new(0));
        let fa = ctx
            .plugin(
                PortPlugin {
                    name: "plugin-a",
                    effective: Arc::clone(&eff_a),
                    veto_port_9999: true,
                },
                map_i64("port", 8080),
            )
            .expect("mount a");
        let fb = ctx
            .plugin(
                PortPlugin {
                    name: "plugin-b",
                    effective: Arc::clone(&eff_b),
                    veto_port_9999: false,
                },
                map_i64("port", 8081),
            )
            .expect("mount b");
        ctx.settle().await;
        assert_eq!(*eff_a.lock().expect("a"), 8080);

        // Invalid config rejected by validate_config; effective stays 8080.
        let bad_err = fa
            .update(map_i64("port", 0))
            .await
            .expect_err("port=0 invalid");
        assert_eq!(bad_err.kind, ErrorKind::ValidationFailed);
        assert_eq!(*eff_a.lock().expect("a"), 8080);

        // Vetoed by local hook on A; effective stays 8080.
        let veto_err = fa
            .update(map_i64("port", 9999))
            .await
            .expect_err("port=9999 vetoed");
        assert!(veto_err.message.contains("vetoed"));
        assert_eq!(*eff_a.lock().expect("a"), 8080);

        // Plugin B update is NOT intercepted by A's local hook.
        fb.update(map_i64("port", 9999)).await.expect("b updates");
        assert_eq!(*eff_b.lock().expect("b"), 9999);

        // Also test update while Pending takes effect on activation.
        let eff_c = Arc::new(Mutex::new(0));
        let eff_c_run = Arc::clone(&eff_c);
        let fc = ctx
            .inject(
                "pending-cfg",
                Deps::empty().with::<SeqService>(),
                move |c| {
                    let eff = Arc::clone(&eff_c_run);
                    Box::pin(async move {
                        let _ = c.require::<SeqService>()?;
                        *eff.lock().expect("c") += 1;
                        Ok(())
                    })
                },
            )
            .expect("mount c");
        ctx.settle().await;
        assert_eq!(fc.state(), FiberState::Pending);
        fc.update(map_i64("port", 7000))
            .await
            .expect("pending update");
        let _p = ctx.plugin(SeqProvider(1), ConfigValue::Null).expect("p");
        ctx.settle().await;
        assert_eq!(fc.state(), FiberState::Active);
        assert_eq!(*eff_c.lock().expect("c"), 1);

        // Verify UpdateConfig waterfall type is callable via on_waterfall too.
        let _ = std::any::type_name::<UpdateConfig>();

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F09: Declarative Loader preflight, patches, and nested children
// ---------------------------------------------------------------------------

struct RecordingPlugin {
    id: &'static str,
    loads: Arc<Mutex<Vec<(String, i64)>>>,
    unloads: Arc<Mutex<Vec<String>>>,
}

impl Plugin for RecordingPlugin {
    fn name(&self) -> &str {
        self.id
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let id = self.id.to_string();
        let loads = Arc::clone(&self.loads);
        let unloads = Arc::clone(&self.unloads);
        Box::pin(async move {
            let val = match &config {
                ConfigValue::Map(m) => match m.get("v") {
                    Some(ConfigValue::Int(n)) => *n,
                    _ => 0,
                },
                _ => 0,
            };
            loads.lock().expect("loads").push((id.clone(), val));
            ctx.effect("rec", move |scope| {
                Box::pin(async move {
                    scope.defer(move || {
                        unloads.lock().expect("unloads").push(id);
                    });
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f09_loader_preflight_patch_and_nested() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let loads = Arc::new(Mutex::new(Vec::new()));
        let unloads = Arc::new(Mutex::new(Vec::new()));
        let resolver = Arc::new(
            StaticResolver::new()
                .register(
                    "mod-a",
                    RecordingPlugin {
                        id: "a",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                )
                .register(
                    "mod-b",
                    RecordingPlugin {
                        id: "b",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                )
                .register(
                    "mod-c",
                    RecordingPlugin {
                        id: "c",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                ),
        );
        let loader = Loader::new(ctx.clone(), resolver);

        let active_ids = loader
            .apply(vec![
                PluginEntry {
                    id: "e1".to_string(),
                    target: EntryTarget::Name("mod-a".to_string()),
                    config: map_i64("v", 1),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
                PluginEntry {
                    id: "e2".to_string(),
                    target: EntryTarget::Name("mod-b".to_string()),
                    config: map_i64("v", 2),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
            ])
            .await
            .expect("initial apply");
        assert_eq!(active_ids, vec!["e1", "e2"]);

        // Preflight failure leaves live fibers untouched.
        let bad = loader
            .apply(vec![
                PluginEntry {
                    id: "e1".to_string(),
                    target: EntryTarget::Name("mod-a".to_string()),
                    config: map_i64("v", 99),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
                PluginEntry {
                    id: "e-bad".to_string(),
                    target: EntryTarget::Name("missing".to_string()),
                    config: ConfigValue::Null,
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
            ])
            .await
            .expect_err("preflight must reject");
        assert_eq!(bad.kind, ErrorKind::PluginNotFound);
        assert_eq!(loads.lock().expect("l").len(), 2);

        // Patches: insert e3, immediately patch e3 config, remove e2,
        // and update e1 config in-place.
        let patched_ids = loader
            .apply_patches(vec![
                EntryPatch::Insert {
                    anchor: Some(InsertAnchor::After("e1".to_string())),
                    entry: PluginEntry {
                        id: "e3".to_string(),
                        target: EntryTarget::Name("mod-c".to_string()),
                        config: map_i64("v", 3),
                        disabled: false,
                        isolate: Vec::new(),
                        children: Vec::new(),
                    },
                },
                EntryPatch::ReplaceConfig {
                    id: "e3".to_string(),
                    config: map_i64("v", 30),
                    disabled: None,
                },
                EntryPatch::Remove {
                    id: "e2".to_string(),
                },
                EntryPatch::ReplaceConfig {
                    id: "e1".to_string(),
                    config: map_i64("v", 10),
                    disabled: None,
                },
            ])
            .await
            .expect("apply patches");
        assert_eq!(patched_ids, vec!["e1", "e3"]);
        let snap = loader.inspect();
        assert_eq!(snap.len(), 2);
        assert!(snap.iter().all(|s| s.state == FiberState::Active));

        // Nested children: mount group, then close reverses child unload.
        unloads.lock().expect("u").clear();
        loader
            .apply(vec![PluginEntry {
                id: "group".to_string(),
                target: EntryTarget::Name("mod-a".to_string()),
                config: ConfigValue::Null,
                disabled: false,
                isolate: vec![ServiceKey::of::<SeqService>()],
                children: vec![
                    PluginEntry {
                        id: "c1".to_string(),
                        target: EntryTarget::Name("mod-b".to_string()),
                        config: map_i64("v", 1),
                        disabled: false,
                        isolate: Vec::new(),
                        children: Vec::new(),
                    },
                    PluginEntry {
                        id: "c2".to_string(),
                        target: EntryTarget::Name("mod-c".to_string()),
                        config: map_i64("v", 2),
                        disabled: false,
                        isolate: Vec::new(),
                        children: Vec::new(),
                    },
                ],
            }])
            .await
            .expect("nested group");
        loader.close().await.expect("close loader");
        let unloaded = unloads.lock().expect("u").clone();
        assert!(unloaded.windows(2).any(|w| w == ["c", "b"]));

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F10: Sanitized inspect, non-blocking settle, root reset, and final dispose
// ---------------------------------------------------------------------------

struct SecretFailPlugin;
impl Plugin for SecretFailPlugin {
    fn name(&self) -> &str {
        "secret-fail"
    }

    fn apply(
        &self,
        _ctx: Context,
        _config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        Box::pin(async move {
            let long_msg = "x".repeat(800);
            Err(FrameError::new(ErrorKind::ActivationFailed, long_msg))
        })
    }
}

#[test]
fn f10_inspect_settle_root_and_dispose() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let mut secret_cfg = BTreeMap::new();
        secret_cfg.insert(
            "secret_token".to_string(),
            ConfigValue::Str("tok_xyz_do_not_leak".to_string()),
        );
        let failed = ctx
            .plugin(SecretFailPlugin, ConfigValue::Map(secret_cfg))
            .expect("mount fail");
        let _ = failed.wait().await;

        let pending = ctx
            .inject("wait-missing", Deps::empty().with::<SeqService>(), |_| {
                Box::pin(async { Ok(()) })
            })
            .expect("mount pending");

        // settle() must return promptly even with Pending fibers.
        let start = Instant::now();
        ctx.settle().await;
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(pending.state(), FiberState::Pending);
        assert_eq!(failed.state(), FiberState::Failed);

        let snap = ctx.inspect();
        let failed_snap = snap
            .fibers
            .iter()
            .find(|f| f.id == failed.id())
            .expect("failed snap");
        let summary =
            failed_snap.error_summary.as_ref().expect("summary present");
        assert!(summary.len() <= 512);
        let dumped = format!("{snap:?}");
        assert!(!dumped.contains("tok_xyz_do_not_leak"));
        assert!(!dumped.contains("secret_token"));

        // root_fiber().dispose() clears mounted fibers while keeping root usable.
        ctx.root_fiber().dispose().await.expect("reset root");
        let after_reset = ctx.inspect();
        assert_eq!(after_reset.fibers.len(), 1);
        assert_eq!(after_reset.fibers[0].id, FiberId(0));

        let reused = ctx
            .plugin(SeqProvider(7), ConfigValue::Null)
            .expect("root still usable");
        reused.wait().await.expect("active after reset");
        assert!(ctx.get::<SeqService>().is_some());

        // Final ctx.dispose() shuts down permanently and rejects new mounts.
        ctx.dispose().await.expect("final dispose");
        assert_eq!(ctx.root_fiber().state(), FiberState::Disposed);
        let err = ctx
            .plugin(SeqProvider(8), ConfigValue::Null)
            .err()
            .expect("disposed root rejects plugins");
        assert_eq!(err.kind, ErrorKind::InactiveFiber);
    });
}

// ---------------------------------------------------------------------------
// F11: ScopedService caller-bound effect ownership
// ---------------------------------------------------------------------------

struct RegistryState {
    entries: Mutex<Vec<(&'static str, bool)>>,
}

#[derive(Clone)]
struct ScopedRegistrySvc {
    state: Arc<RegistryState>,
}

impl Service for ScopedRegistrySvc {
    const NAME: &'static str = "test.scoped_registry";
}

struct ScopedRegistryView {
    svc: Arc<ScopedRegistrySvc>,
    caller: Context,
}

impl ScopedRegistryView {
    fn register(
        &self,
        tag: &'static str,
    ) -> Result<frame::EffectHandle, FrameError> {
        let idx = {
            let mut entries = self.svc.state.entries.lock().expect("lock");
            entries.push((tag, true));
            entries.len() - 1
        };
        let state = Arc::clone(&self.svc.state);
        self.caller.effect_sync(format!("reg:{tag}"), move |scope| {
            scope.defer(move || {
                state.entries.lock().expect("lock")[idx].1 = false;
            });
            Ok(())
        })
    }

    fn active_tags(&self) -> Vec<&'static str> {
        self.svc
            .state
            .entries
            .lock()
            .expect("lock")
            .iter()
            .filter_map(|&(t, active)| active.then_some(t))
            .collect()
    }
}

impl ScopedService for ScopedRegistrySvc {
    type View = ScopedRegistryView;

    fn bind(self: Arc<Self>, caller: Context) -> Self::View {
        ScopedRegistryView { svc: self, caller }
    }
}

#[test]
fn f11_scoped_service() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let state = Arc::new(RegistryState {
            entries: Mutex::new(Vec::new()),
        });
        let reg_provider = ctx
            .inject("reg-provider", Deps::empty(), {
                let state = Arc::clone(&state);
                move |c| {
                    let state = Arc::clone(&state);
                    Box::pin(async move {
                        c.provide(Arc::new(ScopedRegistrySvc { state }))
                            .await?;
                        Ok(())
                    })
                }
            })
            .expect("mount provider");
        reg_provider.wait().await.expect("provider active");

        let consumer_a = ctx
            .inject(
                "consumer-a",
                Deps::empty().with::<ScopedRegistrySvc>(),
                |c| {
                    Box::pin(async move {
                        let view = c.use_service::<ScopedRegistrySvc>()?;
                        let _ = view.register("route-a")?;
                        Ok(())
                    })
                },
            )
            .expect("mount a");
        let consumer_b = ctx
            .inject(
                "consumer-b",
                Deps::empty().with::<ScopedRegistrySvc>(),
                |c| {
                    Box::pin(async move {
                        let view = c.use_service::<ScopedRegistrySvc>()?;
                        let _ = view.register("route-b")?;
                        Ok(())
                    })
                },
            )
            .expect("mount b");

        ctx.settle().await;
        let root_view =
            ctx.try_use_service::<ScopedRegistrySvc>().expect("view");
        assert_eq!(root_view.active_tags(), vec!["route-a", "route-b"]);

        let snap_before = ctx.inspect();
        let prov_snap = snap_before
            .fibers
            .iter()
            .find(|f| f.id == reg_provider.id())
            .expect("provider snap");
        assert_eq!(prov_snap.effect_count, 1);

        // Disposing consumer_a automatically revokes "route-a" without provider impact.
        consumer_a.dispose().await.expect("dispose a");
        assert_eq!(root_view.active_tags(), vec!["route-b"]);
        assert_eq!(reg_provider.state(), FiberState::Active);
        assert_eq!(consumer_b.state(), FiberState::Active);

        ctx.dispose().await.expect("dispose");
        assert!(root_view.active_tags().is_empty());
    });
}

// ---------------------------------------------------------------------------
// F12: Parent-child Fiber tree & same-scope inheritance
// ---------------------------------------------------------------------------

#[test]
fn f12_parent_child_tree_and_inheritance() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let prov = ctx
            .plugin(SeqProvider(77), ConfigValue::Null)
            .expect("provider");
        prov.wait().await.expect("provider active");

        let c1_seen = Arc::new(Mutex::new(None::<u64>));
        let c1_parent = Arc::new(Mutex::new(None::<FiberId>));
        let c2_missing = Arc::new(AtomicBool::new(false));
        let child_ids = Arc::new(Mutex::new(Vec::<FiberId>::new()));

        let c1_seen_w = Arc::clone(&c1_seen);
        let c1_parent_w = Arc::clone(&c1_parent);
        let c2_missing_w = Arc::clone(&c2_missing);
        let child_ids_w = Arc::clone(&child_ids);

        let parent = ctx
            .inject("parent", Deps::empty().with::<SeqService>(), move |p_ctx| {
                let c1_seen_w = Arc::clone(&c1_seen_w);
                let c1_parent_w = Arc::clone(&c1_parent_w);
                let c2_missing_w = Arc::clone(&c2_missing_w);
                let child_ids_w = Arc::clone(&child_ids_w);
                Box::pin(async move {
                    let c1 = p_ctx.inject("child-inherited", Deps::empty(), move |c_ctx| {
                        let c1_seen_w = Arc::clone(&c1_seen_w);
                        let c1_parent_w = Arc::clone(&c1_parent_w);
                        Box::pin(async move {
                            *c1_parent_w.lock().expect("lock") =
                                c_ctx.parent_fiber().map(|h| h.id());
                            let svc = c_ctx.require::<SeqService>()?;
                            *c1_seen_w.lock().expect("lock") = Some(svc.0);
                            Ok(())
                        })
                    })?;

                    let isolated = p_ctx.isolate::<SeqService>();
                    let c2 =
                        isolated.inject("child-isolated", Deps::empty(), move |c_ctx| {
                            let c2_missing_w = Arc::clone(&c2_missing_w);
                            Box::pin(async move {
                                let err = c_ctx
                                    .require::<SeqService>()
                                    .err()
                                    .expect("isolated child must not inherit across isolate");
                                c2_missing_w.store(
                                    err.kind == ErrorKind::MissingDependency,
                                    Ordering::Release,
                                );
                                Ok(())
                            })
                        })?;

                    c1.wait().await?;
                    c2.wait().await?;
                    *child_ids_w.lock().expect("lock") = vec![c1.id(), c2.id()];
                    Ok(())
                })
            })
            .expect("mount parent");

        ctx.settle().await;
        assert_eq!(parent.state(), FiberState::Active);
        assert_eq!(*c1_seen.lock().expect("lock"), Some(77));
        assert_eq!(*c1_parent.lock().expect("lock"), Some(parent.id()));
        assert!(c2_missing.load(Ordering::Acquire));

        let snap = ctx.inspect();
        let parent_snap = snap
            .fibers
            .iter()
            .find(|f| f.id == parent.id())
            .expect("parent snap");
        let expected_children = child_ids.lock().expect("lock").clone();
        assert_eq!(parent_snap.children, expected_children);
        assert_eq!(parent_snap.parent, Some(FiberId(0)));

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F13: Optional dependencies & unload snapshot preservation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OptMetrics(u32);
impl Service for OptMetrics {
    const NAME: &'static str = "test.opt_metrics";
}

#[test]
fn f13_optional_deps_and_unload_snapshot() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let req = ctx
            .plugin(SeqProvider(1), ConfigValue::Null)
            .expect("mount seq");
        req.wait().await.expect("seq active");

        let observed = Arc::new(Mutex::new(Vec::<Option<u32>>::new()));
        let cleanup_saw = Arc::new(Mutex::new(Vec::<Option<u32>>::new()));
        let observed_w = Arc::clone(&observed);
        let cleanup_w = Arc::clone(&cleanup_saw);

        let worker = ctx
            .inject(
                "opt-worker",
                Deps::empty()
                    .with::<SeqService>()
                    .with_optional::<OptMetrics>(),
                move |c| {
                    let observed_w = Arc::clone(&observed_w);
                    let cleanup_w = Arc::clone(&cleanup_w);
                    Box::pin(async move {
                        let _ = c.require::<SeqService>()?;
                        let current = c.get::<OptMetrics>().map(|m| m.0);
                        observed_w.lock().expect("lock").push(current);
                        let ctx_for_clean = c.clone();
                        c.effect("opt-cleanup", move |scope| {
                            Box::pin(async move {
                                scope.defer(move || {
                                    let held = ctx_for_clean
                                        .get::<OptMetrics>()
                                        .map(|m| m.0);
                                    cleanup_w.lock().expect("lock").push(held);
                                });
                                Ok(())
                            })
                        })
                        .await?;
                        Ok(())
                    })
                },
            )
            .expect("mount opt worker");

        ctx.settle().await;
        assert_eq!(worker.state(), FiberState::Active);
        assert_eq!(*observed.lock().expect("lock"), vec![None]);

        // Optional service appears -> worker cleanly reloads with Some(9).
        let opt_provider = ctx
            .inject("opt-provider", Deps::empty(), |c| {
                Box::pin(async move {
                    c.provide(Arc::new(OptMetrics(9))).await?;
                    Ok(())
                })
            })
            .expect("mount opt provider");
        ctx.settle().await;
        assert_eq!(worker.state(), FiberState::Active);
        assert_eq!(*observed.lock().expect("lock"), vec![None, Some(9)]);
        assert_eq!(*cleanup_saw.lock().expect("lock"), vec![None]);

        // Optional service disappears -> worker cleanup still sees Some(9) in snapshot,
        // then worker becomes Active again with None.
        opt_provider.dispose().await.expect("dispose opt provider");
        ctx.settle().await;
        assert_eq!(worker.state(), FiberState::Active);
        assert_eq!(*observed.lock().expect("lock"), vec![None, Some(9), None]);
        assert_eq!(*cleanup_saw.lock().expect("lock"), vec![None, Some(9)]);

        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F14: Context extend, select filtering, and scoped intercept deep merge
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ScopedCfgMarker;
impl Service for ScopedCfgMarker {
    const NAME: &'static str = "test.scoped_cfg_marker";
}

#[derive(Clone, Debug)]
struct ZonePing(&'static str);
impl Event for ZonePing {
    const NAME: &'static str = "test/zone_ping";
}

#[test]
fn f14_context_extend_select_and_intercept() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let mut base = BTreeMap::new();
        base.insert("timeout".to_string(), ConfigValue::Int(1000));
        let mut nested_base = BTreeMap::new();
        nested_base.insert("a".to_string(), ConfigValue::Int(1));
        nested_base.insert("b".to_string(), ConfigValue::Int(2));
        base.insert("nested".to_string(), ConfigValue::Map(nested_base));

        let mut p1 = BTreeMap::new();
        let mut nested_p1 = BTreeMap::new();
        nested_p1.insert("b".to_string(), ConfigValue::Int(20));
        p1.insert("nested".to_string(), ConfigValue::Map(nested_p1));

        let mut p2 = BTreeMap::new();
        p2.insert("timeout".to_string(), ConfigValue::Int(5000));

        let ctx_a = ctx
            .with_meta("zone", ConfigValue::Str("alpha".to_string()))
            .intercept::<ScopedCfgMarker>(ConfigValue::Map(p1))
            .intercept::<ScopedCfgMarker>(ConfigValue::Map(p2));

        let mut ext = BTreeMap::new();
        ext.insert("zone".to_string(), ConfigValue::Str("beta".to_string()));
        let ctx_b = ctx.extend(ext);

        let merged_a = ctx_a.resolve_intercept::<ScopedCfgMarker>(
            ConfigValue::Map(base.clone()),
        );
        let ConfigValue::Map(merged_map) = merged_a else {
            panic!("expected map");
        };
        assert_eq!(merged_map.get("timeout"), Some(&ConfigValue::Int(5000)));
        let Some(ConfigValue::Map(nested_res)) = merged_map.get("nested")
        else {
            panic!("expected nested map");
        };
        assert_eq!(nested_res.get("a"), Some(&ConfigValue::Int(1)));
        assert_eq!(nested_res.get("b"), Some(&ConfigValue::Int(20)));

        // Unintercepted sibling branch stays untouched.
        assert_eq!(
            ctx_b.resolve_intercept::<ScopedCfgMarker>(ConfigValue::Map(
                base.clone()
            )),
            ConfigValue::Map(base)
        );
        assert_eq!(
            ctx_a.meta("zone"),
            Some(&ConfigValue::Str("alpha".to_string()))
        );
        assert_eq!(
            ctx_b.meta("zone"),
            Some(&ConfigValue::Str("beta".to_string()))
        );

        // Chained select() predicates require AND match on emitter metadata.
        let alpha_received = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let alpha_w = Arc::clone(&alpha_received);
        let alpha_ctx = ctx
            .select(|em| {
                em.meta("zone") == Some(&ConfigValue::Str("alpha".to_string()))
            })
            .select(|em| em.meta("tier") == Some(&ConfigValue::Int(1)));
        let _h = alpha_ctx
            .on::<ZonePing, _>(
                move |_, ev| {
                    alpha_w.lock().expect("lock").push(ev.0);
                },
                ListenerOptions::default(),
            )
            .expect("register select listener");

        let ctx_a_tier1 = ctx_a.with_meta("tier", ConfigValue::Int(1));
        let ctx_a_tier2 = ctx_a.with_meta("tier", ConfigValue::Int(2));
        ctx_b.emit(&ZonePing("from-b"), None);
        ctx_a_tier2.emit(&ZonePing("from-a-t2"), None);
        ctx_a_tier1.emit(&ZonePing("from-a-t1"), None);

        assert_eq!(*alpha_received.lock().expect("lock"), vec!["from-a-t1"]);
        ctx.dispose().await.expect("dispose");
    });
}

// ---------------------------------------------------------------------------
// F15: Built-in lifecycle events emitted outside locks (reentrant-safe)
// ---------------------------------------------------------------------------

#[test]
fn f15_builtin_lifecycle_events_reentrant() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let states =
            Arc::new(
                Mutex::new(Vec::<(String, FiberState, FiberState)>::new()),
            );
        let services =
            Arc::new(Mutex::new(Vec::<(&'static str, ServiceAction)>::new()));
        let ready_count = Arc::new(AtomicUsize::new(0));

        let states_w = Arc::clone(&states);
        let _h1 = ctx
            .on::<FiberStateChanged, _>(
                move |listener_ctx, ev| {
                    // Reentrant inspect() inside listener must never deadlock.
                    let _ = listener_ctx.inspect();
                    states_w.lock().expect("lock").push((
                        ev.name.clone(),
                        ev.from,
                        ev.to,
                    ));
                },
                ListenerOptions::default(),
            )
            .expect("listen fiber state");

        let services_w = Arc::clone(&services);
        let _h2 = ctx
            .on::<ServiceChanged, _>(
                move |listener_ctx, ev| {
                    let _ = listener_ctx.get::<SeqService>();
                    services_w
                        .lock()
                        .expect("lock")
                        .push((ev.service, ev.action));
                },
                ListenerOptions::default(),
            )
            .expect("listen service");

        let ready_w = Arc::clone(&ready_count);
        let _h3 = ctx
            .on::<RuntimeReady, _>(
                move |_, _| {
                    ready_w.fetch_add(1, Ordering::AcqRel);
                },
                ListenerOptions::default(),
            )
            .expect("listen ready");

        let f = ctx
            .plugin(SeqProvider(10), ConfigValue::Null)
            .expect("mount provider");
        f.wait().await.expect("active");
        ctx.mark_ready();
        f.dispose().await.expect("dispose provider");

        ctx.dispose().await.expect("dispose");

        let logged_states = states.lock().expect("lock");
        assert!(logged_states.iter().any(|(n, from, to)| {
            n == "seq-provider"
                && *from == FiberState::Pending
                && *to == FiberState::Loading
        }));
        assert!(logged_states.iter().any(|(n, from, to)| {
            n == "seq-provider"
                && *from == FiberState::Loading
                && *to == FiberState::Active
        }));
        assert!(logged_states.iter().any(|(n, from, to)| {
            n == "seq-provider"
                && *from == FiberState::Active
                && *to == FiberState::Unloading
        }));

        let logged_svcs = services.lock().expect("lock");
        assert!(logged_svcs.iter().any(
            |&(s, a)| s == SeqService::NAME && a == ServiceAction::Provided
        ));
        assert!(logged_svcs.iter().any(
            |&(s, a)| s == SeqService::NAME && a == ServiceAction::Removed
        ));
        assert_eq!(ready_count.load(Ordering::Acquire), 1);
    });
}

// ---------------------------------------------------------------------------
// F16: Nested Group incremental diff & LoaderService bootstrap
// ---------------------------------------------------------------------------

type LoadLog = Arc<Mutex<Vec<(&'static str, FiberId, i64)>>>;
type UnloadLog = Arc<Mutex<Vec<(&'static str, FiberId)>>>;

struct CountedPlugin {
    label: &'static str,
    loads: LoadLog,
    unloads: UnloadLog,
}

impl Plugin for CountedPlugin {
    fn name(&self) -> &str {
        self.label
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> frame::BoxFuture<'static, Result<(), FrameError>> {
        let label = self.label;
        let loads = Arc::clone(&self.loads);
        let unloads = Arc::clone(&self.unloads);
        let fid = ctx.fiber().id();
        Box::pin(async move {
            let v = match config {
                ConfigValue::Map(m) => match m.get("v") {
                    Some(ConfigValue::Int(n)) => *n,
                    _ => 0,
                },
                _ => 0,
            };
            loads.lock().expect("lock").push((label, fid, v));
            ctx.effect("counted", move |scope| {
                Box::pin(async move {
                    scope.defer(move || {
                        unloads.lock().expect("lock").push((label, fid));
                    });
                    Ok(())
                })
            })
            .await?;
            Ok(())
        })
    }
}

#[test]
fn f16_nested_group_incremental_diff_and_service() {
    let (ctx, _errors, _spawner) = new_ctx();
    block_on(async {
        let loads = Arc::new(Mutex::new(Vec::new()));
        let unloads = Arc::new(Mutex::new(Vec::new()));
        let resolver: Arc<dyn frame::Resolver> = Arc::new(
            StaticResolver::new()
                .register(
                    "a",
                    CountedPlugin {
                        label: "a",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                )
                .register(
                    "b",
                    CountedPlugin {
                        label: "b",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                )
                .register(
                    "c",
                    CountedPlugin {
                        label: "c",
                        loads: Arc::clone(&loads),
                        unloads: Arc::clone(&unloads),
                    },
                ),
        );

        let loader_fiber = ctx
            .plugin(LoaderPlugin::new(Arc::clone(&resolver)), ConfigValue::Null)
            .expect("mount loader plugin");
        loader_fiber.wait().await.expect("loader plugin active");

        let loader_svc = ctx.get::<LoaderService>().expect("loader service");

        let mk_group = |b_val: i64, include_c: bool| {
            let mut children = vec![
                PluginEntry {
                    id: "ca".to_string(),
                    target: EntryTarget::Name("a".to_string()),
                    config: map_i64("v", 1),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
                PluginEntry {
                    id: "cb".to_string(),
                    target: EntryTarget::Name("b".to_string()),
                    config: map_i64("v", b_val),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
            ];
            if include_c {
                children.push(PluginEntry {
                    id: "cc".to_string(),
                    target: EntryTarget::Name("c".to_string()),
                    config: map_i64("v", 3),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                });
            }
            vec![PluginEntry {
                id: "web".to_string(),
                target: EntryTarget::Group,
                config: ConfigValue::Null,
                disabled: false,
                isolate: vec![ServiceKey::of::<SeqService>()],
                children,
            }]
        };

        loader_svc
            .apply(mk_group(2, false))
            .await
            .expect("initial apply");
        let group_fid_1 = ctx
            .inspect()
            .fibers
            .iter()
            .find(|f| f.name == "web")
            .expect("web group fiber")
            .id;
        assert_eq!(loads.lock().expect("lock").len(), 2);

        // Incremental nested update: change cb config and insert cc.
        // Neither parent "web" nor sibling "a" may unload/reload!
        loader_svc
            .apply(mk_group(20, true))
            .await
            .expect("incremental nested update");
        let group_fid_2 = ctx
            .inspect()
            .fibers
            .iter()
            .find(|f| f.name == "web")
            .expect("web group fiber kept")
            .id;
        assert_eq!(group_fid_1, group_fid_2);

        let a_loads: Vec<_> = loads
            .lock()
            .expect("lock")
            .iter()
            .filter(|(l, _, _)| *l == "a")
            .cloned()
            .collect();
        let b_loads: Vec<_> = loads
            .lock()
            .expect("lock")
            .iter()
            .filter(|(l, _, _)| *l == "b")
            .cloned()
            .collect();
        let c_loads: Vec<_> = loads
            .lock()
            .expect("lock")
            .iter()
            .filter(|(l, _, _)| *l == "c")
            .cloned()
            .collect();
        assert_eq!(a_loads.len(), 1);
        assert_eq!(b_loads.len(), 2);
        assert_eq!(b_loads[0].1, b_loads[1].1);
        assert_eq!(b_loads[1].2, 20);
        assert_eq!(c_loads.len(), 1);

        // LoaderPlugin disposal cleanly tears down managed entries.
        ctx.dispose().await.expect("dispose");
        assert!(unloads.lock().expect("lock").iter().any(|(l, _)| *l == "a"));
    });
}

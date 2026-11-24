use crate::error::FrameError;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type ErrorCallback = Arc<dyn Fn(&FrameError) + Send + Sync>;

/// Host async task spawner adapter.
pub trait Spawner: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>);
}

#[derive(Clone)]
pub struct FrameOptions {
    pub spawner: Arc<dyn Spawner>,
    pub on_error: Option<ErrorCallback>,
}

impl FrameOptions {
    pub fn new(spawner: Arc<dyn Spawner>) -> Self {
        Self {
            spawner,
            on_error: None,
        }
    }

    pub fn with_on_error(mut self, on_error: ErrorCallback) -> Self {
        self.on_error = Some(on_error);
        self
    }
}

struct SignalState {
    done: bool,
    wakers: Vec<Waker>,
}

#[derive(Clone)]
pub(crate) struct Signal(Arc<Mutex<SignalState>>);

impl Signal {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(SignalState {
            done: false,
            wakers: Vec::new(),
        })))
    }

    pub(crate) fn set(&self) {
        let wakers = {
            let mut st = self.0.lock().expect("signal lock");
            if st.done {
                return;
            }
            st.done = true;
            std::mem::take(&mut st.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(crate) fn is_set(&self) -> bool {
        self.0.lock().expect("signal lock").done
    }

    pub(crate) fn wait(&self) -> BoxFuture<'static, ()> {
        struct WaitSignal(Arc<Mutex<SignalState>>);
        impl Future for WaitSignal {
            type Output = ();

            fn poll(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                let mut st = self.0.lock().expect("signal lock");
                if st.done {
                    Poll::Ready(())
                } else {
                    if !st.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                        st.wakers.push(cx.waker().clone());
                    }
                    Poll::Pending
                }
            }
        }
        Box::pin(WaitSignal(Arc::clone(&self.0)))
    }
}

struct SignalValueState<T> {
    value: Option<T>,
    wakers: Vec<Waker>,
}

pub(crate) struct SignalValue<T>(Arc<Mutex<SignalValueState<T>>>);

impl<T> Clone for SignalValue<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone + Send + Sync + 'static> SignalValue<T> {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(SignalValueState {
            value: None,
            wakers: Vec::new(),
        })))
    }

    pub(crate) fn set(&self, val: T) {
        let wakers = {
            let mut st = self.0.lock().expect("signal value lock");
            if st.value.is_some() {
                return;
            }
            st.value = Some(val);
            std::mem::take(&mut st.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(crate) fn wait(&self) -> BoxFuture<'static, T> {
        struct WaitValue<T>(Arc<Mutex<SignalValueState<T>>>);
        impl<T: Clone> Future for WaitValue<T> {
            type Output = T;

            fn poll(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                let mut st = self.0.lock().expect("signal value lock");
                if let Some(val) = &st.value {
                    Poll::Ready(val.clone())
                } else {
                    if !st.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                        st.wakers.push(cx.waker().clone());
                    }
                    Poll::Pending
                }
            }
        }
        Box::pin(WaitValue(Arc::clone(&self.0)))
    }
}

struct NotifyState {
    seq: u64,
    wakers: Vec<Waker>,
}

#[derive(Clone)]
pub(crate) struct Notify(Arc<Mutex<NotifyState>>);

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

impl Notify {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(NotifyState {
            seq: 0,
            wakers: Vec::new(),
        })))
    }

    pub(crate) fn seq(&self) -> u64 {
        self.0.lock().expect("notify lock").seq
    }

    pub(crate) fn notify_all(&self) {
        let wakers = {
            let mut st = self.0.lock().expect("notify lock");
            st.seq = st.seq.wrapping_add(1);
            std::mem::take(&mut st.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(crate) fn wait_change(&self, observed: u64) -> BoxFuture<'static, ()> {
        struct WaitNotify {
            inner: Arc<Mutex<NotifyState>>,
            observed: u64,
        }
        impl Future for WaitNotify {
            type Output = ();

            fn poll(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                let mut st = self.inner.lock().expect("notify lock");
                if st.seq != self.observed {
                    Poll::Ready(())
                } else {
                    if !st.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                        st.wakers.push(cx.waker().clone());
                    }
                    Poll::Pending
                }
            }
        }
        Box::pin(WaitNotify {
            inner: Arc::clone(&self.0),
            observed,
        })
    }
}

struct SerialState {
    locked: bool,
    waiters: VecDeque<Signal>,
}

#[derive(Clone)]
pub(crate) struct SerialQueue(Arc<Mutex<SerialState>>);

pub(crate) struct SerialGuard {
    state: Arc<Mutex<SerialState>>,
}

impl Default for SerialQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialQueue {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(SerialState {
            locked: false,
            waiters: VecDeque::new(),
        })))
    }

    pub(crate) fn acquire(&self) -> BoxFuture<'static, SerialGuard> {
        let state = Arc::clone(&self.0);
        Box::pin(async move {
            let sig = {
                let mut st = state.lock().expect("serial lock");
                if !st.locked {
                    st.locked = true;
                    return SerialGuard {
                        state: Arc::clone(&state),
                    };
                }
                let sig = Signal::new();
                st.waiters.push_back(sig.clone());
                sig
            };

            struct CancelGuard {
                state: Arc<Mutex<SerialState>>,
                sig: Signal,
                acquired: bool,
            }
            impl Drop for CancelGuard {
                fn drop(&mut self) {
                    if !self.acquired && self.sig.is_set() {
                        release_next(&self.state);
                    }
                }
            }

            let mut cancel = CancelGuard {
                state: Arc::clone(&state),
                sig: sig.clone(),
                acquired: false,
            };
            sig.wait().await;
            cancel.acquired = true;
            SerialGuard { state }
        })
    }
}

fn release_next(state: &Arc<Mutex<SerialState>>) {
    let next = {
        let mut st = state.lock().expect("serial lock");
        if let Some(sig) = st.waiters.pop_front() {
            Some(sig)
        } else {
            st.locked = false;
            None
        }
    };
    if let Some(sig) = next {
        sig.set();
    }
}

impl Drop for SerialGuard {
    fn drop(&mut self) {
        release_next(&self.state);
    }
}

pub(crate) fn join_all<T: Send + Unpin + 'static>(
    futs: Vec<BoxFuture<'static, T>>,
) -> BoxFuture<'static, Vec<T>> {
    struct JoinAll<T> {
        futs: Vec<Option<BoxFuture<'static, T>>>,
        results: Vec<Option<T>>,
    }

    impl<T: Unpin> Future for JoinAll<T> {
        type Output = Vec<T>;

        fn poll(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Self::Output> {
            let count = self.futs.len();
            for i in 0..count {
                if let Some(mut fut) = self.futs[i].take() {
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(val) => {
                            self.results[i] = Some(val);
                        }
                        Poll::Pending => {
                            self.futs[i] = Some(fut);
                        }
                    }
                }
            }
            if self.results.iter().all(Option::is_some) {
                let out = self
                    .results
                    .iter_mut()
                    .map(|slot| slot.take().expect("completed result"))
                    .collect();
                Poll::Ready(out)
            } else {
                Poll::Pending
            }
        }
    }

    let count = futs.len();
    let mut results = Vec::with_capacity(count);
    results.resize_with(count, || None);
    Box::pin(JoinAll {
        futs: futs.into_iter().map(Some).collect(),
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_all_empty_completes() {
        let mut fut = join_all::<usize>(Vec::new());
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            matches!(fut.as_mut().poll(&mut cx), Poll::Ready(v) if v.is_empty())
        );
    }

    #[test]
    fn serial_queue_preserves_fifo_order() {
        let queue = SerialQueue::new();
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        let mut f1 = queue.acquire();
        let g1 = match f1.as_mut().poll(&mut cx) {
            Poll::Ready(g) => g,
            Poll::Pending => panic!("first acquire should succeed"),
        };

        let mut f2 = queue.acquire();
        assert!(matches!(f2.as_mut().poll(&mut cx), Poll::Pending));
        let mut f3 = queue.acquire();
        assert!(matches!(f3.as_mut().poll(&mut cx), Poll::Pending));

        drop(g1);
        let g2 = match f2.as_mut().poll(&mut cx) {
            Poll::Ready(g) => g,
            Poll::Pending => panic!("second waiter should wake next"),
        };
        assert!(matches!(f3.as_mut().poll(&mut cx), Poll::Pending));

        drop(g2);
        assert!(matches!(f3.as_mut().poll(&mut cx), Poll::Ready(_)));
    }
}

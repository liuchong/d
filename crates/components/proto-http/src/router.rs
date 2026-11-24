//! Control-plane routing table compiled into lock-free read snapshots.

use frame::{BoxFuture, Context, EffectHandle, FrameError};
use http_api::{
    FilterOutcome, HttpFilter, HttpHandler, HttpRequest, HttpResponse,
    RouteMatch, RouteSpec, Router, StatusCode,
};
use std::cmp::Reverse;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone)]
struct RouteEntry {
    seq: u64,
    spec: RouteSpec,
    handler: Arc<dyn HttpHandler>,
}

#[derive(Clone)]
struct FilterEntry {
    seq: u64,
    priority: i32,
    filter: Arc<dyn HttpFilter>,
}

#[derive(Default)]
struct RoutingSnapshot {
    routes: Vec<RouteEntry>,
    filters: Vec<FilterEntry>,
}

#[derive(Default)]
struct ControlState {
    routes: Vec<RouteEntry>,
    filters: Vec<FilterEntry>,
}

struct RouterInner {
    next_seq: AtomicU64,
    control: Mutex<ControlState>,
    snapshot: RwLock<Arc<RoutingSnapshot>>,
}

impl RouterInner {
    fn rebuild_snapshot(&self, guard: &ControlState) {
        let mut routes = guard.routes.clone();
        routes.sort_by_key(|r| {
            let host_rank = u8::from(r.spec.host.is_some());
            let (kind_rank, len) = match &r.spec.matcher {
                RouteMatch::Exact(s) => (3u8, s.len()),
                RouteMatch::Prefix(s) => (2u8, s.len()),
                RouteMatch::CatchAll => (1u8, 0),
            };
            (
                Reverse(r.spec.priority),
                Reverse(host_rank),
                Reverse(kind_rank),
                Reverse(len),
                r.seq,
            )
        });

        let mut filters = guard.filters.clone();
        filters.sort_by_key(|f| (f.priority, f.seq));

        let next = Arc::new(RoutingSnapshot { routes, filters });
        let mut snap = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
        *snap = next;
    }

    fn take_snapshot(&self) -> Arc<RoutingSnapshot> {
        Arc::clone(&*self.snapshot.read().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Layer 2 HTTP router: control plane mutates via [`EffectHandle`]s, data
/// plane reads immutable snapshots without holding any long lock.
pub struct HttpRouter {
    inner: Arc<RouterInner>,
}

impl HttpRouter {
    /// Construct a stateless snapshot router whose route/filter effects belong to callers.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(RouterInner {
                next_seq: AtomicU64::new(1),
                control: Mutex::new(ControlState::default()),
                snapshot: RwLock::new(Arc::new(RoutingSnapshot::default())),
            }),
        })
    }
}

fn route_matches(entry: &RouteEntry, req: &HttpRequest) -> bool {
    if let Some(expected_host) = &entry.spec.host {
        let Some(actual) = req.headers.get("host") else {
            return false;
        };
        let normalized = if expected_host.contains(':') {
            actual
        } else {
            actual.split(':').next().unwrap_or(actual)
        };
        if !normalized.eq_ignore_ascii_case(expected_host) {
            return false;
        }
    }

    if let Some(methods) = &entry.spec.methods
        && !methods.iter().any(|m| m == &req.method)
    {
        return false;
    }

    match &entry.spec.matcher {
        RouteMatch::Exact(path) => &req.path == path,
        RouteMatch::Prefix(prefix) => req.path.starts_with(prefix),
        RouteMatch::CatchAll => true,
    }
}

impl Router for HttpRouter {
    fn mount_route(
        &self,
        caller: &Context,
        spec: RouteSpec,
        handler: Arc<dyn HttpHandler>,
    ) -> Result<EffectHandle, FrameError> {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        let label = format!("http-route:{}", spec.id);
        let inner_add = Arc::clone(&self.inner);
        let inner_del = Arc::clone(&self.inner);
        caller.effect_sync(label, move |scope| {
            {
                let mut guard =
                    inner_add.control.lock().unwrap_or_else(|e| e.into_inner());
                guard.routes.push(RouteEntry { seq, spec, handler });
                inner_add.rebuild_snapshot(&guard);
            }
            scope.defer(move || {
                let mut guard =
                    inner_del.control.lock().unwrap_or_else(|e| e.into_inner());
                guard.routes.retain(|r| r.seq != seq);
                inner_del.rebuild_snapshot(&guard);
            });
            Ok(())
        })
    }

    fn mount_filter(
        &self,
        caller: &Context,
        filter: Arc<dyn HttpFilter>,
    ) -> Result<EffectHandle, FrameError> {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        let label = format!("http-filter:{}", filter.name());
        let priority = filter.priority();
        let inner_add = Arc::clone(&self.inner);
        let inner_del = Arc::clone(&self.inner);
        caller.effect_sync(label, move |scope| {
            {
                let mut guard =
                    inner_add.control.lock().unwrap_or_else(|e| e.into_inner());
                guard.filters.push(FilterEntry {
                    seq,
                    priority,
                    filter,
                });
                inner_add.rebuild_snapshot(&guard);
            }
            scope.defer(move || {
                let mut guard =
                    inner_del.control.lock().unwrap_or_else(|e| e.into_inner());
                guard.filters.retain(|f| f.seq != seq);
                inner_del.rebuild_snapshot(&guard);
            });
            Ok(())
        })
    }

    fn dispatch(&self, req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let snap = self.inner.take_snapshot();
        Box::pin(async move {
            let meta = req.request_meta();
            let mut cur_req = Some(req);
            let mut short_circuit = None;

            for entry in &snap.filters {
                let req_in = cur_req.take().expect("request must exist");
                match entry.filter.on_request(req_in).await {
                    FilterOutcome::Continue(next) => cur_req = Some(next),
                    FilterOutcome::Respond(resp) => {
                        short_circuit = Some(resp);
                        break;
                    }
                }
            }

            let mut resp = if let Some(resp) = short_circuit {
                resp
            } else {
                let req_in = cur_req.expect("request must exist");
                if let Some(route) =
                    snap.routes.iter().find(|r| route_matches(r, &req_in))
                {
                    route.handler.call(req_in).await
                } else {
                    HttpResponse::text(StatusCode::NOT_FOUND, "Not Found")
                }
            };

            for entry in snap.filters.iter().rev() {
                resp = entry.filter.on_response(&meta, resp).await;
            }
            resp
        })
    }
}

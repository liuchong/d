//! Zero-dependency Layer 3 reverse proxy, load-balancing, and health-check contracts.

use frame::Service;
use std::net::SocketAddr;
use std::sync::Arc;

/// Upstream backend selection strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadBalanceStrategy {
    RoundRobin,
    LeastConn,
    WeightedRoundRobin,
}

impl LoadBalanceStrategy {
    /// Parse canonical snake-case strategy name.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "round_robin" | "rr" => Some(Self::RoundRobin),
            "least_conn" | "lc" => Some(Self::LeastConn),
            "weighted_round_robin" | "wrr" => Some(Self::WeightedRoundRobin),
            _ => None,
        }
    }

    /// Return the canonical strategy identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::LeastConn => "least_conn",
            Self::WeightedRoundRobin => "weighted_round_robin",
        }
    }
}

/// Static configuration for one backend endpoint in an upstream pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendConfig {
    pub addr: SocketAddr,
    pub weight: u32,
    pub tls: bool,
    pub sni: Option<String>,
    pub ca_cert_pem: Option<String>,
    pub insecure_skip_verify: bool,
}

impl BackendConfig {
    /// Construct a plain TCP/HTTP backend with weight 1.
    pub fn plain(addr: SocketAddr) -> Self {
        Self {
            addr,
            weight: 1,
            tls: false,
            sni: None,
            ca_cert_pem: None,
            insecure_skip_verify: false,
        }
    }
}

/// Active background probe configuration for an upstream pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheckConfig {
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub path: Option<String>,
    pub unhealthy_threshold: u32,
    pub healthy_threshold: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval_ms: 2000,
            timeout_ms: 1000,
            path: None,
            unhealthy_threshold: 2,
            healthy_threshold: 1,
        }
    }
}

/// Declarative definition of an upstream target pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamConfig {
    pub id: String,
    pub strategy: LoadBalanceStrategy,
    pub backends: Vec<BackendConfig>,
    pub max_idle_per_backend: usize,
    pub connect_timeout_ms: u64,
    pub response_header_timeout_ms: u64,
    pub active_health_check: Option<HealthCheckConfig>,
    pub passive_eject_threshold: u32,
    pub passive_eject_ms: u64,
}

impl UpstreamConfig {
    /// Construct an upstream pool with sensible production defaults.
    pub fn new(id: impl Into<String>, backends: Vec<BackendConfig>) -> Self {
        Self {
            id: id.into(),
            strategy: LoadBalanceStrategy::RoundRobin,
            backends,
            max_idle_per_backend: 16,
            connect_timeout_ms: 3000,
            response_header_timeout_ms: 15000,
            active_health_check: None,
            passive_eject_threshold: 3,
            passive_eject_ms: 5000,
        }
    }
}

/// Declarative L7 HTTP/SSE/WebSocket reverse proxy route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProxyRouteConfig {
    pub id: String,
    pub host: Option<String>,
    pub match_prefix: String,
    pub strip_prefix: Option<String>,
    pub preserve_host: bool,
    pub upstream: String,
    pub websocket: bool,
    pub retry_idempotent: bool,
    pub extra_headers: Vec<(String, String)>,
}

/// Declarative L4 TCP reverse proxy listener mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpProxyRouteConfig {
    pub id: String,
    pub listen_addr: SocketAddr,
    pub tls_listener_id: Option<String>,
    pub max_connections: Option<usize>,
    pub upstream: String,
}

/// Live health status of one upstream backend instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendHealthSnapshot {
    pub addr: SocketAddr,
    pub healthy: bool,
    pub active_conns: usize,
    pub consecutive_failures: u32,
}

/// Live snapshot of one upstream pool's backend health and active load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamHealthSnapshot {
    pub id: String,
    pub strategy: LoadBalanceStrategy,
    pub backends: Vec<BackendHealthSnapshot>,
}

/// Read-only inspection contract exposed by [`ext-proxy`] for readiness probes
/// and observability metrics.
pub trait UpstreamPool: Send + Sync + 'static {
    /// Snapshot every configured upstream and backend health state.
    fn snapshot(&self) -> Vec<UpstreamHealthSnapshot>;

    /// Return `true` when every configured upstream has at least one healthy backend.
    fn all_upstreams_have_healthy_backend(&self) -> bool {
        let snaps = self.snapshot();
        snaps.iter().all(|u| {
            u.backends.is_empty() || u.backends.iter().any(|b| b.healthy)
        })
    }
}

/// Frame service exposing active upstream pool health snapshots.
#[derive(Clone)]
pub struct UpstreamPoolService(pub Arc<dyn UpstreamPool>);

impl UpstreamPoolService {
    /// Wrap an [`UpstreamPool`] implementation as a registered service.
    pub fn new(inner: Arc<dyn UpstreamPool>) -> Self {
        Self(inner)
    }

    /// Delegate [`UpstreamPool::snapshot`].
    pub fn snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        self.0.snapshot()
    }

    /// Delegate [`UpstreamPool::all_upstreams_have_healthy_backend`].
    pub fn all_upstreams_have_healthy_backend(&self) -> bool {
        self.0.all_upstreams_have_healthy_backend()
    }
}

impl Service for UpstreamPoolService {
    const NAME: &'static str = "proxy.upstreams";
}

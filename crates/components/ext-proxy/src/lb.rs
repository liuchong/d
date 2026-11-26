//! Upstream pool runtime state, load-balancing strategies, and passive ejection.

use crate::pool::IdleBucket;
use proxy_api::{
    BackendConfig, BackendHealthSnapshot, LoadBalanceStrategy, UpstreamConfig,
    UpstreamHealthSnapshot,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) struct BackendState {
    pub(crate) cfg: BackendConfig,
    pub(crate) healthy: AtomicBool,
    pub(crate) active_conns: AtomicUsize,
    pub(crate) consecutive_failures: AtomicU32,
    pub(crate) consecutive_successes: AtomicU32,
    pub(crate) ejected_until: Mutex<Option<Instant>>,
    pub(crate) idle: IdleBucket,
}

impl BackendState {
    fn new(cfg: BackendConfig) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            healthy: AtomicBool::new(true),
            active_conns: AtomicUsize::new(0),
            consecutive_failures: AtomicU32::new(0),
            consecutive_successes: AtomicU32::new(0),
            ejected_until: Mutex::new(None),
            idle: IdleBucket::default(),
        })
    }

    pub(crate) fn is_eligible(&self, now: Instant) -> bool {
        if self.healthy.load(Ordering::Acquire) {
            return true;
        }
        let mut ejected =
            self.ejected_until.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(deadline) = *ejected
            && now >= deadline
        {
            *ejected = None;
            self.healthy.store(true, Ordering::Release);
            self.consecutive_failures.store(0, Ordering::Release);
            return true;
        }
        false
    }

    pub(crate) fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Release);
        *self.ejected_until.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.healthy.store(true, Ordering::Release);
    }

    pub(crate) fn record_passive_failure(&self, threshold: u32, eject_ms: u64) {
        let fails =
            self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        self.consecutive_successes.store(0, Ordering::Release);
        if threshold > 0 && fails >= threshold {
            let until = Instant::now() + Duration::from_millis(eject_ms.max(1));
            *self.ejected_until.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(until);
            self.healthy.store(false, Ordering::Release);
        }
    }

    pub(crate) fn snapshot(&self) -> BackendHealthSnapshot {
        BackendHealthSnapshot {
            addr: self.cfg.addr,
            healthy: self.healthy.load(Ordering::Acquire),
            active_conns: self.active_conns.load(Ordering::Acquire),
            consecutive_failures: self
                .consecutive_failures
                .load(Ordering::Acquire),
        }
    }
}

/// Decrements active connection count automatically when dropped.
pub(crate) struct ActiveConnGuard {
    backend: Arc<BackendState>,
}

impl ActiveConnGuard {
    pub(crate) fn new(backend: Arc<BackendState>) -> Self {
        backend.active_conns.fetch_add(1, Ordering::AcqRel);
        Self { backend }
    }
}

impl Drop for ActiveConnGuard {
    fn drop(&mut self) {
        self.backend.active_conns.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct UpstreamState {
    pub(crate) cfg: UpstreamConfig,
    pub(crate) backends: Vec<Arc<BackendState>>,
    rr_cursor: AtomicUsize,
    wrr_schedule: Vec<usize>,
    wrr_cursor: AtomicUsize,
}

impl UpstreamState {
    pub(crate) fn new(cfg: UpstreamConfig) -> Arc<Self> {
        let backends: Vec<Arc<BackendState>> = cfg
            .backends
            .iter()
            .cloned()
            .map(BackendState::new)
            .collect();
        let wrr_schedule = build_wrr_schedule(&cfg.backends);
        Arc::new(Self {
            cfg,
            backends,
            rr_cursor: AtomicUsize::new(0),
            wrr_schedule,
            wrr_cursor: AtomicUsize::new(0),
        })
    }

    pub(crate) fn select_backend(
        &self,
        skip_idx: Option<usize>,
    ) -> Option<(usize, Arc<BackendState>, ActiveConnGuard)> {
        let len = self.backends.len();
        if len == 0 {
            return None;
        }
        let now = Instant::now();
        let chosen_idx = match self.cfg.strategy {
            LoadBalanceStrategy::RoundRobin => {
                let start = self.rr_cursor.fetch_add(1, Ordering::Relaxed);
                (0..len).map(|off| (start + off) % len).find(|&idx| {
                    skip_idx != Some(idx) && self.backends[idx].is_eligible(now)
                })?
            }
            LoadBalanceStrategy::LeastConn => (0..len)
                .filter(|&idx| {
                    skip_idx != Some(idx) && self.backends[idx].is_eligible(now)
                })
                .min_by_key(|&idx| {
                    (
                        self.backends[idx].active_conns.load(Ordering::Acquire),
                        idx,
                    )
                })?,
            LoadBalanceStrategy::WeightedRoundRobin => {
                if self.wrr_schedule.is_empty() {
                    return None;
                }
                let slen = self.wrr_schedule.len();
                let start = self.wrr_cursor.fetch_add(1, Ordering::Relaxed);
                (0..slen)
                    .map(|off| self.wrr_schedule[(start + off) % slen])
                    .find(|&idx| {
                        skip_idx != Some(idx)
                            && self.backends[idx].is_eligible(now)
                    })?
            }
        };
        let b = Arc::clone(&self.backends[chosen_idx]);
        let guard = ActiveConnGuard::new(Arc::clone(&b));
        Some((chosen_idx, b, guard))
    }

    pub(crate) fn snapshot(&self) -> UpstreamHealthSnapshot {
        UpstreamHealthSnapshot {
            id: self.cfg.id.clone(),
            strategy: self.cfg.strategy,
            backends: self.backends.iter().map(|b| b.snapshot()).collect(),
        }
    }
}

fn build_wrr_schedule(backends: &[BackendConfig]) -> Vec<usize> {
    let mut out = Vec::new();
    for (idx, b) in backends.iter().enumerate() {
        let w = b.weight.clamp(1, 64) as usize;
        for _ in 0..w {
            out.push(idx);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[test]
    fn round_robin_and_weighted_and_least_conn_select_accurately() {
        let b1 = BackendConfig::plain(addr(9001));
        let mut b2 = BackendConfig::plain(addr(9002));
        b2.weight = 3;

        let rr = UpstreamState::new(UpstreamConfig {
            strategy: LoadBalanceStrategy::RoundRobin,
            ..UpstreamConfig::new("rr", vec![b1.clone(), b2.clone()])
        });
        let picks: Vec<usize> =
            (0..4).map(|_| rr.select_backend(None).unwrap().0).collect();
        assert_eq!(picks, vec![0, 1, 0, 1]);

        let wrr = UpstreamState::new(UpstreamConfig {
            strategy: LoadBalanceStrategy::WeightedRoundRobin,
            ..UpstreamConfig::new("wrr", vec![b1.clone(), b2.clone()])
        });
        let wpicks: Vec<usize> = (0..4)
            .map(|_| wrr.select_backend(None).unwrap().0)
            .collect();
        assert_eq!(wpicks, vec![0, 1, 1, 1]);

        let lc = UpstreamState::new(UpstreamConfig {
            strategy: LoadBalanceStrategy::LeastConn,
            ..UpstreamConfig::new("lc", vec![b1, b2])
        });
        let (_i0, _b0, _hold0) = lc.select_backend(None).unwrap();
        let (i1, _b1, _hold1) = lc.select_backend(None).unwrap();
        assert_eq!(i1, 1, "LeastConn must pick idle backend 1 over busy 0");
    }

    #[test]
    fn passive_ejection_excludes_until_cooldown_expires() {
        let cfg = UpstreamConfig {
            passive_eject_threshold: 2,
            passive_eject_ms: 40,
            ..UpstreamConfig::new(
                "p",
                vec![
                    BackendConfig::plain(addr(9011)),
                    BackendConfig::plain(addr(9012)),
                ],
            )
        };
        let up = UpstreamState::new(cfg);
        up.backends[0].record_passive_failure(2, 40);
        assert!(up.backends[0].healthy.load(Ordering::Acquire));
        up.backends[0].record_passive_failure(2, 40);
        assert!(!up.backends[0].healthy.load(Ordering::Acquire));

        for _ in 0..4 {
            assert_eq!(up.select_backend(None).unwrap().0, 1);
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(up.backends[0].is_eligible(Instant::now()));
    }
}

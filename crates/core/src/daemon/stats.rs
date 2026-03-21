//! Atomic daemon metrics collected during operation.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use capsule_protocol::{PROTOCOL_VERSION, StatusResponse};

use super::{ReloadableConfig, SharedState};

/// Lock-free counters for daemon metrics.
///
/// All counters use `Relaxed` ordering — they are advisory and do not
/// synchronise other state.
pub(super) struct DaemonStats {
    // Cache
    pub(super) cache_hits: AtomicU64,
    pub(super) cache_misses: AtomicU64,
    pub(super) cache_evictions: AtomicU64,
    pub(super) inflight_coalesces: AtomicU64,

    // Request
    pub(super) requests_total: AtomicU64,
    pub(super) stale_discards: AtomicU64,

    // Slow compute
    pub(super) slow_computes_started: AtomicU64,
    pub(super) slow_compute_duration_us: AtomicU64,
    pub(super) git_timeouts: AtomicU64,
    pub(super) custom_module_timeouts: AtomicU64,

    // Session
    pub(super) sessions_pruned: AtomicU64,

    // Connection
    pub(super) connections_total: AtomicU64,
    pub(super) connections_active: AtomicU64,

    // Config
    pub(super) config_reloads: AtomicU64,
    pub(super) config_reload_errors: AtomicU64,

    // Daemon
    pub(super) started_at: Instant,
    pub(super) pid: u32,
}

impl DaemonStats {
    pub(super) fn new() -> Self {
        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            cache_evictions: AtomicU64::new(0),
            inflight_coalesces: AtomicU64::new(0),
            requests_total: AtomicU64::new(0),
            stale_discards: AtomicU64::new(0),
            slow_computes_started: AtomicU64::new(0),
            slow_compute_duration_us: AtomicU64::new(0),
            git_timeouts: AtomicU64::new(0),
            custom_module_timeouts: AtomicU64::new(0),
            sessions_pruned: AtomicU64::new(0),
            connections_total: AtomicU64::new(0),
            connections_active: AtomicU64::new(0),
            config_reloads: AtomicU64::new(0),
            config_reload_errors: AtomicU64::new(0),
            started_at: Instant::now(),
            pid: std::process::id(),
        }
    }

    /// Snapshot all counters into a [`StatusResponse`] ready for the wire.
    pub(super) fn snapshot(
        &self,
        state: &SharedState,
        config: &ReloadableConfig,
    ) -> StatusResponse {
        StatusResponse {
            version: PROTOCOL_VERSION,
            pid: self.pid,
            uptime_secs: self.started_at.elapsed().as_secs(),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            cache_evictions: self.cache_evictions.load(Ordering::Relaxed),
            cache_entries: state.cache_len() as u64,
            inflight_coalesces: self.inflight_coalesces.load(Ordering::Relaxed),
            requests_total: self.requests_total.load(Ordering::Relaxed),
            stale_discards: self.stale_discards.load(Ordering::Relaxed),
            slow_computes_started: self.slow_computes_started.load(Ordering::Relaxed),
            slow_compute_duration_us: self.slow_compute_duration_us.load(Ordering::Relaxed),
            git_timeouts: self.git_timeouts.load(Ordering::Relaxed),
            custom_module_timeouts: self.custom_module_timeouts.load(Ordering::Relaxed),
            active_sessions: state.session_len() as u64,
            sessions_pruned: self.sessions_pruned.load(Ordering::Relaxed),
            connections_total: self.connections_total.load(Ordering::Relaxed),
            connections_active: self.connections_active.load(Ordering::Relaxed),
            config_generation: config.generation(),
            config_reloads: self.config_reloads.load(Ordering::Relaxed),
            config_reload_errors: self.config_reload_errors.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_initialises_counters_to_zero() {
        let stats = DaemonStats::new();
        assert_eq!(stats.cache_hits.load(Ordering::Relaxed), 0);
        assert_eq!(stats.requests_total.load(Ordering::Relaxed), 0);
        assert_eq!(stats.connections_active.load(Ordering::Relaxed), 0);
        assert!(stats.pid > 0);
    }
}

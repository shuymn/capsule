//! Named stages of [`super::handle_request`] — makes the request pipeline easier to follow.

use std::sync::Arc;

use capsule_protocol::{ConfigGeneration, PromptGeneration, SessionId};

use super::super::CacheKey;
use crate::{
    config::Config,
    module::{RequestFacts, ResolvedModule},
};

/// Config and resolved modules after [`super::ReloadableConfig::snapshot`].
pub(super) struct ConfigSnapshot {
    pub(super) config: Arc<Config>,
    pub(super) modules: Arc<Vec<ResolvedModule>>,
    pub(super) config_generation: ConfigGeneration,
}

/// Correlates and shell-derived fields after the session generation gate.
pub(super) struct GatedPromptRequest {
    pub(super) session_id: SessionId,
    pub(super) generation: PromptGeneration,
    pub(super) cwd: String,
    pub(super) cols: u16,
    pub(super) last_exit_code: i32,
    pub(super) duration_ms: Option<u64>,
    pub(super) keymap: String,
}

/// Collected facts and derived cache key for the slow path.
pub(super) struct CollectedFacts {
    pub(super) facts: Arc<RequestFacts>,
    pub(super) cache_key: CacheKey,
}

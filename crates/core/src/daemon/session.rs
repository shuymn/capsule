//! Session tracking for connected clients.

use std::collections::HashMap;

use capsule_protocol::SessionId;

/// State for a connected client session.
pub(super) struct Session {
    last_generation: Option<u64>,
}

impl Session {
    const fn new() -> Self {
        Self {
            last_generation: None,
        }
    }

    /// Returns the last processed generation, if any.
    pub(super) const fn last_generation(&self) -> Option<u64> {
        self.last_generation
    }
}

/// Map of active sessions indexed by session ID.
pub(super) struct SessionMap {
    sessions: HashMap<SessionId, Session>,
}

impl SessionMap {
    pub(super) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Gets or creates a session for the given ID.
    pub(super) fn get_or_create(&mut self, id: SessionId) -> &mut Session {
        self.sessions.entry(id).or_insert_with(Session::new)
    }

    /// Checks if a request's generation is newer than the session's last.
    ///
    /// Updates the session's generation if so.
    /// Returns `true` if the request should be processed.
    pub(super) fn check_generation(&mut self, id: SessionId, generation: u64) -> bool {
        let session = self.get_or_create(id);
        match session.last_generation {
            Some(last) if generation <= last => false,
            _ => {
                session.last_generation = Some(generation);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sid() -> SessionId {
        SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
    }

    fn other_sid() -> SessionId {
        SessionId::from_bytes([0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88])
    }

    #[test]
    fn test_session_first_request_accepted() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 1));
    }

    #[test]
    fn test_session_first_request_generation_zero_accepted() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 0));
    }

    #[test]
    fn test_session_increasing_generation_accepted() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 1));
        assert!(map.check_generation(test_sid(), 2));
        assert!(map.check_generation(test_sid(), 5));
    }

    #[test]
    fn test_session_same_generation_rejected() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 3));
        assert!(!map.check_generation(test_sid(), 3));
    }

    #[test]
    fn test_session_older_generation_rejected() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 5));
        assert!(!map.check_generation(test_sid(), 3));
        assert!(!map.check_generation(test_sid(), 1));
    }

    #[test]
    fn test_session_independent_sessions() {
        let mut map = SessionMap::new();
        assert!(map.check_generation(test_sid(), 5));
        // Different session is independent
        assert!(map.check_generation(other_sid(), 1));
        // Original session still rejects old generations
        assert!(!map.check_generation(test_sid(), 3));
    }

    #[test]
    fn test_session_last_generation() {
        let mut map = SessionMap::new();
        let session = map.get_or_create(test_sid());
        assert_eq!(session.last_generation(), None);

        map.check_generation(test_sid(), 42);
        let session = map.get_or_create(test_sid());
        assert_eq!(session.last_generation(), Some(42));
    }
}

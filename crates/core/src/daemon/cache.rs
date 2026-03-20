//! Bounded cache with TTL eviction for slow module results.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// A bounded key-value cache with time-to-live eviction.
///
/// When the cache reaches `max_size`, the oldest entry is evicted on insertion.
/// Entries older than `ttl` are considered expired and not returned by [`get`](Self::get).
pub(super) struct BoundedCache<V> {
    entries: HashMap<String, CacheEntry<V>>,
    max_size: usize,
    ttl: Duration,
}

struct CacheEntry<V> {
    value: V,
    inserted_at: Instant,
}

impl<V> BoundedCache<V> {
    /// Creates a new cache with the given size limit and TTL.
    pub(super) fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::with_capacity(max_size),
            max_size,
            ttl,
        }
    }

    /// Returns a reference to the cached value if it exists and has not expired.
    pub(super) fn get(&self, key: &str) -> Option<&V> {
        let entry = self.entries.get(key)?;
        if entry.inserted_at.elapsed() > self.ttl {
            return None;
        }
        Some(&entry.value)
    }

    /// Inserts a value, evicting the oldest entry if the cache is full.
    pub(super) fn insert(&mut self, key: String, value: V) {
        if self.entries.len() >= self.max_size && !self.entries.contains_key(&key) {
            self.evict_oldest();
        }
        self.entries.insert(
            key,
            CacheEntry {
                value,
                inserted_at: Instant::now(),
            },
        );
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest_key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.inserted_at)
            .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_get_returns_none_for_missing_key() {
        let cache = BoundedCache::<String>::new(10, Duration::from_mins(1));
        assert!(cache.get("missing").is_none());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("/home/user".to_owned(), "cached".to_owned());
        assert_eq!(cache.get("/home/user"), Some(&"cached".to_owned()));
    }

    #[test]
    fn test_cache_update_existing_key() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("key".to_owned(), "v1".to_owned());
        cache.insert("key".to_owned(), "v2".to_owned());
        assert_eq!(cache.get("key"), Some(&"v2".to_owned()));
    }

    #[test]
    fn test_cache_evicts_oldest_on_max_size() {
        let mut cache = BoundedCache::new(2, Duration::from_mins(1));
        cache.insert("a".to_owned(), "1".to_owned());
        // Small delay so insertion times differ
        std::thread::sleep(Duration::from_millis(1));
        cache.insert("b".to_owned(), "2".to_owned());
        std::thread::sleep(Duration::from_millis(1));
        cache.insert("c".to_owned(), "3".to_owned());
        assert!(cache.get("a").is_none(), "oldest entry should be evicted");
        assert_eq!(cache.get("b"), Some(&"2".to_owned()));
        assert_eq!(cache.get("c"), Some(&"3".to_owned()));
    }

    #[test]
    fn test_cache_update_does_not_evict() {
        let mut cache = BoundedCache::new(2, Duration::from_mins(1));
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        // Updating existing key should not trigger eviction
        cache.insert("a".to_owned(), "updated".to_owned());
        assert_eq!(cache.get("a"), Some(&"updated".to_owned()));
        assert_eq!(cache.get("b"), Some(&"2".to_owned()));
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let mut cache = BoundedCache::new(10, Duration::from_millis(1));
        cache.insert("key".to_owned(), "val".to_owned());
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            cache.get("key").is_none(),
            "expired entry should not be returned"
        );
    }
}

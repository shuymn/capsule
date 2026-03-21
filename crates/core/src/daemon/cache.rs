//! Bounded cache with TTL eviction for slow module results.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

/// A bounded key-value cache with time-to-live eviction.
///
/// When the cache reaches `max_size`, the oldest entry is evicted on insertion.
/// Entries older than `ttl` are considered expired and not returned by [`get`](Self::get).
pub(super) struct BoundedCache<K, V> {
    entries: HashMap<K, CacheEntry<V>>,
    max_size: usize,
    ttl: Duration,
}

struct CacheEntry<V> {
    value: V,
    inserted_at: Instant,
}

impl<K, V> BoundedCache<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Creates a new cache with the given size limit and TTL.
    pub(super) fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::with_capacity(max_size),
            max_size,
            ttl,
        }
    }

    /// Returns a reference to the cached value if it exists and has not expired.
    ///
    /// Expired entries are removed from the cache on access.
    pub(super) fn get(&mut self, key: &K) -> Option<&V> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|e| e.inserted_at.elapsed() > self.ttl);
        if expired {
            self.entries.remove(key);
            return None;
        }
        self.entries.get(key).map(|e| &e.value)
    }

    /// Inserts a value, evicting the oldest entry if the cache is full.
    pub(super) fn insert(&mut self, key: K, value: V) {
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

    pub(super) fn clear(&mut self) {
        self.entries.clear();
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
        let mut cache = BoundedCache::<String, String>::new(10, Duration::from_mins(1));
        assert!(cache.get(&"missing".to_owned()).is_none());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("/home/user".to_owned(), "cached".to_owned());
        assert_eq!(
            cache.get(&"/home/user".to_owned()),
            Some(&"cached".to_owned())
        );
    }

    #[test]
    fn test_cache_update_existing_key() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("key".to_owned(), "v1".to_owned());
        cache.insert("key".to_owned(), "v2".to_owned());
        assert_eq!(cache.get(&"key".to_owned()), Some(&"v2".to_owned()));
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
        assert!(
            cache.get(&"a".to_owned()).is_none(),
            "oldest entry should be evicted"
        );
        assert_eq!(cache.get(&"b".to_owned()), Some(&"2".to_owned()));
        assert_eq!(cache.get(&"c".to_owned()), Some(&"3".to_owned()));
    }

    #[test]
    fn test_cache_update_does_not_evict() {
        let mut cache = BoundedCache::new(2, Duration::from_mins(1));
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        // Updating existing key should not trigger eviction
        cache.insert("a".to_owned(), "updated".to_owned());
        assert_eq!(cache.get(&"a".to_owned()), Some(&"updated".to_owned()));
        assert_eq!(cache.get(&"b".to_owned()), Some(&"2".to_owned()));
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let mut cache = BoundedCache::new(10, Duration::from_millis(1));
        cache.insert("key".to_owned(), "val".to_owned());
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            cache.get(&"key".to_owned()).is_none(),
            "expired entry should not be returned"
        );
    }

    #[test]
    fn test_cache_ttl_expiry_removes_entry() {
        let mut cache = BoundedCache::new(10, Duration::from_millis(1));
        cache.insert("key".to_owned(), "val".to_owned());
        assert_eq!(cache.entries.len(), 1);
        std::thread::sleep(Duration::from_millis(5));
        cache.get(&"key".to_owned());
        assert_eq!(
            cache.entries.len(),
            0,
            "expired entry should be removed from the map"
        );
    }
}

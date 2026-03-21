//! Bounded cache with TTL eviction for slow module results.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

/// Result of a cache lookup, distinguishing hit, miss, and TTL expiry.
pub(super) enum CacheGetResult<'a, V> {
    Hit(&'a V),
    Miss,
    Expired,
}

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

    /// Returns a reference to the cached value, distinguishing hit, miss, and
    /// TTL expiry.  Expired entries are removed on access.
    pub(super) fn get(&mut self, key: &K) -> CacheGetResult<'_, V> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|e| e.inserted_at.elapsed() > self.ttl);
        if expired {
            self.entries.remove(key);
            return CacheGetResult::Expired;
        }
        self.entries
            .get(key)
            .map_or(CacheGetResult::Miss, |e| CacheGetResult::Hit(&e.value))
    }

    /// Inserts a value, evicting the oldest entry if the cache is full.
    ///
    /// Returns `true` if an eviction occurred.
    pub(super) fn insert(&mut self, key: K, value: V) -> bool {
        let evicted = self.entries.len() >= self.max_size && !self.entries.contains_key(&key);
        if evicted {
            self.evict_oldest();
        }
        self.entries.insert(
            key,
            CacheEntry {
                value,
                inserted_at: Instant::now(),
            },
        );
        evicted
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
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
    fn test_cache_get_returns_miss_for_missing_key() {
        let mut cache = BoundedCache::<String, String>::new(10, Duration::from_mins(1));
        assert!(matches!(
            cache.get(&"missing".to_owned()),
            CacheGetResult::Miss
        ));
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("/home/user".to_owned(), "cached".to_owned());
        assert!(matches!(
            cache.get(&"/home/user".to_owned()),
            CacheGetResult::Hit(v) if *v == "cached"
        ));
    }

    #[test]
    fn test_cache_update_existing_key() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        cache.insert("key".to_owned(), "v1".to_owned());
        cache.insert("key".to_owned(), "v2".to_owned());
        assert!(matches!(
            cache.get(&"key".to_owned()),
            CacheGetResult::Hit(v) if *v == "v2"
        ));
    }

    #[test]
    fn test_cache_evicts_oldest_on_max_size() {
        let mut cache = BoundedCache::new(2, Duration::from_mins(1));
        assert!(!cache.insert("a".to_owned(), "1".to_owned()));
        std::thread::sleep(Duration::from_millis(1));
        assert!(!cache.insert("b".to_owned(), "2".to_owned()));
        std::thread::sleep(Duration::from_millis(1));
        assert!(
            cache.insert("c".to_owned(), "3".to_owned()),
            "should evict oldest entry"
        );
        assert!(
            matches!(cache.get(&"a".to_owned()), CacheGetResult::Miss),
            "oldest entry should be evicted"
        );
        assert!(matches!(cache.get(&"b".to_owned()), CacheGetResult::Hit(_)));
        assert!(matches!(cache.get(&"c".to_owned()), CacheGetResult::Hit(_)));
    }

    #[test]
    fn test_cache_update_does_not_evict() {
        let mut cache = BoundedCache::new(2, Duration::from_mins(1));
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        assert!(
            !cache.insert("a".to_owned(), "updated".to_owned()),
            "updating existing key should not evict"
        );
        assert!(matches!(
            cache.get(&"a".to_owned()),
            CacheGetResult::Hit(v) if *v == "updated"
        ));
        assert!(matches!(cache.get(&"b".to_owned()), CacheGetResult::Hit(_)));
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let mut cache = BoundedCache::new(10, Duration::from_millis(1));
        cache.insert("key".to_owned(), "val".to_owned());
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            matches!(cache.get(&"key".to_owned()), CacheGetResult::Expired),
            "expired entry should return Expired"
        );
    }

    #[test]
    fn test_cache_ttl_expiry_removes_entry() {
        let mut cache = BoundedCache::new(10, Duration::from_millis(1));
        cache.insert("key".to_owned(), "val".to_owned());
        assert_eq!(cache.len(), 1);
        std::thread::sleep(Duration::from_millis(5));
        cache.get(&"key".to_owned());
        assert_eq!(
            cache.len(),
            0,
            "expired entry should be removed from the map"
        );
    }

    #[test]
    fn test_cache_len() {
        let mut cache = BoundedCache::new(10, Duration::from_mins(1));
        assert_eq!(cache.len(), 0);
        cache.insert("a".to_owned(), "1".to_owned());
        assert_eq!(cache.len(), 1);
        cache.insert("b".to_owned(), "2".to_owned());
        assert_eq!(cache.len(), 2);
    }
}

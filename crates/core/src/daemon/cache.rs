//! Bounded LRU cache for slow module results.

use std::{collections::HashMap, hash::Hash};

/// A bounded key-value cache with least-recently-used eviction.
///
/// When the cache reaches `max_size`, the least recently accessed entry is
/// evicted on insertion.  Accessing an entry via [`get`](Self::get) promotes it
/// to most-recently-used.
pub(super) struct BoundedCache<K, V> {
    entries: HashMap<K, CacheEntry<V>>,
    max_size: usize,
    /// Monotonic counter incremented on every access; used for LRU ordering.
    tick: u64,
}

struct CacheEntry<V> {
    value: V,
    last_tick: u64,
}

impl<K, V> BoundedCache<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Creates a new cache with the given size limit.
    pub(super) fn new(max_size: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(max_size),
            max_size,
            tick: 0,
        }
    }

    /// Returns a reference to the cached value if present, promoting it to
    /// most-recently-used.
    pub(super) fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let entry = self.entries.get_mut(key)?;
        entry.last_tick = self.tick;
        Some(&entry.value)
    }

    /// Inserts a value, evicting the least recently used entry if the cache is
    /// full.
    ///
    /// Returns `true` if an eviction occurred.
    pub(super) fn insert(&mut self, key: K, value: V) -> bool {
        // Double lookup is intentional: the Entry API borrows the map mutably,
        // preventing the `evict_lru` call inside the Vacant arm.
        let evicted = self.entries.len() >= self.max_size && !self.entries.contains_key(&key);
        if evicted {
            self.evict_lru();
        }
        self.tick += 1;
        self.entries.insert(
            key,
            CacheEntry {
                value,
                last_tick: self.tick,
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

    fn evict_lru(&mut self) {
        if let Some(lru_key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_tick)
            .map(|(key, _)| key.clone())
        {
            self.entries.remove(&lru_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_get_returns_none_for_missing_key() {
        let mut cache = BoundedCache::<String, String>::new(10);
        assert!(cache.get(&"missing".to_owned()).is_none());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = BoundedCache::new(10);
        cache.insert("/home/user".to_owned(), "cached".to_owned());
        assert_eq!(
            cache.get(&"/home/user".to_owned()),
            Some(&"cached".to_owned())
        );
    }

    #[test]
    fn test_cache_update_existing_key() {
        let mut cache = BoundedCache::new(10);
        cache.insert("key".to_owned(), "v1".to_owned());
        cache.insert("key".to_owned(), "v2".to_owned());
        assert_eq!(cache.get(&"key".to_owned()), Some(&"v2".to_owned()));
    }

    #[test]
    fn test_cache_evicts_lru_on_max_size() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        assert!(
            cache.insert("c".to_owned(), "3".to_owned()),
            "should evict LRU entry"
        );
        assert!(
            cache.get(&"a".to_owned()).is_none(),
            "LRU entry should be evicted"
        );
        assert!(cache.get(&"b".to_owned()).is_some());
        assert!(cache.get(&"c".to_owned()).is_some());
    }

    #[test]
    fn test_cache_lru_promotion() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        // Access "a" to promote it — "b" becomes LRU.
        cache.get(&"a".to_owned());
        cache.insert("c".to_owned(), "3".to_owned());
        assert!(
            cache.get(&"b".to_owned()).is_none(),
            "b should be evicted as LRU"
        );
        assert!(cache.get(&"a".to_owned()).is_some());
        assert!(cache.get(&"c".to_owned()).is_some());
    }

    #[test]
    fn test_cache_update_does_not_evict() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".to_owned(), "1".to_owned());
        cache.insert("b".to_owned(), "2".to_owned());
        assert!(
            !cache.insert("a".to_owned(), "updated".to_owned()),
            "updating existing key should not evict"
        );
        assert_eq!(cache.get(&"a".to_owned()), Some(&"updated".to_owned()));
        assert!(cache.get(&"b".to_owned()).is_some());
    }

    #[test]
    fn test_cache_len() {
        let mut cache = BoundedCache::new(10);
        assert_eq!(cache.len(), 0);
        cache.insert("a".to_owned(), "1".to_owned());
        assert_eq!(cache.len(), 1);
        cache.insert("b".to_owned(), "2".to_owned());
        assert_eq!(cache.len(), 2);
    }
}

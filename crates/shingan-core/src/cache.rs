use lru::LruCache;
use std::hash::Hash;
use std::num::NonZeroUsize;

/// A bounded LRU cache wrapping `lru::LruCache`.
pub struct BoundedCache<K: Hash + Eq, V> {
    inner: LruCache<K, V>,
}

impl<K: Hash + Eq, V> BoundedCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1).unwrap());
        Self {
            inner: LruCache::new(cap),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    pub fn put(&mut self, key: K, value: V) {
        self.inner.put(key, value);
    }

    pub fn contains(&self, key: &K) -> bool {
        self.inner.contains(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

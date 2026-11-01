//! Keeps the posting lists of frequently queried ("hot") terms decoded in memory.
//!
//! Without it every query reads each term's blob from RocksDB and VByte-decodes
//! it. Query traffic is skewed — a few terms appear in most queries — so a
//! small least-recently-used cache of decoded lists absorbs most of that work.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lru::LruCache;
use serde::Serialize;

use crate::index::postings::{Posting, PostingList};

pub struct HotTermCache {
    /// `None` when the cache is disabled (capacity 0).
    inner: Option<Mutex<Inner>>,
    capacity: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

struct Inner {
    lists: LruCache<String, Arc<PostingList>>,
    resident_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HotTermStats {
    pub capacity: usize,
    pub resident_terms: usize,
    /// Approximate heap bytes of the cached posting lists.
    pub resident_bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

impl HotTermCache {
    /// `capacity` is a number of terms; 0 disables caching.
    pub fn new(capacity: usize) -> Self {
        let inner = NonZeroUsize::new(capacity).map(|cap| {
            Mutex::new(Inner {
                lists: LruCache::new(cap),
                resident_bytes: 0,
            })
        });
        Self {
            inner,
            capacity,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Returns the cached list for `term`, or calls `load` and caches its result.
    /// Missing terms and errors are not cached.
    pub fn get_or_load<E>(
        &self,
        term: &str,
        load: impl FnOnce() -> Result<Option<PostingList>, E>,
    ) -> Result<Option<Arc<PostingList>>, E> {
        let Some(inner) = &self.inner else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return Ok(load()?.map(Arc::new));
        };
        if let Some(list) = lock(inner).lists.get(term) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(Arc::clone(list)));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);

        // Load without holding the lock, so a slow disk read for one term does
        // not block queries for other terms. Two threads may occasionally load
        // the same term at once; the second insert just replaces the first.
        let Some(list) = load()?.map(Arc::new) else {
            return Ok(None);
        };
        let mut guard = lock(inner);
        guard.resident_bytes += heap_bytes(&list);
        if let Some((_, evicted)) = guard.lists.push(term.to_string(), Arc::clone(&list)) {
            guard.resident_bytes -= heap_bytes(&evicted);
        }
        Ok(Some(list))
    }

    pub fn stats(&self) -> HotTermStats {
        let (resident_terms, resident_bytes) = match &self.inner {
            Some(inner) => {
                let guard = lock(inner);
                (guard.lists.len(), guard.resident_bytes)
            }
            None => (0, 0),
        };
        HotTermStats {
            capacity: self.capacity,
            resident_terms,
            resident_bytes,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }
}

/// A panic while holding the lock cannot leave the LRU half-updated in a way
/// that matters here, so recover the guard instead of propagating the poison.
fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

fn heap_bytes(list: &PostingList) -> usize {
    std::mem::size_of_val(list.postings()) + list.position_count() * std::mem::size_of::<u32>() + size_of::<Posting>()
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    fn list(doc_id: u32) -> PostingList {
        PostingList::new(vec![Posting {
            doc_id,
            positions: vec![0, 1],
        }])
    }

    fn load(doc_id: u32) -> impl FnOnce() -> Result<Option<PostingList>, Infallible> {
        move || Ok(Some(list(doc_id)))
    }

    #[test]
    fn second_lookup_is_a_hit() {
        let cache = HotTermCache::new(10);
        let Ok(first) = cache.get_or_load("rust", load(1));
        let Ok(second) = cache.get_or_load("rust", || -> Result<_, Infallible> { panic!("should be cached") });
        assert!(Arc::ptr_eq(&first.unwrap(), &second.unwrap()));
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.resident_terms), (1, 1, 1));
        assert!(stats.resident_bytes > 0);
    }

    #[test]
    fn evicts_the_least_recently_used_term() {
        let cache = HotTermCache::new(2);
        let _ = cache.get_or_load("a", load(1));
        let _ = cache.get_or_load("b", load(2));
        let _ = cache.get_or_load("a", load(1)); // touch a, so b is now least recent
        let _ = cache.get_or_load("c", load(3)); // evicts b

        // Probe the survivors first: reloading "b" evicts again.
        let mut reloaded = Vec::new();
        for term in ["a", "c", "b"] {
            let _ = cache.get_or_load(term, || -> Result<_, Infallible> {
                reloaded.push(term);
                Ok(Some(list(9)))
            });
        }
        assert_eq!(reloaded, ["b"]);
        assert_eq!(cache.stats().resident_terms, 2);
    }

    #[test]
    fn resident_bytes_shrink_on_eviction() {
        let cache = HotTermCache::new(1);
        let _ = cache.get_or_load("a", load(1));
        let one = cache.stats().resident_bytes;
        let _ = cache.get_or_load("b", load(2));
        assert_eq!(cache.stats().resident_bytes, one);
    }

    #[test]
    fn missing_terms_and_errors_are_not_cached() {
        let cache = HotTermCache::new(10);
        let _ = cache.get_or_load("absent", || -> Result<_, Infallible> { Ok(None) });
        assert!(cache.get_or_load("broken", || Err("disk error")).is_err());
        assert_eq!(cache.stats().resident_terms, 0);
    }

    #[test]
    fn zero_capacity_disables_caching() {
        let cache = HotTermCache::new(0);
        let _ = cache.get_or_load("rust", load(1));
        let _ = cache.get_or_load("rust", load(1));
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.resident_terms), (0, 2, 0));
    }
}

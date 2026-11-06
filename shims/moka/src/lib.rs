//! The part of the moka API the OctoFHIR libraries use, without clocks or threads.
//!
//! moka reads `std::time::Instant`, which panics on `wasm32-unknown-unknown`. A browser needs none
//! of what it does: this is a map emptied when it is full, and time to live is not kept.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Entries kept when a capacity counts bytes through a weigher.
const WEIGHED_ENTRIES: u64 = 4096;

struct Shared<K, V> {
    capacity: u64,
    entries: Mutex<HashMap<K, V>>,
}

impl<K: Hash + Eq, V: Clone> Shared<K, V> {
    fn new(capacity: u64) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            entries: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<K, V>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.lock().get(key).cloned()
    }

    fn insert(&self, key: K, value: V) {
        let mut entries = self.lock();
        if entries.len() as u64 >= self.capacity {
            entries.clear();
        }
        entries.insert(key, value);
    }

    fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.lock().remove(key)
    }

    fn clear(&self) {
        self.lock().clear();
    }

    fn len(&self) -> u64 {
        self.lock().len() as u64
    }
}

/// `Cache::builder()` of both kinds of cache.
pub struct CacheBuilder<K, V, C> {
    capacity: u64,
    weighed: bool,
    marker: PhantomData<(K, V, C)>,
}

impl<K, V, C> Default for CacheBuilder<K, V, C> {
    fn default() -> Self {
        Self {
            capacity: u64::MAX,
            weighed: false,
            marker: PhantomData,
        }
    }
}

impl<K, V, C> CacheBuilder<K, V, C> {
    pub fn max_capacity(mut self, capacity: u64) -> Self {
        self.capacity = capacity;
        self
    }

    pub fn time_to_live(self, _duration: Duration) -> Self {
        self
    }

    pub fn time_to_idle(self, _duration: Duration) -> Self {
        self
    }

    pub fn weigher(mut self, _weigher: impl Fn(&K, &V) -> u32) -> Self {
        self.weighed = true;
        self
    }

    fn entries(&self) -> u64 {
        if self.weighed {
            WEIGHED_ENTRIES
        } else {
            self.capacity
        }
    }
}

pub mod sync {
    use super::*;

    pub struct Cache<K, V>(Arc<Shared<K, V>>);

    impl<K, V> Clone for Cache<K, V> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<K: Hash + Eq, V: Clone> CacheBuilder<K, V, Cache<K, V>> {
        pub fn build(self) -> Cache<K, V> {
            Cache(Shared::new(self.entries()))
        }
    }

    impl<K: Hash + Eq, V: Clone> Cache<K, V> {
        pub fn new(capacity: u64) -> Self {
            Self(Shared::new(capacity))
        }

        pub fn builder() -> CacheBuilder<K, V, Self> {
            CacheBuilder::default()
        }

        pub fn get<Q>(&self, key: &Q) -> Option<V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.0.get(key)
        }

        pub fn insert(&self, key: K, value: V) {
            self.0.insert(key, value);
        }

        pub fn get_with(&self, key: K, init: impl FnOnce() -> V) -> V {
            if let Some(value) = self.0.get(&key) {
                return value;
            }
            let value = init();
            self.0.insert(key, value.clone());
            value
        }

        pub fn get_with_by_ref<Q>(&self, key: &Q, init: impl FnOnce() -> V) -> V
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        {
            if let Some(value) = self.0.get(key) {
                return value;
            }
            let value = init();
            self.0.insert(key.to_owned(), value.clone());
            value
        }

        pub fn try_get_with_by_ref<Q, E>(
            &self,
            key: &Q,
            init: impl FnOnce() -> Result<V, E>,
        ) -> Result<V, Arc<E>>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        {
            if let Some(value) = self.0.get(key) {
                return Ok(value);
            }
            let value = init().map_err(Arc::new)?;
            self.0.insert(key.to_owned(), value.clone());
            Ok(value)
        }

        pub fn invalidate<Q>(&self, key: &Q)
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.0.remove(key);
        }

        pub fn remove<Q>(&self, key: &Q) -> Option<V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.0.remove(key)
        }

        pub fn invalidate_all(&self) {
            self.0.clear();
        }

        pub fn entry_count(&self) -> u64 {
            self.0.len()
        }

        pub fn weighted_size(&self) -> u64 {
            self.0.len()
        }

        pub fn run_pending_tasks(&self) {}
    }
}

pub mod future {
    use std::future::Future;

    use super::*;

    pub struct Cache<K, V>(Arc<Shared<K, V>>);

    impl<K, V> Clone for Cache<K, V> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<K: Hash + Eq, V: Clone> CacheBuilder<K, V, Cache<K, V>> {
        pub fn build(self) -> Cache<K, V> {
            Cache(Shared::new(self.entries()))
        }
    }

    impl<K: Hash + Eq, V: Clone> Cache<K, V> {
        pub fn new(capacity: u64) -> Self {
            Self(Shared::new(capacity))
        }

        pub fn builder() -> CacheBuilder<K, V, Self> {
            CacheBuilder::default()
        }

        pub async fn get<Q>(&self, key: &Q) -> Option<V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.0.get(key)
        }

        pub async fn insert(&self, key: K, value: V) {
            self.0.insert(key, value);
        }

        pub async fn try_get_with_by_ref<Q, E>(
            &self,
            key: &Q,
            init: impl Future<Output = Result<V, E>>,
        ) -> Result<V, Arc<E>>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        {
            if let Some(value) = self.0.get(key) {
                return Ok(value);
            }
            let value = init.await.map_err(Arc::new)?;
            self.0.insert(key.to_owned(), value.clone());
            Ok(value)
        }

        pub async fn invalidate<Q>(&self, key: &Q)
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            self.0.remove(key);
        }

        pub fn invalidate_all(&self) {
            self.0.clear();
        }

        pub fn entry_count(&self) -> u64 {
            self.0.len()
        }

        pub fn weighted_size(&self) -> u64 {
            self.0.len()
        }

        pub async fn run_pending_tasks(&self) {}
    }
}

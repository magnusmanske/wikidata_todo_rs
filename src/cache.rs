//! A value fetched now and then from elsewhere, shared until it is too old.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct TimedCache<T> {
    max_age: Duration,
    slot: Mutex<Option<(Instant, Arc<T>)>>,
}

impl<T> TimedCache<T> {
    pub const fn new(max_age: Duration) -> Self {
        Self { max_age, slot: Mutex::const_new(None) }
    }

    /// The cached value, if it is fresh and no fetch holds the cache right now.
    pub fn peek(&self) -> Option<Arc<T>> {
        let slot = self.slot.try_lock().ok()?;
        slot.as_ref().filter(|(fetched, _)| fetched.elapsed() < self.max_age).map(|(_, value)| value.clone())
    }

    /// The cached value, or a fresh one from `fetch`. Concurrent callers wait
    /// for one fetch rather than each making their own.
    pub async fn get_or_fetch<E>(&self, fetch: impl AsyncFnOnce() -> Result<T, E>) -> Result<Arc<T>, E> {
        let mut slot = self.slot.lock().await;
        if let Some((fetched, value)) = slot.as_ref()
            && fetched.elapsed() < self.max_age
        {
            return Ok(value.clone());
        }
        let value = Arc::new(fetch().await?);
        *slot = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

/// Values by key, with when they were fetched.
type Slots<T> = HashMap<String, (Instant, Arc<T>)>;

/// As [`TimedCache`], one value per key (e.g. per wiki); at most `max_keys`.
pub struct KeyedCache<T> {
    max_age: Duration,
    max_keys: usize,
    slots: LazyLock<std::sync::Mutex<Slots<T>>>,
}

impl<T> KeyedCache<T> {
    pub const fn new(max_age: Duration, max_keys: usize) -> Self {
        Self { max_age, max_keys, slots: LazyLock::new(Default::default) }
    }

    /// The cached value for `key`, or a fresh one from `fetch`. (Concurrent
    /// callers may each fetch; the last one is kept.)
    pub async fn get_or_fetch<E>(&self, key: &str, fetch: impl AsyncFnOnce() -> Result<T, E>) -> Result<Arc<T>, E> {
        if let Some((fetched, value)) = self.slots.lock().ok().and_then(|slots| slots.get(key).cloned())
            && fetched.elapsed() < self.max_age
        {
            return Ok(value);
        }
        let value = Arc::new(fetch().await?);
        if let Ok(mut slots) = self.slots.lock() {
            slots.retain(|_, (fetched, _)| fetched.elapsed() < self.max_age);
            if slots.len() < self.max_keys {
                slots.insert(key.to_string(), (Instant::now(), value.clone()));
            }
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetches_once_until_stale() {
        let cache = TimedCache::new(Duration::from_secs(60));
        let first = cache.get_or_fetch(async || Ok::<_, ()>(1)).await.unwrap();
        let second = cache.get_or_fetch(async || Ok::<_, ()>(2)).await.unwrap();
        assert_eq!((*first, *second), (1, 1));

        let stale = TimedCache::new(Duration::ZERO);
        stale.get_or_fetch(async || Ok::<_, ()>(1)).await.unwrap();
        assert_eq!(*stale.get_or_fetch(async || Ok::<_, ()>(2)).await.unwrap(), 2);
        assert!(stale.get_or_fetch(async || Err::<i32, _>("down")).await.is_err());
    }

    #[tokio::test]
    async fn one_value_per_key() {
        let cache = KeyedCache::new(Duration::from_secs(60), 2);
        assert_eq!(*cache.get_or_fetch("a", async || Ok::<_, ()>(1)).await.unwrap(), 1);
        assert_eq!(*cache.get_or_fetch("b", async || Ok::<_, ()>(2)).await.unwrap(), 2);
        assert_eq!(*cache.get_or_fetch("a", async || Ok::<_, ()>(3)).await.unwrap(), 1);
        // Full: fetched, but not kept.
        assert_eq!(*cache.get_or_fetch("c", async || Ok::<_, ()>(4)).await.unwrap(), 4);
        assert_eq!(*cache.get_or_fetch("c", async || Ok::<_, ()>(5)).await.unwrap(), 5);
    }
}

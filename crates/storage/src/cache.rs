//! Cache storage implementations

use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

/// Cache entry
#[derive(Debug, Clone)]
struct CacheEntry<V> {
    value: V,
    expires_at: Instant,
    access_count: u64,
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total entries
    pub entries: usize,
    /// Hit count
    pub hits: u64,
    /// Miss count
    pub misses: u64,
    /// Eviction count
    pub evictions: u64,
}

impl CacheStats {
    /// Hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Cache trait
#[async_trait]
pub trait Cache<K, V>: Send + Sync
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Get value
    async fn get(&self, key: &K) -> Option<V>;
    
    /// Set value with TTL
    async fn set(&self, key: K, value: V, ttl: Duration);
    
    /// Delete key
    async fn delete(&self, key: &K);
    
    /// Check if key exists
    async fn contains(&self, key: &K) -> bool;
    
    /// Clear all entries
    async fn clear(&self);
    
    /// Get statistics
    async fn stats(&self) -> CacheStats;
}

/// In-memory cache with TTL and LRU eviction
pub struct MemoryCache<K, V> {
    data: RwLock<HashMap<K, CacheEntry<V>>>,
    max_size: usize,
    default_ttl: Duration,
    stats: RwLock<CacheStats>,
}

impl<K, V> MemoryCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create new cache
    pub fn new(max_size: usize, default_ttl: Duration) -> Self {
        Self {
            data: RwLock::new(HashMap::with_capacity(max_size)),
            max_size,
            default_ttl,
            stats: RwLock::new(CacheStats::default()),
        }
    }

    /// Create cache with builder
    pub fn builder() -> MemoryCacheBuilder<K, V> {
        MemoryCacheBuilder::new()
    }

    /// Cleanup expired entries
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut data = self.data.write().await;
        
        let expired: Vec<K> = data
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        
        for key in expired {
            data.remove(&key);
        }
    }

    /// Evict least recently used entries
    async fn evict_if_needed(&self) {
        let data = self.data.read().await;
        if data.len() < self.max_size {
            return;
        }
        drop(data);

        let mut data = self.data.write().await;
        
        // Find entry with lowest access count
        if let Some(key_to_remove) = data
            .iter()
            .min_by_key(|(_, entry)| entry.access_count)
            .map(|(k, _)| k.clone())
        {
            data.remove(&key_to_remove);
            
            let mut stats = self.stats.write().await;
            stats.evictions += 1;
        }
    }
}

impl<K, V> Default for MemoryCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn default() -> Self {
        Self::new(1000, Duration::from_secs(300))
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for MemoryCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Option<V> {
        let now = Instant::now();
        let mut data = self.data.write().await;
        
        if let Some(entry) = data.get_mut(key) {
            if entry.expires_at > now {
                entry.access_count += 1;
                
                let mut stats = self.stats.write().await;
                stats.hits += 1;
                
                return Some(entry.value.clone());
            } else {
                data.remove(key);
            }
        }
        
        let mut stats = self.stats.write().await;
        stats.misses += 1;
        
        None
    }

    async fn set(&self, key: K, value: V, ttl: Duration) {
        self.evict_if_needed().await;
        
        let entry = CacheEntry {
            value,
            expires_at: Instant::now() + ttl,
            access_count: 1,
        };
        
        let mut data = self.data.write().await;
        data.insert(key, entry);
    }

    async fn delete(&self, key: &K) {
        let mut data = self.data.write().await;
        data.remove(key);
    }

    async fn contains(&self, key: &K) -> bool {
        let now = Instant::now();
        let data = self.data.read().await;
        
        if let Some(entry) = data.get(key) {
            entry.expires_at > now
        } else {
            false
        }
    }

    async fn clear(&self) {
        let mut data = self.data.write().await;
        data.clear();
    }

    async fn stats(&self) -> CacheStats {
        let data = self.data.read().await;
        let stats = self.stats.read().await;
        
        CacheStats {
            entries: data.len(),
            hits: stats.hits,
            misses: stats.misses,
            evictions: stats.evictions,
        }
    }
}

/// Cache builder
pub struct MemoryCacheBuilder<K, V> {
    max_size: usize,
    default_ttl: Duration,
    _k: std::marker::PhantomData<K>,
    _v: std::marker::PhantomData<V>,
}

impl<K, V> MemoryCacheBuilder<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create new builder
    fn new() -> Self {
        Self {
            max_size: 1000,
            default_ttl: Duration::from_secs(300),
            _k: std::marker::PhantomData,
            _v: std::marker::PhantomData,
        }
    }

    /// Set max size
    pub fn max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    /// Set default TTL
    pub fn default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = ttl;
        self
    }

    /// Build cache
    pub fn build(self) -> MemoryCache<K, V> {
        MemoryCache::new(self.max_size, self.default_ttl)
    }
}

/// Tiered cache (L1 memory, L2 backing)
pub struct TieredCache<K, V> {
    l1: Arc<dyn Cache<K, V>>,
    l2: Arc<dyn Cache<K, V>>,
}

impl<K, V> TieredCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create tiered cache
    pub fn new(l1: Arc<dyn Cache<K, V>>, l2: Arc<dyn Cache<K, V>>) -> Self {
        Self { l1, l2 }
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for TieredCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Option<V> {
        // Try L1 first
        if let Some(value) = self.l1.get(key).await {
            return Some(value);
        }
        
        // Try L2
        if let Some(value) = self.l2.get(key).await {
            // Promote to L1
            self.l1.set(key.clone(), value.clone(), Duration::from_secs(300)).await;
            return Some(value);
        }
        
        None
    }

    async fn set(&self, key: K, value: V, ttl: Duration) {
        // Set in both layers
        self.l1.set(key.clone(), value.clone(), ttl).await;
        self.l2.set(key, value, ttl).await;
    }

    async fn delete(&self, key: &K) {
        self.l1.delete(key).await;
        self.l2.delete(key).await;
    }

    async fn contains(&self, key: &K) -> bool {
        self.l1.contains(key).await || self.l2.contains(key).await
    }

    async fn clear(&self) {
        self.l1.clear().await;
        self.l2.clear().await;
    }

    async fn stats(&self) -> CacheStats {
        let s1 = self.l1.stats().await;
        let s2 = self.l2.stats().await;
        
        CacheStats {
            entries: s1.entries + s2.entries,
            hits: s1.hits + s2.hits,
            misses: s1.misses + s2.misses,
            evictions: s1.evictions + s2.evictions,
        }
    }
}

/// Null cache (always misses)
pub struct NullCache<K, V> {
    _k: std::marker::PhantomData<K>,
    _v: std::marker::PhantomData<V>,
}

impl<K, V> NullCache<K, V> {
    /// Create null cache
    pub fn new() -> Self {
        Self {
            _k: std::marker::PhantomData,
            _v: std::marker::PhantomData,
        }
    }
}

impl<K, V> Default for NullCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for NullCache<K, V>
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, _key: &K) -> Option<V> {
        None
    }

    async fn set(&self, _key: K, _value: V, _ttl: Duration) {}

    async fn delete(&self, _key: &K) {}

    async fn contains(&self, _key: &K) -> bool {
        false
    }

    async fn clear(&self) {}

    async fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_cache() {
        let cache = MemoryCache::<String, i32>::new(100, Duration::from_secs(60));
        
        // Set and get
        cache.set("key1".to_string(), 42, Duration::from_secs(60)).await;
        assert_eq!(cache.get(&"key1".to_string()).await, Some(42));
        
        // Non-existent key
        assert_eq!(cache.get(&"key2".to_string()).await, None);
        
        // Contains
        assert!(cache.contains(&"key1".to_string()).await);
        assert!(!cache.contains(&"key2".to_string()).await);
        
        // Delete
        cache.delete(&"key1".to_string()).await;
        assert!(!cache.contains(&"key1".to_string()).await);
    }

    #[tokio::test]
    async fn test_cache_ttl() {
        let cache = MemoryCache::<String, i32>::new(100, Duration::from_millis(50));
        
        cache.set("key".to_string(), 42, Duration::from_millis(10)).await;
        assert!(cache.contains(&"key".to_string()).await);
        
        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!cache.contains(&"key".to_string()).await);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = MemoryCache::<String, i32>::new(100, Duration::from_secs(60));
        
        cache.set("key".to_string(), 42, Duration::from_secs(60)).await;
        
        cache.get(&"key".to_string()).await; // hit
        cache.get(&"key".to_string()).await; // hit
        cache.get(&"missing".to_string()).await; // miss
        
        let stats = cache.stats().await;
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 0.666).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let cache = MemoryCache::<String, i32>::new(2, Duration::from_secs(60));
        
        cache.set("a".to_string(), 1, Duration::from_secs(60)).await;
        cache.set("b".to_string(), 2, Duration::from_secs(60)).await;
        cache.set("c".to_string(), 3, Duration::from_secs(60)).await; // Should evict one
        
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evictions, 1);
    }

    #[tokio::test]
    async fn test_null_cache() {
        let cache = NullCache::<String, i32>::new();
        
        cache.set("key".to_string(), 42, Duration::from_secs(60)).await;
        assert_eq!(cache.get(&"key".to_string()).await, None);
        assert!(!cache.contains(&"key".to_string()).await);
    }
}

//! AnyMap — TypeId 索引的 type-erased 容器
//!
//! 每个 TypeId 至多存一个值，类型安全且无运行时 downcast 错误。
//! 不含 nuzo Value，故无需 TraceRef 实现。

use nuzo_core::XxHashMap;
use std::any::{Any, TypeId};

/// TypeId 索引的异构容器
pub struct AnyMap {
    inner: XxHashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl AnyMap {
    pub fn new() -> Self {
        Self { inner: XxHashMap::default() }
    }

    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> Option<T> {
        let id = TypeId::of::<T>();
        let old = self.inner.insert(id, Box::new(value));
        old.and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.inner.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    pub fn get_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.inner.get_mut(&TypeId::of::<T>()).and_then(|b| b.downcast_mut::<T>())
    }

    pub fn remove<T: Any + Send + Sync>(&mut self) -> Option<T> {
        self.inner.remove(&TypeId::of::<T>()).and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn contains<T: Any + Send + Sync>(&self) -> bool {
        self.inner.contains_key(&TypeId::of::<T>())
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

impl Default for AnyMap {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AnyMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnyMap").field("len", &self.inner.len()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_get() {
        let mut m = AnyMap::new();
        m.insert(42u32);
        assert_eq!(m.get::<u32>(), Some(&42u32));
        assert!(m.contains::<u32>());
        assert!(!m.contains::<u64>());
    }

    #[test]
    fn test_override() {
        let mut m = AnyMap::new();
        m.insert(1u32);
        let old = m.insert(2u32);
        assert_eq!(old, Some(1u32));
        assert_eq!(m.get::<u32>(), Some(&2u32));
    }

    #[test]
    fn test_multi_type() {
        let mut m = AnyMap::new();
        m.insert(1u32);
        m.insert("hello".to_string());
        m.insert(vec![1u8, 2, 3]);
        assert_eq!(m.len(), 3);
        assert_eq!(m.get::<String>(), Some(&"hello".to_string()));
        assert_eq!(m.get::<Vec<u8>>(), Some(&vec![1u8, 2, 3]));
    }

    #[test]
    fn test_remove() {
        let mut m = AnyMap::new();
        m.insert(42u32);
        let removed = m.remove::<u32>();
        assert_eq!(removed, Some(42u32));
        assert!(!m.contains::<u32>());
        assert_eq!(m.len(), 0);
    }
}

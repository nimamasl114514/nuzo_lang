//! xxHash-based 哈希容器类型别名
//!
//! 提供使用 xxHash3 作为默认 hasher 的 HashMap/HashSet，
//! 替代标准库的 SipHash 以获得更好的性能。
//!
//! ## 性能对比
//!
//! | Hasher | 吞吐量 (GB/s) | 适用场景 |
//! |--------|--------------|---------|
//! | SipHash 1-3 (std) | ~0.5 | 默认安全选项 |
//! | xxHash3 | ~5-10 | VM 热路径、高频查找 |
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use nuzo_core::hash::{XxHashMap, xx_hash_map};
//!
//! let mut map: XxHashMap<String, i32> = xx_hash_map(64);
//! map.insert("key".to_string(), 42);
//! ```

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use xxhash_rust::xxh3::Xxh3;

/// 使用 xxHash3 作为默认 hasher 的 HashMap
///
/// 比 `std::collections::HashMap`（使用 SipHash 1-3）快 **5-10 倍**，
/// 适合 VM 热路径、符号表、字符串缓存等性能敏感场景。
///
/// # 安全性说明
///
/// xxHash3 **非加密安全**，仅适用于哈希表内部使用，
/// 禁止用于 HMAC、数字签名等安全敏感场景。
///
/// # 类型参数
///
/// - `K`: 键类型，必须实现 `Eq` + `Hash`
/// - `V`: 值类型
pub type XxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<Xxh3>>;

/// 使用 xxHash3 作为默认 hasher 的 HashSet
///
/// 性能特征同 [`XxHashMap`]，适用于去重、成员检测等集合操作。
///
/// # 类型参数
///
/// - `K`: 元素类型，必须实现 `Eq` + `Hash`
pub type XxHashSet<K> = HashSet<K, BuildHasherDefault<Xxh3>>;

/// 创建带预分配容量的 XxHashMap
///
/// 预分配可避免插入时的 rehash，适合已知大致规模的场景（如符号表初始化）。
///
/// # 参数
///
/// - `capacity`: 预期元素数量（不是字节数）
///
/// # 返回值
///
/// 空的 `XxHashMap`，底层缓冲区至少能容纳 `capacity` 个元素而不 rehash
///
/// # Example
///
/// ```rust,ignore
/// use nuzo_core::hash::{XxHashMap, xx_hash_map};
///
/// let map: XxHashMap<String, i32> = xx_hash_map(64);
/// assert!(map.capacity() >= 64);
/// ```
#[inline]
pub fn xx_hash_map<K, V>(capacity: usize) -> XxHashMap<K, V> {
    XxHashMap::with_capacity_and_hasher(capacity, BuildHasherDefault::default())
}

/// 创建空 XxHashMap（无预分配）
///
/// 等价于 `XxHashMap::with_hasher(BuildHasherDefault::default())`，
/// 提供更简洁的调用语法。
///
/// # Example
///
/// ```rust,ignore
/// use nuzo_core::hash::{XxHashMap, xx_hash_map_new};
///
/// let mut map: XxHashMap<i32, String> = xx_hash_map_new();
/// map.insert(1, "hello".to_string());
/// ```
#[inline]
pub fn xx_hash_map_new<K, V>() -> XxHashMap<K, V> {
    XxHashMap::with_hasher(BuildHasherDefault::default())
}

/// 创建带预分配容量的 XxHashSet
///
/// 同 [`xx_hash_map`]，但用于集合场景。
///
/// # Example
///
/// ```rust,ignore
/// use nuzo_core::hash::{XxHashSet, xx_hash_set};
///
/// let set: XxHashSet<i32> = xx_hash_set(32);
/// assert!(set.capacity() >= 32);
/// ```
#[inline]
pub fn xx_hash_set<K>(capacity: usize) -> XxHashSet<K> {
    XxHashSet::with_capacity_and_hasher(capacity, BuildHasherDefault::default())
}

/// 创建空 XxHashSet（无预分配）
///
/// 同 [`xx_hash_map_new`] 的 HashSet 版本。
#[inline]
pub fn xx_hash_set_new<K>() -> XxHashSet<K> {
    XxHashSet::with_hasher(BuildHasherDefault::default())
}

/// Compute xxHash3 64-bit hash of a byte slice.
///
/// Use this for cache keys (e.g., source bytecode caching) where collision resistance
/// matters more than std SipHash's DoS resistance. xxHash3 has better distribution
/// than FNV-1a and is significantly faster than SipHash.
///
/// # Known Vector
///
/// `xxh3_64(b"") == 0x2d06800538d394c2` (canonical xxh3 empty-input hash).
///
/// # Example
///
/// ```rust,ignore
/// use nuzo_core::hash::xxh3_64;
///
/// assert_eq!(xxh3_64(b""), 0x2d06800538d394c2);
/// assert_eq!(xxh3_64(b"hello"), xxh3_64(b"hello"));
/// ```
#[inline]
pub fn xxh3_64(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

// ══════════════════════════════════════════════════════════════════
//                              测试套件
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Happy Path：基本 CRUD 操作 ────────────────────────────────

    #[test]
    fn test_basic_crud() {
        let mut map: XxHashMap<i32, String> = xx_hash_map_new();

        // Create + Read
        map.insert(1, "one".to_string());
        map.insert(2, "two".to_string());
        assert_eq!(map.get(&1).unwrap(), "one");
        assert_eq!(map.len(), 2);

        // Update
        map.insert(1, "uno".to_string());
        assert_eq!(map.get(&1).unwrap(), "uno");
        assert_eq!(map.len(), 2); // 覆盖不增加长度

        // Delete
        map.remove(&1);
        assert_eq!(map.get(&1), None);
        assert_eq!(map.len(), 1);
    }

    // ── Edge Case：容量预分配 ─────────────────────────────────────

    #[test]
    fn test_with_capacity() {
        let map: XxHashMap<i32, i32> = xx_hash_map(100);
        assert!(map.capacity() >= 100, "预分配容量不足：expected >= 100, got {}", map.capacity());

        let set: XxHashSet<i32> = xx_hash_set(50);
        assert!(set.capacity() >= 50, "预分配容量不足：expected >= 50, got {}", set.capacity());
    }

    // ── Edge Case：空集合 / 零容量 ────────────────────────────────

    #[test]
    fn test_empty_collections() {
        let map: XxHashMap<String, i32> = xx_hash_map_new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.get("nonexistent"), None);

        let set: XxHashSet<i32> = xx_hash_set_new();
        assert!(set.is_empty());
        assert!(!set.contains(&42));
    }

    // ── Happy Path：字符串键（VM 最常用场景）────────────────────

    #[test]
    fn test_string_keys() {
        let mut map: XxHashMap<String, u32> = xx_hash_map_new();
        map.insert("hello".to_string(), 1);
        map.insert("world".to_string(), 2);

        assert_eq!(*map.get("hello").unwrap(), 1);
        assert_eq!(*map.get("world").unwrap(), 2);
        assert!(map.contains_key("hello"));
        assert!(!map.contains_key("missing"));
    }

    // ── Happy Path：迭代与聚合 ────────────────────────────────────

    #[test]
    fn test_iteration() {
        let mut map: XxHashMap<i32, i32> = xx_hash_map_new();
        for i in 0..10 {
            map.insert(i, i * 2);
        }

        // 验证所有值都正确存储
        for (&k, &v) in &map {
            assert_eq!(v, k * 2, "键 {} 的值应为 {}，实际为 {}", k, k * 2, v);
        }

        // 聚合验证：0+2+4+...+18 = 90
        let sum: i32 = map.values().sum();
        assert_eq!(sum, 90);
    }

    // ── Poison Pill：碰撞处理（大量相同 hash 前缀的键）──────────

    #[test]
    fn test_collision_resistance() {
        let mut map: XxHashMap<String, i32> = xx_hash_map_new();

        // 插入大量相似字符串，测试 hash 分布
        for i in 0..1000 {
            map.insert(format!("key_{}", i), i);
        }

        assert_eq!(map.len(), 1000, "碰撞导致数据丢失");

        // 验证随机访问正确性
        for i in 0..1000 {
            assert_eq!(map.get(&format!("key_{}", i)).unwrap(), &i, "键 key_{} 查找失败", i);
        }
    }

    // ── Edge Case：边界容量值 ─────────────────────────────────────
    #[test]
    fn test_boundary_capacities() {
        // 容量 = 0（合法但无意义）
        let _map: XxHashMap<i32, i32> = xx_hash_map(0);

        // 容量 = 1（最小有效值）
        let map: XxHashMap<i32, i32> = xx_hash_map(1);
        assert!(map.capacity() >= 1);

        // 大容量预分配
        let map: XxHashMap<i32, i32> = xx_hash_map(65536);
        assert!(map.capacity() >= 65536);
    }

    // ── xxh3_64：已知向量与稳定性 ─────────────────────────────────

    #[test]
    fn test_xxh3_64_empty_input_known_vector() {
        // xxh3 64-bit hash of empty input is a fixed canonical value.
        // This guards against accidental algorithm switch (e.g., back to FNV-1a).
        assert_eq!(xxh3_64(b""), 0x2d06800538d394c2);
    }

    #[test]
    fn test_xxh3_64_stability() {
        // Same input must produce same output across calls (cache key invariant).
        let h1 = xxh3_64(b"hello");
        let h2 = xxh3_64(b"hello");
        assert_eq!(h1, h2);

        // Different inputs must produce different outputs (basic distribution check).
        let h_short = xxh3_64(b"print");
        let h_long = xxh3_64(b"fn main() { print(42); }");
        assert_ne!(h1, h_short);
        assert_ne!(h_short, h_long);
        assert_ne!(h1, h_long);

        // Empty vs non-empty must differ.
        assert_ne!(xxh3_64(b""), xxh3_64(b"a"));
    }

    #[test]
    fn test_xxh3_64_longer_source_snippet() {
        // Verify a longer source-like input has a stable, non-zero hash.
        let source = b"fn factorial(n) { if n <= 1 { return 1; } return n * factorial(n - 1); }";
        let h = xxh3_64(source);
        assert_ne!(h, 0);
        assert_eq!(h, xxh3_64(source));
    }
}

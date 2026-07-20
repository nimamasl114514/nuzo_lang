//! # RuntimeContext -- 封装所有全局池的多实例隔离容器
//!
//! [`RuntimeContext`] 提供一个自包含的容器，用于管理：
//! - **字符串池 (String Pool)**: 驻留字符串，支持去重
//! - **堆对象池 (Heap Object Pool)**: 数组、字典、范围、闭包、内建函数
//! - **Box 池 (Box Pool)**: 可变闭包捕获存储
//!
//! ## 设计动机
//!
//! 此设计支持以下场景：
//! 1. **多 VM 实例隔离**：例如沙盒执行或并行测试，每个实例拥有独立的全局状态
//! 2. **优雅 teardown**：销毁 context 即可清理所有关联资源，无全局状态污染
//! 3. **未来迁移路径**：为从全局静态池迁移到 per-context 管理奠定基础
//!
//! ## 与默认全局池的关系
//!
//! `RuntimeContext` 是**可选的替代方案**。默认情况下，
//! [`Value`] 使用 `value.rs` 中的全局静态 `STRING_POOL` 和 `HEAP_POOL`。
//! 当需要多实例隔离时，可使用 `RuntimeContext` 替代。

use nuzo_core::XxHashMap;
use std::hash::BuildHasherDefault;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use super::errors::InternalError;
use super::errors::NuzoError;
#[cfg(test)]
use super::errors::NuzoErrorKind;
use super::heap::HeapObject;
use super::value::Value;

/// 运行时上下文，封装所有全局池。
///
/// 每个实例维护独立的字符串池、堆对象池和 Box 池，
/// 使得不同执行上下文之间完全隔离。
pub struct RuntimeContext {
    /// 驻留字符串存储：索引 -> Arc<String>
    strings: RwLock<XxHashMap<u32, Arc<String>>>,
    /// 反向查找表：字符串内容 -> 索引（O(1) 去重查找，替代旧版线性扫描）
    /// 与 `strings` 在同一锁下增删，保持一致。
    string_reverse: RwLock<XxHashMap<String, u32>>,
    /// 单调递增计数器，用于分配唯一的字符串索引
    string_counter: AtomicU64,
    /// 堆分配对象：索引 -> Arc<HeapObject>
    heap: RwLock<Vec<Arc<HeapObject>>>,

    /// 可变 Box 池，用于闭包捕获：索引 -> Value
    boxes: RwLock<Vec<Value>>,
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Lock Helpers -- 统一封装 RwLock poisoned 处理
// ============================================================================
//
// RwLock poisoned 表示持有锁的线程 panic，属于不可恢复的程序状态错误
// （数据可能不一致）。提供两种处理策略：
// - `lock_write`：返回 Result，用于已返回 Result 的写方法（错误传播）
// - `lock_read_or_panic`/`lock_write_or_panic`：poisoned 时 panic，用于不返回
//   Result 的方法（panic 是 Rust 标准库推荐的 poisoned 处理方式）

/// 获取写锁，poisoned 时返回 LockPoisoned 错误（用于返回 Result 的方法）
fn lock_write<T>(lock: &RwLock<T>) -> Result<std::sync::RwLockWriteGuard<'_, T>, NuzoError> {
    lock.write().map_err(|_| NuzoError::internal(InternalError::LockPoisoned, None))
}

/// 获取读锁，poisoned 时返回 LockPoisoned 错误
fn lock_read<T>(lock: &RwLock<T>) -> Result<std::sync::RwLockReadGuard<'_, T>, NuzoError> {
    lock.read().map_err(|_| NuzoError::internal(InternalError::LockPoisoned, None))
}

/// 获取写锁，poisoned 时返回 LockPoisoned 错误
fn lock_write_or_panic<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|_| panic!("RwLock poisoned (unrecoverable program state)"))
}

/// 获取读锁，poisoned 时 panic（用于不返回 Result 的方法）
fn lock_read_or_panic<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|_| panic!("RwLock poisoned (unrecoverable program state)"))
}

impl RuntimeContext {
    /// Create a new empty RuntimeContext with all pools initialized.
    pub fn new() -> Self {
        RuntimeContext {
            strings: RwLock::new(XxHashMap::with_hasher(BuildHasherDefault::default())),
            string_reverse: RwLock::new(XxHashMap::with_hasher(BuildHasherDefault::default())),
            string_counter: AtomicU64::new(0),
            heap: RwLock::new(Vec::new()),
            boxes: RwLock::new(Vec::new()),
        }
    }

    // ========================================================================
    // String Pool Operations
    // ========================================================================

    /// Intern a string, returning its unique index.
    ///
    /// If the string has been interned before, returns the existing index.
    /// Otherwise, assigns a new index and stores the string.
    ///
    /// 性能：通过反向 lookup map 实现 O(1) 平均去重查找，
    /// 替代旧版 O(n) 线性扫描（n 为已驻留字符串数）。
    pub fn intern_string(&mut self, s: &str) -> u32 {
        // 先查反向表（命中即返回）
        {
            let reverse = lock_read_or_panic(&self.string_reverse);
            if let Some(&idx) = reverse.get(s) {
                return idx;
            }
        }
        // 未命中：拿双写锁并再次确认（防止并发插入相同字符串）
        let mut strings = lock_write_or_panic(&self.strings);
        let mut reverse = lock_write_or_panic(&self.string_reverse);
        // Double-check：可能在抢锁期间已被其他线程插入
        if let Some(&idx) = reverse.get(s) {
            return idx;
        }
        let idx = self.string_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u32;
        let arc_str = Arc::new(s.to_string());
        reverse.insert(s.to_string(), idx);
        strings.insert(idx, arc_str);
        idx
    }

    /// Look up an interned string by index.
    ///
    /// Returns `None` if the index is not valid.
    pub fn get_string(&self, idx: u32) -> Result<Option<Arc<String>>, NuzoError> {
        let strings = lock_read(&self.strings)?;
        Ok(strings.get(&idx).map(Arc::clone))
    }

    /// Get the number of interned strings.
    pub fn string_count(&self) -> Result<usize, NuzoError> {
        Ok(lock_read(&self.strings)?.len())
    }

    // ========================================================================
    // Heap Object Pool Operations
    // ========================================================================

    /// Allocate a heap object, returning its unique index.
    pub fn alloc_heap(&mut self, obj: HeapObject) -> u64 {
        let idx = {
            let mut heap = lock_write_or_panic(&self.heap);
            let idx = heap.len();
            heap.push(Arc::new(obj));
            idx
        };
        idx as u64
    }

    /// Look up a heap object by index.
    ///
    /// Returns `None` if the index is out of bounds.
    pub fn heap(&self, idx: u64) -> Result<Option<Arc<HeapObject>>, NuzoError> {
        let heap = lock_read(&self.heap)?;
        Ok(heap.get(idx as usize).map(Arc::clone))
    }

    /// Get the number of allocated heap objects.
    pub fn heap_count(&self) -> Result<usize, NuzoError> {
        Ok(lock_read(&self.heap)?.len())
    }

    // ========================================================================
    // Box Pool Operations
    // ========================================================================

    /// Allocate a box in the mutable capture pool, returning its index.
    pub fn alloc_box(&mut self, val: Value) -> usize {
        {
            let mut boxes = lock_write_or_panic(&self.boxes);
            let idx = boxes.len();
            boxes.push(val);
            idx
        }
    }

    /// Read a box's value by index (returns a copy).
    ///
    /// Returns `None` if the index is out of bounds.
    pub fn get_box(&self, idx: usize) -> Result<Option<Value>, NuzoError> {
        let boxes = lock_read(&self.boxes)?;
        Ok(boxes.get(idx).copied())
    }

    /// Update a box's value by index.
    ///
    /// Returns an error if the index is out of bounds.
    pub fn set_box(&mut self, idx: usize, val: Value) -> Result<(), NuzoError> {
        let mut boxes = lock_write(&self.boxes)?;
        if let Some(slot) = boxes.get_mut(idx) {
            *slot = val;
            Ok(())
        } else {
            Err(NuzoError::index_out_of_bounds(idx.to_string(), boxes.len().to_string()))
        }
    }

    /// Get the number of allocated boxes.
    pub fn box_count(&self) -> Result<usize, NuzoError> {
        Ok(lock_read(&self.boxes)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_context_is_empty() {
        let ctx = RuntimeContext::new();
        assert_eq!(ctx.string_count().unwrap(), 0);
        assert_eq!(ctx.heap_count().unwrap(), 0);
        assert_eq!(ctx.box_count().unwrap(), 0);
    }

    #[test]
    fn test_string_intern_dedup() {
        let mut ctx = RuntimeContext::new();
        let idx_a = ctx.intern_string("hello");
        let idx_b = ctx.intern_string("hello");
        let idx_c = ctx.intern_string("world");

        assert_eq!(idx_a, idx_b, "same string should return same index");
        assert_ne!(idx_a, idx_c, "different strings should have different indices");

        let retrieved = ctx.get_string(idx_a).unwrap().unwrap();
        assert_eq!(retrieved.as_str(), "hello");
    }

    #[test]
    fn test_heap_alloc_retrieve() {
        let mut ctx = RuntimeContext::new();
        let idx = ctx
            .alloc_heap(HeapObject::Array(vec![Value::from_number(1.0), Value::from_number(2.0)]));

        let obj = ctx.heap(idx).unwrap().unwrap();
        match obj.as_ref() {
            HeapObject::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0].as_number(), 1.0);
            }
            other => panic!("expected Array, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn test_box_alloc_set_get() {
        let mut ctx = RuntimeContext::new();
        let idx = ctx.alloc_box(Value::from_number(42.0));

        assert_eq!(ctx.get_box(idx).unwrap(), Some(Value::from_number(42.0)));

        ctx.set_box(idx, Value::from_number(99.0)).unwrap();
        assert_eq!(ctx.get_box(idx).unwrap(), Some(Value::from_number(99.0)));
    }

    #[test]
    fn test_box_out_of_bounds() {
        let mut ctx = RuntimeContext::new();
        let result = ctx.set_box(0, Value::from_number(1.0));
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::IndexOutOfBounds { .. } => {}
            other => panic!("expected IndexOutOfBounds, got {:?}", other),
        }
    }

    #[test]
    fn test_intern_string_empty() {
        let mut ctx = RuntimeContext::new();
        let idx = ctx.intern_string("");
        assert_eq!(ctx.get_string(idx).unwrap().unwrap().as_str(), "");
        assert_eq!(ctx.string_count().unwrap(), 1);
    }

    #[test]
    fn test_intern_string_multiple_increases_count() {
        let mut ctx = RuntimeContext::new();
        ctx.intern_string("a");
        ctx.intern_string("b");
        ctx.intern_string("c");
        assert_eq!(ctx.string_count().unwrap(), 3);
    }

    #[test]
    fn test_get_string_invalid_index() {
        let ctx = RuntimeContext::new();
        assert!(ctx.get_string(999).unwrap().is_none());
    }

    #[test]
    fn test_alloc_heap_multiple_count() {
        let mut ctx = RuntimeContext::new();
        ctx.alloc_heap(HeapObject::Array(vec![]));
        ctx.alloc_heap(HeapObject::Array(vec![]));
        ctx.alloc_heap(HeapObject::Box(Value::from_number(1.0)));
        assert_eq!(ctx.heap_count().unwrap(), 3);
    }

    #[test]
    fn test_heap_invalid_index() {
        let ctx = RuntimeContext::new();
        assert!(ctx.heap(999).unwrap().is_none());
    }

    #[test]
    fn test_alloc_box_multiple_count() {
        let mut ctx = RuntimeContext::new();
        ctx.alloc_box(Value::from_number(1.0));
        ctx.alloc_box(Value::from_number(2.0));
        assert_eq!(ctx.box_count().unwrap(), 2);
    }

    #[test]
    fn test_get_box_out_of_bounds() {
        let ctx = RuntimeContext::new();
        assert!(ctx.get_box(0).unwrap().is_none());
        assert!(ctx.get_box(999).unwrap().is_none());
    }

    #[test]
    fn test_alloc_heap_returns_distinct_indices() {
        let mut ctx = RuntimeContext::new();
        let idx1 = ctx.alloc_heap(HeapObject::Array(vec![]));
        let idx2 = ctx.alloc_heap(HeapObject::Array(vec![]));
        assert_ne!(idx1, idx2);
    }

    #[test]
    fn test_alloc_box_returns_distinct_indices() {
        let mut ctx = RuntimeContext::new();
        let idx1 = ctx.alloc_box(Value::from_number(1.0));
        let idx2 = ctx.alloc_box(Value::from_number(2.0));
        assert_ne!(idx1, idx2);
    }

    #[test]
    fn test_default_context_is_empty() {
        let ctx = RuntimeContext::default();
        assert_eq!(ctx.string_count().unwrap(), 0);
        assert_eq!(ctx.heap_count().unwrap(), 0);
        assert_eq!(ctx.box_count().unwrap(), 0);
    }

    /// 性能回归测试：intern_string 去重路径必须 O(1) 而非 O(n)。
    /// 通过大量字符串 + 重复 intern 同一字符串验证不退化。
    #[test]
    fn test_intern_string_dedup_via_reverse_lookup() {
        let mut ctx = RuntimeContext::new();
        // 填充 1000 个不同字符串
        for i in 0..1000u32 {
            let s = format!("str_{}", i);
            ctx.intern_string(&s);
        }
        // 重复 intern 第一个字符串：必须返回相同索引（O(1) 路径）
        let first_idx = ctx.intern_string("str_0");
        let first_idx_again = ctx.intern_string("str_0");
        assert_eq!(first_idx, first_idx_again, "dedup must return same index via reverse lookup");
        // 计数不应增加
        assert_eq!(ctx.string_count().unwrap(), 1000, "dedup must not increase string_count");
    }

    /// 回归测试：intern 不同字符串必须分配新索引。
    #[test]
    fn test_intern_string_distinct_alloc() {
        let mut ctx = RuntimeContext::new();
        let mut idxs = Vec::new();
        for i in 0..100u32 {
            let s = format!("unique_{}", i);
            idxs.push(ctx.intern_string(&s));
        }
        let unique: std::collections::HashSet<u32> = idxs.iter().copied().collect();
        assert_eq!(unique.len(), 100, "all indices must be distinct");
    }
}

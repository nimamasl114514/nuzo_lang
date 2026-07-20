//! TurboSlab 堆分配器 — 自研精简 Slab 分配器
//!
//! 替换原堆池，实现：
//! - Slab + 位图管理（标量 trailing_zeros，保留 SIMD 扩展点）
//! - Per-CPU 缓存（thread_id hash 到 8 桶）
//! - 跨核释放（无锁 CAS 远程释放链表）
//! - Grace period reclaim（手动触发，保护最近 64 次分配不被误回收）

pub mod cache;
pub mod cpu_cache;
pub mod remote_free;
pub mod slab;

use crate::heap::HeapObject;
use nuzo_core::XxHashMap;
use std::any::TypeId;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

pub use cache::TurboCache;
pub use cpu_cache::TurboCpuCache;
pub use remote_free::RemoteFreeNode;
pub use slab::{SizeClass, TurboSlab};

/// 回收触发间隔（保留用于手动周期性 reclaim 的参考值）。
///
/// 注意：`alloc` 不再自动按此间隔触发 reclaim，以避免并行测试环境下
/// 误回收其他线程刚分配的对象。需要回收时由调用方显式调用 `reclaim_orphaned`。
pub const RECLAIM_INTERVAL: u64 = 256;

/// Grace period 窗口大小：最近这么多次分配的对象受保护，不会被 reclaim 回收。
pub const RECLAIM_GRACE_SIZE: usize = 64;

/// 统计信息
pub struct HeapStats {
    pub total_slots: usize,
    pub occupied_slots: usize,
    pub free_slots: usize,
    pub alloc_count: u64,
    pub reclaim_count: u64,
    pub slab_count: usize,
}

/// 公开入口（保持与原堆池相同 API）
pub struct TurboSlabAllocator {
    caches: XxHashMap<TypeId, TurboCache>,
    #[allow(dead_code)] // T7 接入 TypeId 分桶后使用
    type_index: RwLock<XxHashMap<TypeId, u32>>,
    /// 新路径分配计数（用于统计，不再自动触发 reclaim）。
    alloc_count: AtomicU64,
    /// 最近分配的扁平索引队列（FIFO），grace period 内保护不被 reclaim 误回收。
    recent_allocs: VecDeque<u32>,
}

impl Default for TurboSlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl TurboSlabAllocator {
    pub fn new() -> Self {
        Self {
            caches: XxHashMap::default(),
            type_index: RwLock::new(XxHashMap::default()),
            alloc_count: AtomicU64::new(0),
            recent_allocs: VecDeque::new(),
        }
    }

    /// 在 TurboCache slab 中分配对象，返回全局扁平索引。
    ///
    /// T6 采用单默认 bucket：所有 HeapObject 使用同一个 TypeId 缓存，因此 flat_idx
    /// 可直接作为全局索引返回。T7 再考虑 TypeId 分桶后的全局索引映射。
    ///
    /// # 返回值
    /// - `Some(flat_idx)`：分配成功，`flat_idx` 为全局扁平索引
    /// - `None`：分配失败。可能原因：
    ///   - slab 内部位图扫描无空闲位（slab 已满且无法增长）
    ///   - 系统内存分配失败（OOM）
    ///
    /// # 设计说明
    /// 旧版（P1 BUG-turboslab-alloc-panic）使用 `.expect()` 在分配失败时直接 panic，
    /// 违反"禁止生产路径 panic"规则。改为返回 `Option<u32>` 后，调用方可在 `None`
    /// 时执行 reclaim 后重试，或向上传播错误。
    ///
    /// 注意：`alloc_count` 在调用入口即递增（即使后续分配失败），用于统计真实
    /// 分配尝试次数；这与 grace period 保护逻辑无冲突（recent_allocs 仅在成功时追加）。
    pub fn alloc(&mut self, obj: HeapObject) -> Option<u32> {
        let type_id = TypeId::of::<HeapObject>();

        // 仅统计分配次数，不再自动触发 reclaim。
        // 原周期触发机制会在并行环境下误回收其他线程刚分配的对象（strong_count==1），
        // 改为 grace period + 手动显式回收。
        self.alloc_count.fetch_add(1, Ordering::Relaxed);

        let cache = self.caches.entry(type_id).or_insert_with(|| {
            let size_class = SizeClass::new(std::mem::size_of::<Arc<HeapObject>>(), 256);
            TurboCache::new(size_class)
        });

        // P1 修复：移除 .expect() panic，将 None 透传给调用方
        let flat_idx = cache.alloc(obj)?;

        // 记录最近分配的索引，grace period 内保护不被 reclaim 误回收。
        self.recent_allocs.push_back(flat_idx);
        if self.recent_allocs.len() > RECLAIM_GRACE_SIZE {
            self.recent_allocs.pop_front();
        }

        Some(flat_idx)
    }

    /// 返回指定索引对应的 `HeapObject` 只读指针。
    ///
    /// # 边界检查
    /// 内部委托给 [`TurboCache::get_object_ptr`]，后者执行：
    /// 1. `slab_index = idx / objects_per_slab` 越界检查（`all_slabs.get(slab_index)`）
    /// 2. `in_slab_idx` 占用状态检查（`slab.is_occupied(in_slab_idx)`）
    ///
    /// 任一检查失败返回空指针，不会触发 UB。
    ///
    /// # 对齐保证
    /// 返回的指针来自 `Arc::as_ptr(&arc)`，`Arc<HeapObject>` 的数据布局保证
    /// 指针与 `HeapObject` 对齐要求一致（Rust 的 `Arc` 通过 `alloc::Layout`
    /// 强制对齐），不会产生未对齐引用。
    ///
    /// # Safety（调用方约束）
    /// 返回的 `*const HeapObject` 仅在槽位保持占用期间有效。调用方不得：
    /// - 在 `reclaim_orphaned` / `free` 后继续使用该指针
    /// - 通过该指针构造 `&mut HeapObject`（应使用 [`get_mut`](Self::get_mut)）
    pub fn get(&self, idx: u32) -> *const HeapObject {
        let type_id = TypeId::of::<HeapObject>();
        if let Some(cache) = self.caches.get(&type_id)
            && let Some(arc_ptr) = cache.get_object_ptr(idx)
        {
            // SAFETY: arc_ptr 来自 cache.get_object_ptr，已通过边界与占用检查；
            // &*arc_ptr 借用 Arc 内部数据，Arc::as_ptr 返回指向 HeapObject 的
            // 对齐指针，生命周期与 slab 槽位占用状态绑定。
            return unsafe { Arc::as_ptr(&*arc_ptr) };
        }
        std::ptr::null()
    }

    /// 返回可变指针，但仅在对应 Arc 的 `strong_count == 1` 时才非空，
    /// 避免在共享引用下 unsafe 修改堆对象内容。
    ///
    /// # 边界检查
    /// 与 [`get`](Self::get) 相同的双重边界检查（slab 索引 + 占用状态）。
    /// 此外通过 `Arc::strong_count == 1` 检查确保独占访问，避免数据竞争。
    ///
    /// # 对齐保证
    /// 与 [`get`](Self::get) 相同，通过 `Arc::as_ptr` 保证指针对齐。
    pub fn get_mut(&self, idx: u32) -> *mut HeapObject {
        let type_id = TypeId::of::<HeapObject>();
        if let Some(cache) = self.caches.get(&type_id)
            && let Some(arc_ptr) = cache.get_object_ptr(idx)
        {
            // SAFETY: arc_ptr 已通过边界与占用检查。strong_count == 1 保证
            // 独占访问，Arc::as_ptr 返回对齐指针；转换为 *mut HeapObject 后
            // 调用方需确保不再通过其他路径访问该 Arc（由 strong_count 不变量保证）。
            unsafe {
                let arc_ref = &*arc_ptr;
                if Arc::strong_count(arc_ref) == 1 {
                    return Arc::as_ptr(arc_ref) as *mut HeapObject;
                }
            }
        }
        std::ptr::null_mut()
    }

    /// 返回指定索引对应的 `Arc<HeapObject>` 克隆（若槽位仍被占用）。
    pub fn get_arc(&self, idx: u32) -> Option<Arc<HeapObject>> {
        let type_id = TypeId::of::<HeapObject>();
        if let Some(cache) = self.caches.get(&type_id)
            && let Some(arc_ptr) = cache.get_object_ptr(idx)
        {
            return unsafe { Some(Arc::clone(&*arc_ptr)) };
        }
        None
    }

    /// 回收孤立条目：遍历每个 TurboCache，回收其中 strong_count==1 的 slab 槽位。
    ///
    /// 最近 `RECLAIM_GRACE_SIZE` 次分配的对象受 grace period 保护，不会被回收，
    /// 避免并行环境下误回收其他线程刚分配的对象。返回回收数量。
    pub fn reclaim_orphaned(&mut self) -> usize {
        let exclusions: HashSet<u32> = self.recent_allocs.iter().copied().collect();
        let mut reclaimed = 0;
        for cache in self.caches.values_mut() {
            reclaimed += cache.reclaim_orphaned_with_exclusions(&exclusions);
        }
        // 清理 recent_allocs 中已被回收的索引，避免 exclusions HashSet 膨胀
        if reclaimed > 0 {
            let alive: Vec<u32> = self
                .recent_allocs
                .iter()
                .filter(|&&idx| !self.get(idx).is_null())
                .copied()
                .collect();
            self.recent_allocs = alive.into();
        }
        reclaimed
    }

    /// 返回累计分配次数。
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> HeapStats {
        let mut total_slots = 0;
        let mut occupied_slots = 0;
        let mut free_slots = 0;
        let mut alloc_count = 0;
        let mut reclaim_count = 0;
        let mut slab_count = 0;
        for cache in self.caches.values() {
            let s = cache.stats();
            total_slots += s.total_slots;
            occupied_slots += s.occupied_slots;
            free_slots += s.free_slots;
            alloc_count += s.alloc_count;
            reclaim_count += s.reclaim_count;
            slab_count += s.slab_count;
        }
        HeapStats {
            total_slots,
            occupied_slots,
            free_slots,
            alloc_count,
            reclaim_count,
            slab_count,
        }
    }
}

// ============================================================================
// ArrayAllocCache — 数组分配缓存（T-4 大数组投机合并优化）
// ============================================================================

/// 数组分配缓存 — 按 capacity 分桶缓存 `Vec<HeapObject>`，减少重复 buffer 分配。
///
/// # 设计背景
/// TurboSlab 路径用 `Arc<HeapObject>` 引用计数管理生命周期（与 Gc 的 mark-sweep
/// 独立）。当数组分配频繁时，每次构造 `Vec<HeapObject>` 的 buffer 分配/释放成为
/// 瓶颈。本缓存按 capacity 分桶复用已分配的 Vec buffer。
///
/// # 复用判定
/// TurboSlab 路径的对象生命周期由 `Arc::strong_count` 决定：当 `strong_count == 1`
/// （仅缓存/分配器持有）时，对象可安全回收并复用其 buffer。调用方应在确认
/// `strong_count == 1` 后再调用 [`return_back`](Self::return_back) 归还 Vec。
///
/// # 版本号
/// [`version`](Self::version) 在每个分配周期通过
/// [`bump_version`](Self::bump_version) 递增，用于检测过期缓存条目（stale
/// detection）：版本号回绕（u64 溢出归零）时清空缓存，避免潜在的过期条目。
///
/// # 本次范围
/// 仅提供独立实现与单元测试，**不接入** `default_heap_alloc`，避免触发其他测试回归。
pub struct ArrayAllocCache {
    /// 按 capacity 分桶的缓存条目。每条 `(capacity, Vec<HeapObject>)`。
    /// Vec 在归还时已清空（`is_empty()`），仅保留 capacity 供复用。
    cache: Vec<(usize, Vec<HeapObject>)>,
    /// 分配版本号，每次 [`bump_version`](Self::bump_version) 递增。
    version: u64,
}

impl ArrayAllocCache {
    /// 创建空缓存。
    pub fn new() -> Self {
        Self { cache: Vec::new(), version: 0 }
    }

    /// 尝试取出一个 `capacity >= needed` 的 Vec。
    ///
    /// 查找最近的（局部性好）capacity 足够的桶，取出并返回。若无可匹配桶返回 `None`。
    /// 返回的 Vec 已清空（`is_empty()`），仅保留 capacity。
    ///
    /// # 复用安全
    /// 调用方应确保取出的 Vec 中的 HeapObject 已满足 `Arc::strong_count == 1`
    /// （由 [`return_back`](Self::return_back) 归还前保证）。
    #[inline]
    pub fn try_get(&mut self, needed: usize) -> Option<Vec<HeapObject>> {
        let pos = self.cache.iter().rposition(|(cap, _)| *cap >= needed)?;
        let (_, vec) = self.cache.swap_remove(pos);
        Some(vec)
    }

    /// 将 Vec 归还到缓存，供后续 [`try_get`](Self::try_get) 复用其 buffer。
    ///
    /// 归还前清空内容（保留 capacity）。调用方应确保 Vec 中的 HeapObject 已可安全
    /// 回收（`Arc::strong_count == 1`）。
    ///
    /// # 防膨胀
    /// 缓存条目数上限 64，超出时丢弃。
    #[inline]
    pub fn return_back(&mut self, mut vec: Vec<HeapObject>) {
        const CACHE_CAP: usize = 64;
        if self.cache.len() >= CACHE_CAP {
            return;
        }
        let cap = vec.capacity();
        vec.clear();
        self.cache.push((cap, vec));
    }

    /// 递增分配版本号，用于失效过期缓存条目。
    ///
    /// 版本号回绕（u64 溢出归零）时清空缓存，避免潜在的过期条目。
    #[inline]
    pub fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        if self.version == 0 {
            self.cache.clear();
        }
    }

    /// 当前缓存条目数。
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// 当前版本号。
    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Default for ArrayAllocCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;
    use nuzo_core::NIL;

    fn dummy_obj() -> HeapObject {
        HeapObject::Box(NIL)
    }

    // ── ArrayAllocCache 单元测试（T-4）──────────────────────────────────

    #[test]
    fn test_array_cache_try_get_empty() {
        let mut cache = ArrayAllocCache::new();
        assert!(cache.is_empty());
        assert!(cache.try_get(10).is_none(), "empty cache must return None");
    }

    #[test]
    fn test_array_cache_return_back_and_try_get() {
        let mut cache = ArrayAllocCache::new();
        let vec: Vec<HeapObject> = (0..5).map(|_| dummy_obj()).collect();
        let cap = vec.capacity();
        cache.return_back(vec);
        assert_eq!(cache.len(), 1, "one entry after return_back");
        let got = cache.try_get(5).expect("should get vec with enough capacity");
        assert!(got.is_empty(), "returned vec must be cleared");
        assert!(got.capacity() >= 5, "capacity must satisfy request");
        assert_eq!(got.capacity(), cap, "capacity must be preserved");
        assert!(cache.is_empty(), "cache drained after try_get");
    }

    #[test]
    fn test_array_cache_try_get_smaller_capacity_returns_none() {
        let mut cache = ArrayAllocCache::new();
        let vec: Vec<HeapObject> = vec![dummy_obj(); 3];
        cache.return_back(vec);
        // 需要 100，但缓存只有 capacity ~3
        assert!(cache.try_get(100).is_none(), "insufficient capacity must return None");
    }

    #[test]
    fn test_array_cache_try_get_exact_or_larger_capacity() {
        let mut cache = ArrayAllocCache::new();
        let big: Vec<HeapObject> = Vec::with_capacity(200);
        cache.return_back(big);
        let got = cache.try_get(100).expect("capacity 200 >= 100");
        assert!(got.capacity() >= 100);
        // 同时验证请求恰好等于容量边界
        let exact: Vec<HeapObject> = Vec::with_capacity(50);
        cache.return_back(exact);
        let got2 = cache.try_get(50).expect("capacity 50 == 50");
        assert!(got2.capacity() >= 50);
    }

    #[test]
    fn test_array_cache_bump_version_increments() {
        let mut cache = ArrayAllocCache::new();
        assert_eq!(cache.version(), 0);
        cache.bump_version();
        assert_eq!(cache.version(), 1);
        cache.bump_version();
        assert_eq!(cache.version(), 2);
    }

    #[test]
    fn test_array_cache_bump_version_wrap_clears_cache() {
        let mut cache = ArrayAllocCache::new();
        cache.version = u64::MAX;
        cache.return_back(vec![dummy_obj(); 2]);
        assert_eq!(cache.len(), 1);
        cache.bump_version(); // wraps to 0
        assert_eq!(cache.version(), 0);
        assert!(cache.is_empty(), "cache should be cleared on version wrap");
    }

    #[test]
    fn test_array_cache_return_back_preserves_capacity() {
        let mut cache = ArrayAllocCache::new();
        let mut vec: Vec<HeapObject> = Vec::with_capacity(150);
        for _ in 0..100 {
            vec.push(dummy_obj());
        }
        let cap = vec.capacity();
        cache.return_back(vec);
        let got = cache.try_get(100).unwrap();
        assert_eq!(got.capacity(), cap, "capacity must be preserved");
        assert!(got.is_empty(), "content must be cleared");
    }

    #[test]
    fn test_array_cache_return_back_cap_limit() {
        let mut cache = ArrayAllocCache::new();
        // 填满 64 条
        for _ in 0..64 {
            cache.return_back(vec![dummy_obj(); 1]);
        }
        assert_eq!(cache.len(), 64);
        // 第 65 条应被丢弃
        cache.return_back(vec![dummy_obj(); 1]);
        assert_eq!(cache.len(), 64, "cache should be capped at 64");
    }

    #[test]
    fn test_array_cache_strong_count_reuse_semantics() {
        // 验证 TurboSlab 路径的复用判定语义：Arc strong_count == 1 时可安全复用。
        // ArrayAllocCache 本身不检查 strong_count（由调用方在 return_back 前确保），
        // 此处验证含 Arc 的 HeapObject 的 strong_count 行为符合预期。
        let arc_obj = std::sync::Arc::new(HeapObject::Box(NIL));
        assert_eq!(std::sync::Arc::strong_count(&arc_obj), 1, "sole owner: count==1");
        let cloned = std::sync::Arc::clone(&arc_obj);
        assert_eq!(std::sync::Arc::strong_count(&arc_obj), 2, "shared: count==2");
        drop(cloned);
        assert_eq!(std::sync::Arc::strong_count(&arc_obj), 1, "after drop: count==1");
    }

    #[test]
    fn test_array_cache_default_impl() {
        let cache = ArrayAllocCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.version(), 0);
    }

    #[test]
    fn test_array_cache_multiple_buckets_lru_order() {
        // 验证 rposition 取最近归还的（局部性好）：依次归还小、大，请求中等应命中大桶
        let mut cache = ArrayAllocCache::new();
        cache.return_back(Vec::<HeapObject>::with_capacity(10));
        cache.return_back(Vec::<HeapObject>::with_capacity(500));
        let got = cache.try_get(100).expect("should hit capacity-500 bucket");
        assert!(got.capacity() >= 100);
        // 剩下 capacity-10 桶
        assert_eq!(cache.len(), 1);
        let rest = cache.try_get(5).expect("should hit capacity-10 bucket");
        assert!(rest.capacity() >= 5);
    }

    #[test]
    fn test_reclaim_orphaned() {
        let mut alloc = TurboSlabAllocator::new();
        // 填满 grace window，使第一个分配的对象超出保护范围
        let first_idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
        for _ in 0..RECLAIM_GRACE_SIZE {
            let _ = alloc.alloc(dummy_obj());
        }
        assert!(!alloc.get(first_idx).is_null(), "allocated object should be reachable");

        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 1, "orphan Arc beyond grace window should be reclaimed");
        assert!(alloc.get(first_idx).is_null(), "reclaimed slot should return null");
    }

    #[test]
    fn test_reclaim_skips_shared() {
        let mut alloc = TurboSlabAllocator::new();
        let idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
        // 填满 grace window 使 idx 超出保护范围，验证靠 strong_count 保护
        for _ in 0..RECLAIM_GRACE_SIZE {
            let _ = alloc.alloc(dummy_obj());
        }

        // 通过内部 cache 拿到 Arc 并克隆，模拟外部引用。
        let type_id = TypeId::of::<HeapObject>();
        let cache = alloc.caches.get(&type_id).unwrap();
        let arc_ptr = cache.get_object_ptr(idx).unwrap();
        let arc_clone = unsafe { Arc::clone(&*arc_ptr) };
        assert_eq!(unsafe { Arc::strong_count(&*arc_ptr) }, 2);

        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 0, "shared Arc should not be reclaimed even beyond grace window");

        drop(arc_clone);
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 1, "Arc should be reclaimed after external drop");
        assert!(alloc.get(idx).is_null());
    }

    #[test]
    fn test_periodic_reclaim() {
        // 移除周期性自动 reclaim 后，alloc 不再在固定间隔触发回收。
        // 此测试验证：连续分配 RECLAIM_INTERVAL 次不会自动 reclaim，
        // 所有对象仍占用槽位（仅在 grace window 内的受保护）。
        let mut alloc = TurboSlabAllocator::new();
        for _ in 0..RECLAIM_INTERVAL {
            let _ = alloc.alloc(dummy_obj());
        }
        let stats = alloc.stats();
        assert_eq!(
            stats.reclaim_count, 0,
            "alloc should not auto-trigger reclaim after removing periodic trigger"
        );
        // 手动 reclaim：超出 grace window 的孤立对象会被回收
        let reclaimed = alloc.reclaim_orphaned();
        assert!(
            reclaimed > 0,
            "manual reclaim should free orphans beyond grace window: {}",
            reclaimed
        );
        // grace window 内的 64 个对象仍占用
        assert_eq!(
            alloc.stats().occupied_slots,
            RECLAIM_GRACE_SIZE,
            "only grace window objects should remain after manual reclaim"
        );
    }

    #[test]
    fn test_get_mut_unique_only() {
        let mut alloc = TurboSlabAllocator::new();
        let idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
        assert!(!alloc.get_mut(idx).is_null(), "unique orphan should allow mut pointer");

        let arc = alloc.get_arc(idx).unwrap();
        assert!(alloc.get_mut(idx).is_null(), "shared Arc should not allow mut pointer");
        drop(arc);

        // reclaim 后 Arc 被 drop，重新分配同一索引仍可能得到唯一 Arc
        alloc.reclaim_orphaned();
        let idx2 = alloc.alloc(dummy_obj()).expect("alloc should succeed after reclaim");
        assert!(!alloc.get_mut(idx2).is_null());
    }

    #[test]
    fn test_get_arc() {
        let mut alloc = TurboSlabAllocator::new();
        let idx = alloc
            .alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(42.0)]))
            .expect("alloc should succeed");
        let arc = alloc.get_arc(idx).unwrap();
        assert!(matches!(arc.as_ref(), HeapObject::Array(_)));
    }

    #[test]
    fn test_reclaim_orphaned_on_empty_pool() {
        let mut alloc = TurboSlabAllocator::new();
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 0, "empty pool should have 0 orphaned entries");
    }

    #[test]
    fn test_reclaim_orphaned_multiple() {
        let mut alloc = TurboSlabAllocator::new();
        // 分配 RECLAIM_GRACE_SIZE + 3 个对象，前 3 个超出 grace window 可被回收
        let idxs: Vec<u32> = (0..(RECLAIM_GRACE_SIZE + 3))
            .map(|_| alloc.alloc(dummy_obj()).expect("alloc should succeed"))
            .collect();
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 3, "should reclaim 3 orphans beyond grace window");
        for &idx in &idxs[..3] {
            assert!(alloc.get(idx).is_null(), "reclaimed slot should return null");
        }
    }

    #[test]
    fn test_reclaim_preserves_referenced_objects() {
        let mut alloc = TurboSlabAllocator::new();
        let orphan_idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
        let shared_idx = alloc
            .alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(42.0)]))
            .expect("alloc should succeed");
        let arc = alloc.get_arc(shared_idx).unwrap();
        // 填满 grace window 使 orphan_idx 和 shared_idx 都超出保护范围，
        // 验证 shared_idx 靠 strong_count==2 保护而非 grace period
        for _ in 0..RECLAIM_GRACE_SIZE {
            let _ = alloc.alloc(dummy_obj());
        }

        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(
            reclaimed, 1,
            "only orphaned entry should be reclaimed; shared Arc protected by strong_count"
        );
        assert!(!alloc.get(shared_idx).is_null(), "referenced slot should remain reachable");
        assert!(alloc.get(orphan_idx).is_null(), "orphaned slot should return null");

        drop(arc);
    }

    #[test]
    fn test_alloc_near_limit() {
        let mut alloc = TurboSlabAllocator::new();
        let limit = crate::constants::HEAP_POOL_INDEX_LIMIT;
        let mut refs = Vec::new();
        for i in 0..limit {
            let idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
            assert!(
                idx < limit as u32,
                "idx {} at iteration {} must stay below HEAP_POOL_INDEX_LIMIT ({})",
                idx,
                i,
                limit
            );
            // 持有引用，防止周期性 reclaim 复用槽位，从而迫使 slab 增长
            refs.push(alloc.get_arc(idx).unwrap());
        }
        let stats = alloc.stats();
        assert_eq!(stats.total_slots, limit, "should reach limit slots when references are held");
    }

    #[test]
    fn test_long_run_stable() {
        let mut alloc = TurboSlabAllocator::new();
        for i in 0..1000 {
            let _ = alloc.alloc(dummy_obj());
            // 移除自动周期 reclaim 后，手动定期触发以保持槽位可复用
            if (i + 1) % RECLAIM_INTERVAL == 0 {
                alloc.reclaim_orphaned();
            }
        }
        // 最终回收一次，清理超出 grace window 的孤立对象
        alloc.reclaim_orphaned();
        let stats = alloc.stats();
        // 手动 reclaim 每 256 次触发一次，slab 数应保持稳定
        assert!(
            stats.slab_count <= 4,
            "slab count should remain bounded under long-running alloc/drop: {}",
            stats.slab_count
        );
        // grace window 内的 64 个对象仍占用，其余已回收
        assert!(
            stats.occupied_slots <= RECLAIM_GRACE_SIZE,
            "occupied slots should be bounded by grace window: {}",
            stats.occupied_slots
        );
    }

    /// 验证 grace period 保护最近分配的对象不被 reclaim 误回收。
    #[test]
    fn test_reclaim_skips_recent_allocs() {
        let mut alloc = TurboSlabAllocator::new();
        let idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");
        // 刚分配的对象在 grace window 内，reclaim 不应回收
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 0, "recent alloc should be protected by grace period");
        assert!(!alloc.get(idx).is_null(), "recent alloc should still be reachable");
    }

    /// 验证超出 grace window 的孤立对象会被回收，窗口内的受保护。
    #[test]
    fn test_reclaim_frees_old_allocs() {
        let mut alloc = TurboSlabAllocator::new();
        // 分配 RECLAIM_GRACE_SIZE + 10 个对象，前 10 个超出 grace window
        let idxs: Vec<u32> = (0..(RECLAIM_GRACE_SIZE + 10))
            .map(|_| alloc.alloc(dummy_obj()).expect("alloc should succeed"))
            .collect();
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 10, "first 10 allocs beyond grace window should be reclaimed");
        // 前 10 个被回收
        for &idx in &idxs[..10] {
            assert!(alloc.get(idx).is_null(), "old alloc should be reclaimed");
        }
        // 后 64 个在 grace window 内仍可访问
        for &idx in &idxs[10..] {
            assert!(!alloc.get(idx).is_null(), "recent alloc should remain reachable");
        }
    }

    /// 验证 grace period 的 FIFO 挤出语义。
    #[test]
    fn test_grace_period_fifo_eviction() {
        let mut alloc = TurboSlabAllocator::new();
        let idxs: Vec<u32> =
            (0..65).map(|_| alloc.alloc(dummy_obj()).expect("alloc should succeed")).collect();
        // recent_allocs 容量为 RECLAIM_GRACE_SIZE (64)，第 1 个被 FIFO 挤出
        assert_eq!(
            alloc.recent_allocs.len(),
            RECLAIM_GRACE_SIZE,
            "recent_allocs should be capped at RECLAIM_GRACE_SIZE"
        );
        assert_eq!(
            *alloc.recent_allocs.front().unwrap(),
            idxs[1],
            "front should be the 2nd alloc (1st evicted by FIFO)"
        );
        // 手动 reclaim：第 1 个被回收，第 2-65 个受保护
        let reclaimed = alloc.reclaim_orphaned();
        assert_eq!(reclaimed, 1, "only the FIFO-evicted alloc should be reclaimed");
        assert!(alloc.get(idxs[0]).is_null(), "evicted alloc should be reclaimed");
        for &idx in &idxs[1..] {
            assert!(!alloc.get(idx).is_null(), "grace window allocs should remain");
        }
    }

    // ── P1 回归测试 ───────────────────────────────────────────────────

    /// P1 #1 回归测试：验证 get / get_mut 对越界索引返回空指针而非 UB。
    ///
    /// 覆盖场景：
    /// - 索引远超已分配范围（slab_index 越界）
    /// - 索引在 slab 范围内但 in_slab_idx 未占用
    /// - get_mut 在共享 Arc（strong_count > 1）时返回 null
    /// - get_mut 在独占 Arc（strong_count == 1）时返回有效指针
    #[test]
    fn test_turboslab_bounded_access() {
        let mut alloc = TurboSlabAllocator::new();

        // 分配一个对象，获得有效索引
        let valid_idx = alloc.alloc(dummy_obj()).expect("alloc should succeed");

        // 1. 越界索引：远超已分配 slab 范围
        //    slab_index = u32::MAX / objects_per_slab 远超 all_slabs.len()
        let out_of_bounds_idx = u32::MAX;
        assert!(
            alloc.get(out_of_bounds_idx).is_null(),
            "get with out-of-bounds idx must return null, not UB"
        );
        assert!(
            alloc.get_mut(out_of_bounds_idx).is_null(),
            "get_mut with out-of-bounds idx must return null, not UB"
        );

        // 2. 索引在 slab 范围内但未占用：valid_idx + 1 应未分配
        //    （同一 slab 内的相邻槽位，bit 未置 0 表示占用）
        let unoccupied_idx = valid_idx + 1;
        assert!(alloc.get(unoccupied_idx).is_null(), "get on unoccupied slot must return null");
        assert!(
            alloc.get_mut(unoccupied_idx).is_null(),
            "get_mut on unoccupied slot must return null"
        );

        // 3. get_mut 在共享 Arc 下返回 null（strong_count > 1）
        let arc = alloc.get_arc(valid_idx).expect("get_arc should succeed for live idx");
        assert_eq!(
            std::sync::Arc::strong_count(&arc),
            2,
            "strong_count must be 2 after get_arc (slab + local)"
        );
        assert!(
            alloc.get_mut(valid_idx).is_null(),
            "get_mut on shared Arc (strong_count > 1) must return null"
        );

        // 4. get_mut 在独占 Arc 下返回有效指针（strong_count == 1）
        drop(arc);
        assert!(
            !alloc.get_mut(valid_idx).is_null(),
            "get_mut on unique Arc (strong_count == 1) must return non-null"
        );

        // 5. get 对有效索引返回非空指针
        assert!(!alloc.get(valid_idx).is_null(), "get on valid occupied idx must return non-null");
    }

    /// P1 #1 回归测试：验证 alloc 返回 Option<u32>（不再 panic）。
    ///
    /// 覆盖场景：
    /// - 正常路径返回 Some(idx)，idx < HEAP_POOL_INDEX_LIMIT
    /// - 返回的 idx 可通过 get 访问到刚分配的对象
    /// - 连续多次 alloc 返回不同的有效索引
    #[test]
    fn test_turboslab_alloc_returns_result() {
        let mut alloc = TurboSlabAllocator::new();
        let limit = crate::constants::HEAP_POOL_INDEX_LIMIT;

        // 1. 首次分配返回 Some(idx)
        let idx1 = alloc.alloc(dummy_obj()).expect("first alloc should return Some");
        assert!(
            idx1 < limit as u32,
            "alloc must return idx < HEAP_POOL_INDEX_LIMIT (got {})",
            idx1
        );

        // 2. idx 可通过 get 访问到对象
        let ptr1 = alloc.get(idx1);
        assert!(!ptr1.is_null(), "get on just-allocated idx must return non-null");

        // 3. 第二次分配返回不同的 idx
        let idx2 = alloc.alloc(dummy_obj()).expect("second alloc should return Some");
        assert_ne!(idx1, idx2, "consecutive allocs must return distinct indices");

        // 4. 两个 idx 都可访问
        assert!(!alloc.get(idx1).is_null(), "first idx must remain accessible");
        assert!(!alloc.get(idx2).is_null(), "second idx must be accessible");

        // 5. 多次连续分配都返回有效 Some
        let mut indices = Vec::new();
        for _ in 0..10 {
            let idx = alloc.alloc(dummy_obj()).expect("bulk alloc should return Some");
            assert!(!alloc.get(idx).is_null(), "each bulk-allocated idx must be accessible");
            indices.push(idx);
        }
        // 所有索引应互不相同
        let unique: HashSet<u32> = indices.iter().copied().collect();
        assert_eq!(unique.len(), 10, "all 10 bulk-allocated indices must be distinct");
    }
}

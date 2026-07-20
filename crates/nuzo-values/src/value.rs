//! # Value 类型系统 — 堆依赖扩展方法
//!
//! 从 v0.5.0 起，`Value`、`ValueTag`、`RangeValue` 定义已下沉到 `nuzo_core`。
//! 本模块通过 `pub use` 重导出以保持向后兼容，并通过 `ValueExt` trait
//! 提供依赖 HeapObject / RuntimeContext 的扩展方法。
//!
//! ## 模块结构
//!
//! | 层 | 位置 | 内容 |
//! |----|------|------|
//! | L1 (nuzo_core) | `nuzo_core::value` | Value struct + 纯位操作 (42 方法) |
//! | L2 (nuzo_values) | `nuzo_values::value::ValueExt` | 堆依赖扩展方法 (30 方法) |
//!
//! ## 线程本地缓存
//!
//! TL-μCache: 线程本地 8路直接映射缓存，旁路全局锁
//! TL-HeapCache: 堆对象访问缓存，读路径零锁零分配

pub use nuzo_core::value::{FALSE, NIL, RangeValue, TRUE, Value, ValueTag};

// Re-export sub-module types for backward compatibility and convenience
pub use crate::function::FunctionPrototype;
pub use crate::heap::{
    BuiltinFnPtr, CaptureInfo, CaptureMode, CapturedVar, HeapObject, HeapObjectOps, RangeEnd,
};
pub use crate::nuzo_dict::{NuzoDict, NuzoEntry, nuzo_mix};

use crate::constants::*;
use nuzo_core::XxHashMap;
use nuzo_core::error::NuzoError;
use std::cell::{Cell, RefCell};
use std::hash::BuildHasherDefault;
use std::sync::{Arc, RwLock};

// ============================================================================
// 线程本地微缓存 (TL-μCache)
// ============================================================================

// 字符串驻留微缓存 (8-way direct mapped)
thread_local! {
    #[allow(clippy::missing_const_for_thread_local)] // RefCell::new 非 const（含运行时初始化），无法使用 const
    static TL_STR_CACHE: RefCell<[(u64, u32); 8]> = RefCell::new([(0, 0); 8]);
}

// 堆对象访问微缓存 (4-way direct mapped)
thread_local! {
    static TL_HEAP_CACHE: RefCell<[(u32, Option<Arc<HeapObject>>); 4]> =
        RefCell::new(core::array::from_fn(|_| (0, None)));
}

#[inline(always)]
fn tl_str_cache_get(hash: u64) -> Option<u32> {
    TL_STR_CACHE.with(|cache| {
        let c = cache.borrow();
        let idx = (hash as usize) & 7;
        let (h, i) = c[idx];
        if h == hash { Some(i) } else { None }
    })
}

#[inline(always)]
fn tl_str_cache_set(hash: u64, index: u32) {
    TL_STR_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        let idx = (hash as usize) & 7;
        c[idx] = (hash, index);
    });
}

#[inline(always)]
fn tl_heap_cache_get(idx: u32) -> Option<Arc<HeapObject>> {
    TL_HEAP_CACHE.with(|cache| {
        let c = cache.borrow();
        let slot = (idx as usize) & 3;
        if c[slot].0 == idx { c[slot].1.clone() } else { None }
    })
}

#[inline(always)]
fn tl_heap_cache_set(idx: u32, obj: Arc<HeapObject>) {
    TL_HEAP_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        let slot = (idx as usize) & 3;
        c[slot] = (idx, Some(obj));
    });
}

// ============================================================================
// Miss 计数器 — 冷路径自动跳过 TL 探测，消除 cold path 开销
// ============================================================================

const TL_MISS_THRESHOLD: u32 = 4;

thread_local! {
    static TL_STR_MISSES: Cell<u32> = const { Cell::new(0) };
    static TL_HEAP_MISSES: Cell<u32> = const { Cell::new(0) };
}

#[inline(always)]
fn should_check_tl_str() -> bool {
    TL_STR_MISSES.with(|c| c.get() < TL_MISS_THRESHOLD)
}

#[inline(always)]
fn should_check_tl_heap() -> bool {
    TL_HEAP_MISSES.with(|c| c.get() < TL_MISS_THRESHOLD)
}

#[inline(always)]
fn tl_str_record_hit() {
    TL_STR_MISSES.with(|c| c.set(0));
}
#[inline(always)]
fn tl_str_record_miss() {
    TL_STR_MISSES.with(|c| c.set(c.get().saturating_add(1)));
}
#[inline(always)]
fn tl_heap_record_hit() {
    TL_HEAP_MISSES.with(|c| c.set(0));
}
#[inline(always)]
fn tl_heap_record_miss() {
    TL_HEAP_MISSES.with(|c| c.set(c.get().saturating_add(1)));
}

// ============================================================================
// 全局字符串池 — 驻留字符串存储（向后兼容接口）
// ============================================================================

struct StringPool {
    strings: Vec<Arc<str>>,
    interner: XxHashMap<String, u32>,
    hashes: Vec<u32>,
}

static STRING_POOL: std::sync::LazyLock<RwLock<StringPool>> = std::sync::LazyLock::new(|| {
    RwLock::new(StringPool {
        strings: Vec::new(),
        interner: XxHashMap::with_hasher(BuildHasherDefault::default()),
        hashes: Vec::new(),
    })
});

fn string_pool_read() -> std::sync::RwLockReadGuard<'static, StringPool> {
    STRING_POOL.read().unwrap_or_else(|e| e.into_inner())
}

fn string_pool_write() -> std::sync::RwLockWriteGuard<'static, StringPool> {
    STRING_POOL.write().unwrap_or_else(|e| e.into_inner())
}

// ============================================================================
// 全局堆对象池（BUG-002 根治：TurboSlab 自研精简 Slab 分配器）
// ============================================================================
//
// TurboSlab 替换原堆池，实现：
// - Slab + 位图管理（标量 trailing_zeros，保留 SIMD 扩展点）
// - Per-CPU 缓存（thread_id hash 到 8 桶）
// - 跨核释放（用 Mutex 替代无锁 CAS，单 VM 场景足够）
// - Grace period reclaim（手动/周期触发，保护最近 64 次分配不被误回收）

use crate::turboslab::{HeapStats, TurboSlabAllocator};

static HEAP_POOL: std::sync::LazyLock<RwLock<TurboSlabAllocator>> =
    std::sync::LazyLock::new(|| RwLock::new(TurboSlabAllocator::new()));

fn heap_pool_read() -> std::sync::RwLockReadGuard<'static, TurboSlabAllocator> {
    HEAP_POOL.read().unwrap_or_else(|e| e.into_inner())
}

fn heap_pool_write() -> std::sync::RwLockWriteGuard<'static, TurboSlabAllocator> {
    HEAP_POOL.write().unwrap_or_else(|e| e.into_inner())
}

// ============================================================================
// 可插拔堆访问器 (Pluggable Heap Accessors) — GC 集成接口
// ============================================================================

/// 堆分配函数类型：将 [`HeapObject`] 存入堆并返回其索引。
///
/// 返回的索引必须 < `HEAP_POOL_INDEX_LIMIT`，否则会导致 GC chunk space 索引冲突。
/// `default_heap_alloc` 内部已通过 `check_heap_pool_index_limit` 强制此不变量。
pub type HeapAllocFn = fn(HeapObject) -> u32;

/// 堆只读访问函数类型：返回索引 `idx` 处 [`HeapObject`] 的只读裸指针。
///
/// # Safety 契约（stop-the-world）
///
/// 实现方在函数内部获取锁守卫（如 `RwLockReadGuard`）取出裸指针后即返回，
/// **守卫随函数栈销毁释放**。调用方拿到的是无锁保护的裸指针。
///
/// 因此调用方必须保证：在解引用返回的指针期间，不会触发：
/// - `reclaim_orphaned`（槽位回收，可能释放底层 `Arc`）
/// - slab 扩容（可能 realloc 导致指针悬垂）
/// - 任何其他可能令 `idx` 槽位失效的操作
///
/// 实际用法：仅在 VM safe-point / stop-the-world 阶段内持有指针，
/// 且持有期间不调用任何可能触发 GC 或 slab 扩容的代码路径。
pub type HeapGetFn = fn(u32) -> *const HeapObject;

/// 堆可变访问函数类型：返回索引 `idx` 处 [`HeapObject`] 的可变裸指针。
///
/// # Safety 契约（stop-the-world）
///
/// 与 [`HeapGetFn`] 相同的 stop-the-world 约束，外加：
/// - 持有 `*mut` 期间必须保证独占访问（无其他线程读写该槽位）
/// - 持有期间不得触发任何可能令 `idx` 失效的操作（reclaim、slab 扩容、GC compaction）
///
/// 默认实现 `default_heap_get_mut` 内部获取 `RwLockWriteGuard` 取出 `*mut` 后即返回，
/// 守卫随栈销毁。调用方需自行保证上述约束。
pub type HeapGetMutFn = fn(u32) -> *mut HeapObject;

/// GC 根收集函数类型：返回当前所有作为 GC 根的 [`Value`]。
pub type HeapRootsFn = fn() -> Vec<Value>;

thread_local! {
    static HEAP_ALLOC_FN: std::cell::Cell<HeapAllocFn> = std::cell::Cell::new(default_heap_alloc);
    static HEAP_GET_FN: std::cell::Cell<HeapGetFn> = std::cell::Cell::new(default_heap_get);
    static HEAP_GET_MUT_FN: std::cell::Cell<HeapGetMutFn> = std::cell::Cell::new(default_heap_get_mut);
    static HEAP_ROOTS_FN: std::cell::Cell<Option<HeapRootsFn>> = const { std::cell::Cell::new(None) };
    /// Optional GC-backed allocator used by `from_heap_object_gc` / `allocate_box`.
    /// When installed, GC-managed allocations bypass the default HEAP_POOL.
    static GC_HEAP_ALLOC_FN: std::cell::Cell<Option<HeapAllocFn>> = const { std::cell::Cell::new(None) };
}

/// Guard against HEAP_POOL index overflow into GC chunk space.
///
/// Indices >= HEAP_POOL_INDEX_LIMIT collide with GC chunk space, causing silent
/// memory corruption. Panic is preferable to corruption.
///
/// `Result` is not an option because `HeapAllocFn` is a function pointer type
/// `fn(HeapObject) -> u32` registered via `register_heap_accessors`, so callers
/// cannot propagate errors without rewriting the whole accessor mechanism.
///
/// # Current status (B4-followup)
///
/// This panic guard is now wired into `default_heap_alloc`. The HEAP_POOL_INDEX_LIMIT
/// has been raised to 4096 and reclaim is triggered by occupancy pressure (>= 80%)
/// instead of a fixed interval, preventing index overflow under normal operation.
pub(crate) fn check_heap_pool_index_limit(idx: u32) {
    if idx as usize >= crate::constants::HEAP_POOL_INDEX_LIMIT {
        panic!(
            "HEAP_POOL index {} overflow (limit {}): next allocation would collide with GC chunk space",
            idx,
            crate::constants::HEAP_POOL_INDEX_LIMIT,
        );
    }
}

/// 默认堆分配函数：将 [`HeapObject`] 分配到全局 `HEAP_POOL` 并返回其索引。
///
/// Reclaim is triggered by occupancy pressure before each allocation:
/// when `occupied_slots >= total_slots / 2` (50% threshold), `reclaim_orphaned`
/// runs to recycle unused slots. This keeps indices below `HEAP_POOL_INDEX_LIMIT`
/// under normal cross-test accumulation.
///
/// This replaces the previous fixed-interval trigger (`RECLAIM_INTERVAL = 256`)
/// which could not keep up with cross-test accumulation in the global HEAP_POOL.
pub fn default_heap_alloc(obj: HeapObject) -> u32 {
    let mut pool = heap_pool_write();

    // Pressure-based reclaim BEFORE allocation.
    // Reclaiming before alloc gives the pool a chance to recycle slots so the
    // new allocation can reuse a freed slot instead of growing past the limit.
    {
        let stats = pool.stats();
        if stats.total_slots > 0 && stats.occupied_slots * 2 >= stats.total_slots {
            pool.reclaim_orphaned();
        }
    }

    // P1 修复：TurboSlabAllocator::alloc 现在返回 Option<u32>。
    // 在当前实现中，TurboCache::alloc 的 5 步回退路径会在 slab 满时
    // 自动分配新 slab（Step 5），因此返回 None 实际上只可能在 OOM
    // （`alloc::alloc` 失败时 `handle_alloc_error` 直接 abort 进程）。
    // 这里保留 None 检查作为防御性兜底，附带诊断信息便于排查。
    let idx = match pool.alloc(obj) {
        Some(idx) => idx,
        None => {
            let stats = pool.stats();
            panic!(
                "default_heap_alloc: TurboSlabAllocator::alloc returned None \
                 (stats: total={}, occupied={}, free={}, slabs={}). \
                 This indicates slab allocation failure (possible OOM or bitmap exhaustion).",
                stats.total_slots, stats.occupied_slots, stats.free_slots, stats.slab_count
            );
        }
    };

    // Hard guard: panic if index overflows into GC chunk space.
    check_heap_pool_index_limit(idx);

    idx
}

/// 默认堆读取函数：返回索引 `idx` 处 [`HeapObject`] 的只读指针。
///
/// 若索引无效或槽位已被回收，返回空指针。
///
/// # Safety 契约（stop-the-world，详见 [`HeapGetFn`]）
///
/// 本函数内部通过 `heap_pool_read()` 获取 `RwLockReadGuard`，调用
/// `pool.get(idx)` 取得 `*const HeapObject` 后函数返回，**守卫随函数栈销毁释放**。
///
/// 调用方拿到的是无锁保护的裸指针。**指针的有效性仅保证到下次可能修改堆状态的操作之前**。
/// 持有指针期间若触发以下任一操作，将产生未定义行为：
/// - `heap_reclaim_orphaned` / `pool.reclaim_orphaned()`：槽位被回收，底层 `Arc` 释放
/// - slab 扩容：`TurboSlabAllocator` 内部 `Vec` realloc 导致指针悬垂
/// - 任何写锁获取（`heap_pool_write`）：独占锁可能导致读端数据不一致
///
/// 实际用法：仅在 VM safe-point / stop-the-world 阶段内持有指针，且持有期间不调用
/// 任何可能触发上述操作的代码路径。
pub fn default_heap_get(idx: u32) -> *const HeapObject {
    heap_pool_read().get(idx)
}

/// 默认堆可变读取函数：返回索引 `idx` 处 [`HeapObject`] 的可变指针。
///
/// 若索引无效或槽位已被回收，返回空指针。
///
/// # Safety 契约（stop-the-world，详见 [`HeapGetMutFn`]）
///
/// 本函数内部通过 `heap_pool_write()` 获取 `RwLockWriteGuard`，调用
/// `pool.get_mut(idx)` 取得 `*mut HeapObject` 后函数返回，**守卫随函数栈销毁释放**。
///
/// 调用方拿到的是无锁保护的可变裸指针。与 [`default_heap_get`] 相同的 stop-the-world
/// 约束全部适用，外加：
/// - 持有 `*mut` 期间必须保证独占访问（无其他线程读写该槽位）
/// - 持有期间不得触发 reclaim、slab 扩容、GC compaction 等任何可能令 `idx` 失效的操作
///
/// 实际用法：仅在 VM safe-point / stop-the-world 阶段内持有指针进行 in-place 修改，
/// 且持有期间不调用任何可能触发 GC 或 slab 扩容的代码路径。
pub fn default_heap_get_mut(idx: u32) -> *mut HeapObject {
    heap_pool_write().get_mut(idx)
}

/// 手动触发孤立条目回收，返回回收数量。
///
/// 回收全局 `HEAP_POOL` 中 `Arc` 强引用计数为 1 的孤立对象，释放槽位供后续复用。
pub fn heap_reclaim_orphaned() -> usize {
    heap_pool_write().reclaim_orphaned()
}

/// 返回当前堆分配器统计信息。
///
/// 统计信息包括总槽位数、已用槽位数、空闲槽位数等。
pub fn heap_stats() -> HeapStats {
    heap_pool_read().stats()
}

/// 注册可插拔的堆访问函数，用于将默认 `HEAP_POOL` 替换为自定义实现。
///
/// 典型用途：VM 启动时注册 GC 管理的堆，使 [`Value::from_heap_object_gc`]
/// 和 [`Value::mutate_heap_object`] 路由到 GC 堆。
///
/// # 线程安全约束（必须遵守）
///
/// 本函数使用 `thread_local! + Cell<fn>` 存储函数指针。`Cell<fn>` 是线程本地的，
/// **注册仅在调用线程内生效**，其他线程不会看到更新（仍使用默认的 `default_heap_*`）。
///
/// 由于全局 `HEAP_POOL` 在进程内多线程共享，跨线程使用未注册的默认访问器
/// 仍可访问同一池（默认实现就是直接读写 `HEAP_POOL`），但无法路由到自定义 GC 堆。
///
/// 因此调用方必须遵守：
/// - **必须在 VM 启动前单线程完成注册**，确保后续所有 VM 工作线程都从同一个
///   注册过的线程派生（子线程继承父线程的 `thread_local` 初始值，但不会继承运行时设置）
/// - 或在每个 VM 工作线程启动时各自调用本函数注册
///
/// Sprint 5 重构方向：改用 `AtomicUsize` 存储 fn 指针以实现进程级全局可见，
/// 消除 thread_local 跨线程不可见问题。
pub fn register_heap_accessors(
    alloc_fn: HeapAllocFn,
    get_fn: HeapGetFn,
    get_mut_fn: HeapGetMutFn,
    roots_fn: Option<HeapRootsFn>,
) {
    HEAP_ALLOC_FN.with(|f| f.set(alloc_fn));
    HEAP_GET_FN.with(|f| f.set(get_fn));
    HEAP_GET_MUT_FN.with(|f| f.set(get_mut_fn));
    HEAP_ROOTS_FN.with(|f| f.set(roots_fn));
}

/// 重置堆访问函数为默认值（使用全局 `HEAP_POOL`，无 GC 根收集）。
pub fn reset_heap_accessors() {
    HEAP_ALLOC_FN.with(|f| f.set(default_heap_alloc));
    HEAP_GET_FN.with(|f| f.set(default_heap_get));
    HEAP_GET_MUT_FN.with(|f| f.set(default_heap_get_mut));
    HEAP_ROOTS_FN.with(|f| f.set(None));
    GC_HEAP_ALLOC_FN.with(|f| f.set(None));
}

/// 注册 GC 支持的分配器，供 [`Value::from_heap_object_gc`] 和 [`allocate_box`] 使用。
///
/// 注册后，GC 管理的分配将绕过默认的 `HEAP_POOL`。
pub fn register_gc_heap_alloc(alloc_fn: HeapAllocFn) {
    GC_HEAP_ALLOC_FN.with(|f| f.set(Some(alloc_fn)));
}

/// 注销 GC 支持的分配器，回退到默认 `HEAP_POOL`。
pub fn unregister_gc_heap_alloc() {
    GC_HEAP_ALLOC_FN.with(|f| f.set(None));
}

/// 获取当前注册的堆根收集函数（如果存在）。
pub fn get_heap_roots_fn() -> Option<HeapRootsFn> {
    HEAP_ROOTS_FN.with(|f| f.get())
}

// ============================================================================
// Box 池函数 (GC 管理的堆实现)
// ============================================================================

pub fn allocate_box(value: Value) -> Result<usize, NuzoError> {
    let val = Value::from_heap_object_gc(HeapObject::Box(value));
    Ok(val.heap_idx_or_err()? as usize)
}

/// 读取 GC Box 索引 `idx` 处存储的值。
///
/// 若索引无效或对象不是 Box，返回 `None`。
pub fn get_box(idx: usize) -> Option<Value> {
    let box_val =
        Value::from_bits(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC));
    box_val
        .with_heap_object(|obj| match obj {
            HeapObject::Box(v) => Some(*v),
            _ => None,
        })
        .flatten()
}

/// 写入 GC Box 索引 `idx` 处的值。
///
/// # 错误
/// - 索引无效时返回 [`NuzoError::index_out_of_bounds`]
/// - 索引处对象不是 Box 时返回类型不匹配错误
pub fn set_box(idx: usize, value: Value) -> Result<(), NuzoError> {
    let box_val =
        Value::from_bits(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC));
    let result = box_val.mutate_heap_object(|obj| match obj {
        HeapObject::Box(v) => {
            *v = value;
            true
        }
        _ => false,
    });
    match result {
        Some(true) => Ok(()),
        Some(false) => Err(NuzoError::type_mismatch("box", "non-box heap object")),
        None => Err(NuzoError::index_out_of_bounds(idx.to_string(), "gc-heap".to_string())),
    }
}

// ============================================================================
// ValueExt — 堆依赖扩展方法
// ============================================================================

/// 为 `Value` 提供依赖 HeapObject / 字符串池 / RuntimeContext 的扩展方法。
///
/// 这些方法无法放在 `nuzo_core::Value` 中，因为它们依赖 L2 层的类型。
#[allow(clippy::wrong_self_convention)] // trait 方法按值取 self 以消费 NaN-tagged 位模式，命名约定与语义一致
pub trait ValueExt {
    // ── 集合检测 ──
    fn is_collection(self) -> bool;
    fn collection_contains(self, target: Value) -> bool;

    // ── 堆对象访问 ──
    fn as_heap_object_opt(self) -> Option<Arc<HeapObject>>;
    fn mutate_heap_object<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut HeapObject) -> R;
    fn as_heap_object_ref(&self) -> Option<*const HeapObject>;
    fn with_heap_object<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&HeapObject) -> R;
    fn with_heap_object_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut HeapObject) -> R;

    // ── GC 堆分配 ──
    fn from_heap_object_gc(obj: HeapObject) -> Value;

    // ── 类型信息 ──
    fn type_name(self) -> &'static str;
    fn is_range(self) -> bool;
    fn as_range_opt(self) -> Option<RangeValue>;
    fn is_closure(self) -> bool;
    fn as_closure_opt(self) -> Option<Arc<FunctionPrototype>>;
    fn as_closure_heap_object_opt(self) -> Option<Arc<HeapObject>>;
    fn is_builtin_fn(self) -> bool;
    fn as_builtin_fn_opt(self) -> Option<(String, usize, BuiltinFnPtr)>;
    fn is_callable(self) -> bool;

    // ── 字符串池 ──
    fn from_string(s: &str) -> Value;
    fn as_string_opt(self) -> Option<String>;
    fn string_from_index(idx: u32) -> Option<String>;
    fn concat_repr(self) -> String;

    // ── 字符串拼接加法（覆盖 nuzo_core 的纯数值加法） ──
    fn add_with_string(self, other: Value) -> Result<Value, NuzoError>;

    // ── Smi 安全解码 ──
    /// 尝试将 Value 解码为 Smi 整数，失败时返回类型错误而非 panic。
    ///
    /// 与 `Value::as_smi()`（基于 `debug_assert!`，release 构建为 no-op）不同，
    /// 此方法在 release 构建中也会执行类型检查，适合不可信输入路径。
    ///
    /// # 返回
    /// - `Ok(i64)`: 成功解码的 Smi 整数值
    /// - `Err(NuzoError::TypeMismatch)`: 非 Smi 类型，错误中包含实际类型名
    fn try_as_smi(self) -> Result<i64, NuzoError>;
}

impl ValueExt for Value {
    fn is_collection(self) -> bool {
        if self.is_heap_object() {
            if let Some(obj) = self.as_heap_object_opt() {
                matches!(obj.as_ref(), HeapObject::Array(_) | HeapObject::Dict(_))
            } else {
                false
            }
        } else {
            false
        }
    }

    fn collection_contains(self, target: Value) -> bool {
        if !self.is_heap_object() {
            return false;
        }
        if let Some(obj) = self.as_heap_object_opt() {
            match obj.as_ref() {
                HeapObject::Array(arr) => arr.iter().any(|v| v.value_equals(&target)),
                HeapObject::Dict(nuzo_dict) => nuzo_dict.contains_value(&target),
                _ => false,
            }
        } else {
            false
        }
    }

    fn as_heap_object_opt(self) -> Option<Arc<HeapObject>> {
        if !self.is_heap_object() {
            return None;
        }

        if self.is_gc_managed() {
            let idx = match self.heap_index() {
                Some(idx) => idx,
                None => {
                    debug_assert!(
                        false,
                        "invariant violated: is_gc_managed() implies is_heap_object, but heap_index() returned None"
                    );
                    return None;
                }
            };
            let ptr = HEAP_GET_FN.with(|getter| getter.get()(idx));
            if ptr.is_null() {
                return None;
            }
            Some(Arc::new(unsafe { (*ptr).clone() }))
        } else {
            let index = (self.to_bits() & HEAP_INDEX_MASK) as u32;
            if should_check_tl_heap() {
                if let Some(cached) = tl_heap_cache_get(index) {
                    tl_heap_record_hit();
                    return Some(cached);
                }
                tl_heap_record_miss();
            }
            let pool = heap_pool_read();
            let obj = pool.get_arc(index)?;
            tl_heap_cache_set(index, Arc::clone(&obj));
            Some(obj)
        }
    }

    fn mutate_heap_object<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut HeapObject) -> R,
    {
        if !self.is_heap_object() {
            return None;
        }
        if self.is_gc_managed() {
            // # Safety 契约（stop-the-world）
            //
            // GC 路径通过 `HEAP_GET_MUT_FN` 取得 `*mut HeapObject` 后立即解引用为
            // `&mut HeapObject` 并调用 `f`。**与默认 `default_heap_get_mut` 相同，
            // 守卫在 accessor 函数返回时即释放**，调用 `f` 期间没有任何锁保护。
            //
            // 这意味着调用方必须保证 `f` 执行期间处于 VM safe-point / stop-the-world：
            // - 不会触发 GC（GC 可能移动/回收对象）
            // - 不会触发 slab 扩容（realloc 导致 `*mut` 悬垂）
            // - 不会通过其他路径并发访问同一 `idx` 槽位（数据竞争）
            //
            // 非默认 accessor（GC 自定义实现）必须自行保证其返回的 `*mut` 在
            // safe-point 期间保持有效，否则将产生未定义行为。
            //
            // 详见 [`HeapGetMutFn`] 类型的 Safety 契约。
            let idx = match self.heap_index() {
                Some(idx) => idx,
                None => {
                    debug_assert!(
                        false,
                        "invariant violated: is_gc_managed() implies is_heap_object, but heap_index() returned None"
                    );
                    return None;
                }
            };
            let mut_ptr = HEAP_GET_MUT_FN.with(|getter| getter.get()(idx));
            if mut_ptr.is_null() {
                return None;
            }
            // SAFETY: 调用方保证当前处于 stop-the-world safe-point，详见上方 Safety 契约。
            Some(f(unsafe { &mut *mut_ptr }))
        } else {
            let index = (self.to_bits() & HEAP_INDEX_MASK) as u32;
            let pool = heap_pool_write();
            let ptr = pool.get_mut(index);
            if ptr.is_null() {
                return None;
            }
            // SAFETY: `pool` 守卫存活于整个 `f` 调用期间，且 `ptr` 来自 `pool.get_mut`，
            // 非 GC 路径下 RwLockWriteGuard 提供独占访问保证。
            Some(f(unsafe { &mut *ptr }))
        }
    }

    fn as_heap_object_ref(&self) -> Option<*const HeapObject> {
        if !self.is_heap_object() {
            return None;
        }
        if self.is_gc_managed() {
            let idx = match self.heap_index() {
                Some(idx) => idx,
                None => {
                    debug_assert!(
                        false,
                        "invariant violated: is_gc_managed() implies is_heap_object, but heap_index() returned None"
                    );
                    return None;
                }
            };
            let ptr = HEAP_GET_FN.with(|f| f.get()(idx));
            if ptr.is_null() {
                return None;
            }
            Some(ptr)
        } else {
            let index = (self.to_bits() & HEAP_INDEX_MASK) as u32;
            let pool = heap_pool_read();
            let ptr = pool.get(index);
            if ptr.is_null() { None } else { Some(ptr) }
        }
    }

    fn with_heap_object<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&HeapObject) -> R,
    {
        if !self.is_heap_object() {
            return None;
        }
        if self.is_gc_managed() {
            let idx = match self.heap_index() {
                Some(idx) => idx,
                None => {
                    debug_assert!(
                        false,
                        "invariant violated: is_gc_managed() implies is_heap_object, but heap_index() returned None"
                    );
                    return None;
                }
            };
            let ptr = HEAP_GET_FN.with(|getter| getter.get()(idx));
            if ptr.is_null() {
                return None;
            }
            Some(f(unsafe { &*ptr }))
        } else {
            let index = (self.to_bits() & HEAP_INDEX_MASK) as u32;
            let pool = heap_pool_read();
            let ptr = pool.get(index);
            if ptr.is_null() { None } else { Some(f(unsafe { &*ptr })) }
        }
    }

    fn with_heap_object_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut HeapObject) -> R,
    {
        if !self.is_heap_object() {
            return None;
        }
        self.mutate_heap_object(f)
    }

    fn from_heap_object_gc(obj: HeapObject) -> Value {
        // Prefer the GC-backed allocator when installed; otherwise fall back to HEAP_POOL.
        let alloc =
            GC_HEAP_ALLOC_FN.with(|f| f.get()).unwrap_or_else(|| HEAP_ALLOC_FN.with(|h| h.get()));
        let idx = alloc(obj);
        // 不变量：分配器返回的索引必须能装入 NaN-tagged 45-bit heap index 字段，
        // 否则 `idx & HEAP_INDEX_MASK_NO_GC` 会截断高位导致索引冲突。
        //
        // 此处保持 `assert!`（与 `from_ptr` / `check_heap_pool_index_limit` 策略一致），
        // 因为这是分配器契约的内部不变量，违反即代表分配器实现错误，
        // 继续执行将产生静默内存腐败（多个对象共享同一索引）。
        // Release 构建中 panic 优于静默 UB。
        assert!(
            idx as u64 <= HEAP_INDEX_MASK_NO_GC,
            "from_heap_object_gc: allocator returned index {} exceeding 45-bit limit ({}); \
             this is an allocator contract violation (allocator returned too large an index, \
             which would cause NaN-tagged index truncation and silent memory corruption)",
            idx,
            HEAP_INDEX_MASK_NO_GC,
        );
        Value::from_bits(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC))
    }

    fn type_name(self) -> &'static str {
        match self.tag() {
            ValueTag::Nil => "nil",
            ValueTag::Bool => "bool",
            ValueTag::Smi => "integer",
            ValueTag::Float => "number",
            ValueTag::String => "string",
            ValueTag::Pointer => {
                if let Some(heap_obj) = self.as_heap_object_opt() {
                    heap_obj.ops().type_name()
                } else {
                    "object"
                }
            }
            ValueTag::Unknown => "unknown",
        }
    }

    #[inline(always)]
    fn is_range(self) -> bool {
        self.is_heap_object()
            && matches!(self.as_heap_object_opt().as_deref(), Some(HeapObject::Range { .. }))
    }

    fn as_range_opt(self) -> Option<RangeValue> {
        if !self.is_heap_object() {
            return None;
        }
        match self.as_heap_object_opt()?.as_ref() {
            HeapObject::Range { start, end, range_end } => Some(RangeValue {
                start: *start,
                end: *end,
                inclusive: matches!(*range_end, RangeEnd::Inclusive),
            }),
            _ => None,
        }
    }

    #[inline(always)]
    fn is_closure(self) -> bool {
        self.is_heap_object()
            && matches!(self.as_heap_object_opt().as_deref(), Some(HeapObject::Closure { .. }))
    }

    fn as_closure_opt(self) -> Option<Arc<FunctionPrototype>> {
        if !self.is_heap_object() {
            return None;
        }
        match self.as_heap_object_opt()?.as_ref() {
            HeapObject::Closure { prototype, .. } => Some(Arc::clone(prototype)),
            _ => None,
        }
    }

    fn as_closure_heap_object_opt(self) -> Option<Arc<HeapObject>> {
        if self.is_closure() { self.as_heap_object_opt() } else { None }
    }

    fn is_builtin_fn(self) -> bool {
        self.is_heap_object()
            && matches!(self.as_heap_object_opt().as_deref(), Some(HeapObject::BuiltinFn { .. }))
    }

    fn as_builtin_fn_opt(self) -> Option<(String, usize, BuiltinFnPtr)> {
        if !self.is_heap_object() {
            return None;
        }
        match self.as_heap_object_opt()?.as_ref() {
            HeapObject::BuiltinFn { name, arity, func } => Some((name.clone(), *arity, *func)),
            _ => None,
        }
    }

    #[inline(always)]
    fn is_callable(self) -> bool {
        self.is_closure() || self.is_builtin_fn()
    }

    // ── 字符串池方法 ──

    #[inline(always)]
    fn from_string(s: &str) -> Value {
        let str_hash = || -> u64 {
            let h = s.as_bytes().iter().fold(s.len() as u32, |acc, &b| nuzo_mix(acc ^ (b as u32)));
            h as u64
        };
        if should_check_tl_str() {
            let h = str_hash();
            if let Some(idx) = tl_str_cache_get(h) {
                let pool = string_pool_read();
                if let Some(cached) = pool.strings.get(idx as usize)
                    && cached.as_ref() == s
                {
                    tl_str_record_hit();
                    return Value::from_bits(STRING_TAG | (idx as u64 & STRING_INDEX_MASK));
                }
            }
            tl_str_record_miss();
        }

        let mut pool = string_pool_write();
        if let Some(&idx) = pool.interner.get(s) {
            tl_str_cache_set(str_hash(), idx);
            tl_str_record_hit();
            return Value::from_bits(STRING_TAG | (idx as u64 & STRING_INDEX_MASK));
        }

        let idx = pool.strings.len() as u32;
        let arc_str: Arc<str> = Arc::from(s);
        pool.strings.push(Arc::clone(&arc_str));
        pool.interner.insert(s.to_string(), idx);
        pool.hashes.push(nuzo_mix(idx));

        debug_assert!((idx as u64) < (1u64 << 47), "string index exceeds 47-bit limit");
        tl_str_cache_set(str_hash(), idx);
        Value::from_bits(STRING_TAG | (idx as u64 & STRING_INDEX_MASK))
    }

    #[inline(always)]
    fn as_string_opt(self) -> Option<String> {
        if !self.is_string() {
            return None;
        }
        let index = (self.to_bits() & STRING_INDEX_MASK) as usize;
        let pool = string_pool_read();
        pool.strings.get(index).map(|arc_str| arc_str.as_ref().to_owned())
    }

    fn string_from_index(idx: u32) -> Option<String> {
        let pool = string_pool_read();
        pool.strings.get(idx as usize).map(|s| s.as_ref().to_owned())
    }

    fn concat_repr(self) -> String {
        if self.is_string() {
            self.as_string_opt().unwrap_or_default()
        } else if self.is_heap_object() {
            // BUG-B 根治：L1 nuzo_core::Value::Display 对堆对象输出 "<heap>"，
            // 因为 L1 不能依赖 L2 的 HeapObject。L2 在此截断，调用 HeapObject 的 Display。
            self.as_heap_object_opt()
                .map(|obj| obj.to_string())
                .unwrap_or_else(|| self.to_string_repr())
        } else {
            self.to_string_repr()
        }
    }

    /// 字符串拼接加法（覆盖 nuzo_core 的纯数值 `add`）。
    ///
    /// 当操作数为字符串时执行拼接，否则委托给 nuzo_core 的数值加法。
    fn add_with_string(self, other: Value) -> Result<Value, NuzoError> {
        if self.is_string() || other.is_string() {
            let left = self.concat_repr();
            let right = other.concat_repr();
            return Ok(Value::from_string(&format!("{}{}", left, right)));
        }
        self.add(other)
    }

    /// `try_as_smi` 实现：先检查 `is_smi()`，成功则调用 `as_smi()`（此时
    /// debug_assert 必通过），失败则返回 `TypeMismatch` 错误。
    #[inline]
    fn try_as_smi(self) -> Result<i64, NuzoError> {
        if self.is_smi() {
            Ok(self.as_smi())
        } else {
            Err(NuzoError::type_mismatch("smi", self.type_name()))
        }
    }
}

// ============================================================================
// Display / Serialize 钩子注册（注入 nuzo_core::Value）
// ============================================================================

/// Display 格式化钩子：为字符串和堆对象提供完整格式化。
///
/// 注入到 `nuzo_core::Value::Display` 实现中，使得 `format!("{}", val)`
/// 能正确显示字符串内容和堆对象结构。
fn value_display_hook(value: &nuzo_core::Value) -> Option<String> {
    if value.is_string() {
        let s = value.as_string_opt()?;
        Some(format!("\"{}\"", s))
    } else if value.is_heap_object() {
        value.as_heap_object_opt().map(|obj| format!("{}", obj))
    } else {
        None
    }
}

/// Serialize 序列化钩子：为字符串提供正确的 JSON 序列化。
///
/// 注入到 `nuzo_core::Value::Serialize` 实现中。
fn value_serialize_hook(value: &nuzo_core::Value) -> Option<String> {
    if value.is_string() { value.as_string_opt() } else { None }
}

/// 注册 Display 和 Serialize 钩子到 `nuzo_core`。
///
/// 应在程序启动时调用一次，使 `Value` 的 `Display` 和 `Serialize`
/// 实现能正确访问字符串池和堆对象（例如字符串字面量带引号显示）。
pub fn register_value_hooks() {
    nuzo_core::set_display_hook(value_display_hook);
    nuzo_core::set_serialize_hook(value_serialize_hook);
}

// ============================================================================
// 单元测试（堆依赖测试，从原 value.rs 迁移）
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::InternalError;
    use crate::turboslab::RECLAIM_GRACE_SIZE;
    use std::sync::OnceLock;

    fn ensure_hooks() {
        static HOOKS: OnceLock<()> = OnceLock::new();
        HOOKS.get_or_init(|| {
            register_value_hooks();
        });
    }

    #[test]
    fn test_string_construction() {
        ensure_hooks();
        let v = Value::from_string("hello");
        assert!(v.is_string());
        assert!(!v.is_number());
        assert!(!v.is_bool());
        assert!(!v.is_nil());
        assert!(!v.is_ptr());
        assert!(!v.is_smi());
    }
    #[test]
    fn test_string_display() {
        ensure_hooks();
        assert_eq!(format!("{}", Value::from_string("hello")), "\"hello\"");
        assert_eq!(format!("{}", Value::from_string("")), "\"\"");
    }
    #[test]
    fn test_string_equality() {
        let a = Value::from_string("test");
        let b = Value::from_string("test");
        assert_eq!(a, b);
    }
    #[test]
    fn test_from_string_roundtrip() {
        for s in &["", "a", "unicode: 中文", "special: !@#$%", "12345", "newlines:\n"] {
            let v = Value::from_string(s);
            assert_eq!(v.as_string_opt().as_deref(), Some(*s));
        }
    }
    #[test]
    fn test_non_string_as_string_opt_returns_none() {
        assert_eq!(NIL.as_string_opt(), None);
        assert_eq!(TRUE.as_string_opt(), None);
        assert_eq!(Value::from_number(42.0).as_string_opt(), None);
    }
    #[test]
    fn test_string_copy_semantics() {
        let o = Value::from_string("copy");
        let c = o;
        assert_eq!(o, c);
    }
    #[test]
    fn test_multiple_strings_independent() {
        let s1 = Value::from_string("first");
        let s2 = Value::from_string("second");
        assert_ne!(s1, s2);
        assert_eq!(s1.as_string_opt().as_deref(), Some("first"));
    }
    #[test]
    fn test_string_serde_serialization() {
        ensure_hooks();
        let json = serde_json::to_string(&Value::from_string("serde")).unwrap();
        assert_eq!(json, "\"serde\"");
    }
    #[test]
    fn test_is_truthy_string() {
        assert!(Value::from_string("").is_truthy());
    }
    #[test]
    fn test_is_collection() {
        assert!(!NIL.is_collection());
        assert!(Value::from_heap_object_gc(HeapObject::Array(vec![])).is_collection());
        assert!(Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new())).is_collection());
    }
    #[test]
    fn test_array_contains_number() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
        ]));
        assert!(arr.collection_contains(Value::from_number(2.0)));
        assert!(!arr.collection_contains(Value::from_number(99.0)));
    }
    #[test]
    fn test_empty_array_contains_nothing() {
        assert!(!Value::from_heap_object_gc(HeapObject::Array(vec![])).collection_contains(TRUE));
    }
    #[test]
    fn test_dict_contains_value() {
        let mut d = NuzoDict::new();
        d.insert(Value::from_string("x").string_index().unwrap(), Value::from_number(1.0));
        let dict = Value::from_heap_object_gc(HeapObject::Dict(d));
        assert!(dict.collection_contains(Value::from_number(1.0)));
    }
    #[test]
    fn test_value_equals_different_types_with_string() {
        assert!(!Value::from_number(1.0).value_equals(&Value::from_string("1")));
    }
    #[test]
    fn test_debug_output_contains_heap_info() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        let s = format!("{:?}", arr);
        assert!(s.contains("Array") || s.contains("HeapObject"));
    }
    #[test]
    fn test_nuzo_error_display_program_variants() {
        assert!(format!("{}", NuzoError::division_by_zero()).contains("division by zero"));
    }
    #[test]
    fn test_nuzo_error_clone_and_eq() {
        let e1 = NuzoError::division_by_zero();
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }
    #[test]
    fn test_from_internal_to_nuzo_in_value_context() {
        let ne: NuzoError = InternalError::NoChunkLoaded.into();
        assert!(matches!(ne.kind, NuzoErrorKind::Internal(InternalError::NoChunkLoaded, None)));
    }
    #[test]
    fn test_range_creation_and_detection() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Exclusive,
        });
        assert!(r.is_range());
        assert_eq!(r.as_range_opt(), Some(RangeValue { start: 1.0, end: 5.0, inclusive: false }));
    }
    #[test]
    fn test_range_display() {
        ensure_hooks();
        assert_eq!(
            format!(
                "{}",
                Value::from_heap_object_gc(HeapObject::Range {
                    start: 1.0,
                    end: 5.0,
                    range_end: RangeEnd::Exclusive
                })
            ),
            "1..5"
        );
        assert_eq!(
            format!(
                "{}",
                Value::from_heap_object_gc(HeapObject::Range {
                    start: 1.0,
                    end: 5.0,
                    range_end: RangeEnd::Inclusive
                })
            ),
            "1..=5"
        );
    }
    #[test]
    fn test_builtin_fn_creation_and_query() {
        fn dummy(_: &[Value]) -> Result<Value, NuzoError> {
            Ok(NIL)
        }
        let b = Value::from_heap_object_gc(HeapObject::BuiltinFn {
            name: "t".into(),
            arity: 0,
            func: dummy,
        });
        assert!(b.is_builtin_fn());
        assert!(b.is_callable());
        let (n, a, _) = b.as_builtin_fn_opt().unwrap();
        assert_eq!(n, "t");
        assert_eq!(a, 0);
    }
    #[test]
    fn test_closure_detection() {
        let proto = FunctionPrototype::new(
            "<anonymous>".to_string(),
            2,
            2,
            Arc::new(vec![]),
            Arc::new(vec![]),
            vec![],
            Arc::new(vec![]),
            Arc::new(Default::default()),
            0,
        );
        let c = Value::from_heap_object_gc(HeapObject::Closure {
            prototype: Arc::new(proto),
            captured: vec![],
            parent_env: None,
        });
        assert!(c.is_closure());
        assert_eq!(c.as_closure_opt().unwrap().arity, 2);
    }
    #[test]
    fn test_tag_classification_heap() {
        assert_eq!(Value::from_string("hi").tag(), ValueTag::String);
        assert_eq!(Value::from_heap_object_gc(HeapObject::Array(vec![])).tag(), ValueTag::Pointer);
    }
    #[test]
    fn test_type_names() {
        assert_eq!(NIL.type_name(), "nil");
        assert_eq!(Value::from_smi(42).type_name(), "integer");
        assert_eq!(Value::from_number(2.5).type_name(), "number");
        assert_eq!(Value::from_string("hi").type_name(), "string");
    }
    #[test]
    fn test_mutate_heap_object() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![Value::from_number(1.0)]));
        let res = arr.mutate_heap_object(|obj| {
            if let HeapObject::Array(inner) = obj {
                inner.push(Value::from_number(2.0));
                inner.len()
            } else {
                0
            }
        });
        assert_eq!(res, Some(2));
        if let Some(obj) = arr.as_heap_object_opt()
            && let HeapObject::Array(inner) = obj.as_ref()
        {
            assert_eq!(inner.len(), 2);
        }
    }
    #[test]
    fn test_mutate_non_heap_returns_none() {
        assert_eq!(NIL.mutate_heap_object(|_| panic!()), None);
    }
    #[test]
    fn test_allocate_get_set_box() {
        let idx = allocate_box(Value::from_number(42.0)).unwrap();
        assert_eq!(get_box(idx), Some(Value::from_number(42.0)));
        set_box(idx, Value::from_string("updated")).unwrap();
        assert_eq!(get_box(idx), Some(Value::from_string("updated")));
    }
    #[test]
    fn test_set_box_invalid_index() {
        assert!(set_box(9999, Value::from_number(1.0)).is_err());
    }
    #[test]
    fn test_from_heap_object_gc_uses_registered_allocator() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CNT: AtomicU32 = AtomicU32::new(100);
        fn ta(_: HeapObject) -> u32 {
            CNT.fetch_add(1, Ordering::SeqCst)
        }
        register_heap_accessors(ta, |_| std::ptr::null(), |_| std::ptr::null_mut(), None);
        let v = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(v.is_gc_managed());
        assert_eq!(v.heap_index(), Some(100));
        reset_heap_accessors();
    }
    #[test]
    fn test_from_heap_object_gc_prefers_gc_allocator() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static GC_ALLOC_CNT: AtomicU32 = AtomicU32::new(300);
        fn gc_alloc(_: HeapObject) -> u32 {
            GC_ALLOC_CNT.fetch_add(1, Ordering::SeqCst)
        }
        register_gc_heap_alloc(gc_alloc);
        let v = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(v.is_gc_managed());
        assert_eq!(v.heap_index(), Some(300));
        unregister_gc_heap_alloc();
        reset_heap_accessors();
    }
    #[test]
    fn test_allocate_box_routes_to_gc_when_installed() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static BOX_CNT: AtomicU32 = AtomicU32::new(HEAP_POOL_INDEX_LIMIT as u32 + 10);
        fn gc_alloc(_: HeapObject) -> u32 {
            BOX_CNT.fetch_add(1, Ordering::SeqCst)
        }
        register_gc_heap_alloc(gc_alloc);
        let idx = allocate_box(Value::from_number(42.0)).unwrap();
        assert!(
            idx >= HEAP_POOL_INDEX_LIMIT,
            "GC box index {} should be >= HEAP_POOL_INDEX_LIMIT",
            idx
        );
        unregister_gc_heap_alloc();
        reset_heap_accessors();
    }
    #[test]
    fn test_is_heap_object_direct() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.is_heap_object());
        assert!(!NIL.is_heap_object());
        assert!(!Value::from_number(1.0).is_heap_object());
        assert!(!Value::from_string("x").is_heap_object());
    }
    #[test]
    fn test_is_string_direct() {
        assert!(!Value::from_number(1.0).is_string());
        assert!(!Value::from_string("x").is_bool());
    }
    #[test]
    fn test_concat_repr_string() {
        let s = Value::from_string("hello");
        assert_eq!(s.concat_repr(), "hello");
    }
    #[test]
    fn test_concat_repr_number() {
        assert_eq!(Value::from_number(42.0).concat_repr(), "42");
    }
    #[test]
    fn test_concat_repr_nil() {
        assert_eq!(NIL.concat_repr(), "nil");
    }
    #[test]
    fn test_to_string_repr_string() {
        ensure_hooks();
        assert_eq!(Value::from_string("hi").to_string_repr(), "\"hi\"");
    }
    #[test]
    fn test_from_string_index_and_string_from_index() {
        let s = Value::from_string("interned_str");
        let idx = s.string_index().unwrap();
        let v = Value::from_string_index(idx);
        assert!(v.is_string());
        assert_eq!(v.as_string_opt().as_deref(), Some("interned_str"));
        assert_eq!(Value::string_from_index(idx).as_deref(), Some("interned_str"));
    }
    #[test]
    fn test_string_from_index_invalid() {
        assert!(Value::string_from_index(999_999).is_none());
    }
    #[test]
    fn test_heap_idx_or_err_heap_object() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.heap_idx_or_err().is_ok());
    }
    #[test]
    fn test_with_heap_object_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
        ]));
        let len = arr.with_heap_object(|obj| match obj {
            HeapObject::Array(v) => v.len(),
            _ => 0,
        });
        assert_eq!(len, Some(2));
    }
    #[test]
    fn test_with_heap_object_non_heap() {
        assert_eq!(NIL.with_heap_object(|_| 42), None);
    }
    #[test]
    fn test_with_heap_object_mut_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![Value::from_number(1.0)]));
        let res = arr.with_heap_object_mut(|obj| {
            if let HeapObject::Array(v) = obj {
                v.push(Value::from_number(2.0));
                v.len()
            } else {
                0
            }
        });
        assert_eq!(res, Some(2));
    }
    #[test]
    fn test_with_heap_object_mut_non_heap() {
        assert_eq!(NIL.with_heap_object_mut(|_| 42), None);
    }
    #[test]
    fn test_as_heap_object_ref_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![Value::from_number(1.0)]));
        let ptr = arr.as_heap_object_ref();
        assert!(ptr.is_some());
        let p = ptr.unwrap();
        assert!(!p.is_null());
        unsafe {
            assert!(matches!(*p, HeapObject::Array(_)));
        }
    }
    #[test]
    fn test_as_heap_object_ref_non_heap() {
        assert!(NIL.as_heap_object_ref().is_none());
        assert!(Value::from_number(42.0).as_heap_object_ref().is_none());
    }
    #[test]
    fn test_as_closure_heap_object_opt() {
        let proto = FunctionPrototype::new(
            "<anonymous>".to_string(),
            2,
            2,
            Arc::new(vec![]),
            Arc::new(vec![]),
            vec![],
            Arc::new(vec![]),
            Arc::new(Default::default()),
            0,
        );
        let c = Value::from_heap_object_gc(HeapObject::Closure {
            prototype: Arc::new(proto),
            captured: vec![],
            parent_env: None,
        });
        assert!(c.as_closure_heap_object_opt().is_some());
    }
    #[test]
    fn test_as_closure_heap_object_opt_non_closure() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.as_closure_heap_object_opt().is_none());
        assert!(NIL.as_closure_heap_object_opt().is_none());
    }
    #[test]
    fn test_default_heap_alloc_get() {
        let idx = default_heap_alloc(HeapObject::Array(vec![Value::from_number(1.0)]));
        let ptr = default_heap_get(idx);
        assert!(!ptr.is_null());
        unsafe {
            assert!(matches!(*ptr, HeapObject::Array(_)));
        }
    }
    #[test]
    fn test_b4_heap_pool_limit_normal_alloc() {
        // Normal allocation must succeed, stay below HEAP_POOL_INDEX_LIMIT, and the
        // boundary check must accept all valid indices [0, HEAP_POOL_INDEX_LIMIT).
        let idx = default_heap_alloc(HeapObject::Box(Value::from_number(42.0)));
        assert!(
            idx < HEAP_POOL_INDEX_LIMIT as u32,
            "normal allocation must stay below HEAP_POOL_INDEX_LIMIT, got {}",
            idx,
        );
        let ptr = default_heap_get(idx);
        assert!(!ptr.is_null(), "allocated object must be retrievable");
        unsafe {
            assert!(
                matches!(*ptr, HeapObject::Box(_)),
                "retrieved object must match what was allocated",
            );
        }
        // Boundary check accepts the full valid range without panicking.
        check_heap_pool_index_limit(0);
        check_heap_pool_index_limit(HEAP_POOL_INDEX_LIMIT as u32 - 1);
    }

    #[test]
    #[should_panic(expected = "HEAP_POOL index")]
    fn test_b4_heap_pool_limit_panic_on_overflow() {
        // Index >= HEAP_POOL_INDEX_LIMIT collides with GC chunk space and must panic.
        // We exercise the guard directly (not via default_heap_alloc) to avoid
        // polluting the global HEAP_POOL with 1024+ held Arcs, which would break
        // other tests running in parallel.
        check_heap_pool_index_limit(HEAP_POOL_INDEX_LIMIT as u32);
    }

    #[test]
    fn test_default_heap_get_invalid_index() {
        let ptr = default_heap_get(999_999);
        assert!(ptr.is_null());
    }
    #[test]
    fn test_default_heap_get_mut_unique_arc() {
        let idx = default_heap_alloc(HeapObject::Box(Value::from_number(42.0)));
        let ptr = default_heap_get_mut(idx);
        assert!(!ptr.is_null());
        unsafe {
            if let HeapObject::Box(ref mut v) = *ptr {
                *v = Value::from_number(99.0);
            }
        }
        let ptr2 = default_heap_get(idx);
        unsafe {
            if let HeapObject::Box(v) = *ptr2 {
                assert_eq!(v.as_number(), 99.0);
            }
        }
    }
    #[test]
    fn test_default_heap_get_mut_invalid_index() {
        let ptr = default_heap_get_mut(999_999);
        assert!(ptr.is_null());
    }
    #[test]
    fn test_get_heap_roots_fn_default_none() {
        reset_heap_accessors();
        assert!(get_heap_roots_fn().is_none());
    }
    #[test]
    fn test_get_heap_roots_fn_registered() {
        fn roots_fn() -> Vec<Value> {
            vec![NIL, TRUE]
        }
        register_heap_accessors(
            default_heap_alloc,
            default_heap_get,
            default_heap_get_mut,
            Some(roots_fn),
        );
        assert!(get_heap_roots_fn().is_some());
        let roots = get_heap_roots_fn().unwrap()();
        assert_eq!(roots.len(), 2);
        reset_heap_accessors();
    }
    #[test]
    fn test_as_builtin_fn_opt_non_heap() {
        assert!(NIL.as_builtin_fn_opt().is_none());
        assert!(Value::from_number(1.0).as_builtin_fn_opt().is_none());
    }
    #[test]
    fn test_as_builtin_fn_opt_non_builtin() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.as_builtin_fn_opt().is_none());
    }
    #[test]
    fn test_as_closure_opt_non_heap() {
        assert!(NIL.as_closure_opt().is_none());
    }
    #[test]
    fn test_as_closure_opt_non_closure() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.as_closure_opt().is_none());
    }
    #[test]
    fn test_as_range_opt_non_heap() {
        assert!(NIL.as_range_opt().is_none());
    }
    #[test]
    fn test_as_range_opt_non_range() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert!(arr.as_range_opt().is_none());
    }
    #[test]
    fn test_collection_contains_non_heap() {
        assert!(!NIL.collection_contains(Value::from_number(1.0)));
        assert!(!Value::from_number(1.0).collection_contains(Value::from_number(1.0)));
    }
    #[test]
    fn test_collection_contains_non_collection() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Exclusive,
        });
        assert!(!r.collection_contains(Value::from_number(1.0)));
    }
    #[test]
    fn test_is_callable_non_callable() {
        assert!(!NIL.is_callable());
        assert!(!Value::from_number(1.0).is_callable());
        assert!(!Value::from_string("x").is_callable());
    }

    // ─── add_with_string ───
    #[test]
    fn test_add_with_string_concat() {
        let a = Value::from_string("hello");
        let b = Value::from_string(" world");
        let result = a.add_with_string(b).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("hello world"));
    }
    #[test]
    fn test_add_with_string_number() {
        let result = Value::from_number(10.0).add_with_string(Value::from_number(32.0)).unwrap();
        assert_eq!(result.as_number(), 42.0);
    }

    // ─── range inclusive test ───
    #[test]
    fn test_range_inclusive_detection() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Inclusive,
        });
        assert!(r.is_range());
        assert_eq!(r.as_range_opt(), Some(RangeValue { start: 1.0, end: 5.0, inclusive: true }));
    }

    // ─── BUG-002 根治验证测试 ───
    // 根治：TurboSlabAllocator 周期性回收 strong_count==1 的孤立条目，
    // 回收后的槽位由 slab 位图管理并在后续分配中复用。

    #[test]
    fn test_default_heap_alloc_returns_unique_valid_indices() {
        let mut indices = std::collections::HashSet::new();
        for i in 0..8 {
            let idx = default_heap_alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(
                i as f64,
            )]));
            assert!(!default_heap_get(idx).is_null(), "allocated slot should be accessible");
            assert!(indices.insert(idx), "indices should be unique: duplicated {}", idx);
        }
    }

    #[test]
    fn test_default_heap_alloc_strong_count_semantics() {
        let idx = default_heap_alloc(HeapObject::BuiltinFn {
            name: "test_fn".into(),
            arity: 0,
            func: |_| Ok(nuzo_core::Value::from_number(0.0)),
        });
        let ptr = default_heap_get(idx);
        assert!(!ptr.is_null(), "allocated object should be accessible");
        unsafe {
            match &*ptr {
                HeapObject::BuiltinFn { name, .. } => assert_eq!(name, "test_fn"),
                other => panic!("expected BuiltinFn, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_default_heap_alloc_pool_growth_semantics() {
        let before = {
            let pool = heap_pool_read();
            pool.stats().total_slots
        };
        let idx = default_heap_alloc(HeapObject::Array(vec![nuzo_core::NIL]));
        let after = {
            let pool = heap_pool_read();
            pool.stats().total_slots
        };
        // TurboSlab 按需分配新 slab，分配后总槽位数应增长或保持稳定
        assert!(after >= before, "pool total slots should not shrink after alloc");
        // idx 必须在当前总槽位范围内
        assert!((idx as usize) < after, "idx must be a valid slot index");
    }

    #[test]
    fn test_reclaim_orphaned_converts_orphaned_to_free() {
        // 在隔离的 TurboSlabAllocator 上测试，避免全局 HEAP_POOL 并行干扰
        let mut allocator = TurboSlabAllocator::new();
        // 分配一个对象但不保留 Value 引用 → Arc 的 strong_count==1
        let idx =
            allocator.alloc(HeapObject::Array(vec![nuzo_core::NIL])).expect("alloc should succeed");
        // 填满 grace window 使 idx 超出保护范围，方可被 reclaim 回收
        for _ in 0..RECLAIM_GRACE_SIZE {
            let _ = allocator.alloc(HeapObject::Box(nuzo_core::NIL));
        }
        // 手动触发回收
        let reclaimed = allocator.reclaim_orphaned();
        assert!(reclaimed >= 1, "should reclaim at least 1 orphaned entry");
        // 回收后 slot 应为 Free，get 返回 null
        assert!(allocator.get(idx).is_null(), "reclaimed slot should return null from get");
    }

    #[test]
    fn test_reuse_after_reclaim() {
        // 全局 HEAP_POOL 共享，并行测试可能影响槽位复用，
        // 因此只验证"回收后分配能成功返回一个有效对象"而非特定索引
        {
            let mut pool = heap_pool_write();
            pool.reclaim_orphaned();
        }
        let idx = default_heap_alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(99.0)]));
        let ptr = default_heap_get(idx);
        assert!(!ptr.is_null(), "allocated slot should be accessible");
        unsafe {
            match &*ptr {
                HeapObject::Array(arr) => {
                    assert_eq!(arr[0].as_number(), 99.0, "should read correct data")
                }
                _ => panic!("expected Array"),
            }
        }
    }

    #[test]
    fn test_default_heap_get_mut_returns_null_on_free_slot() {
        // 在隔离的 TurboSlabAllocator 上测试，避免全局 HEAP_POOL 并行干扰
        let mut allocator = TurboSlabAllocator::new();
        let idx =
            allocator.alloc(HeapObject::Array(vec![nuzo_core::NIL])).expect("alloc should succeed");
        // 填满 grace window 使 idx 超出保护范围，方可被 reclaim 回收
        for _ in 0..RECLAIM_GRACE_SIZE {
            let _ = allocator.alloc(HeapObject::Box(nuzo_core::NIL));
        }
        allocator.reclaim_orphaned();
        let ptr = allocator.get_mut(idx);
        assert!(ptr.is_null(), "get_mut on Free slot should return null");
    }

    #[test]
    fn test_reuse_slot_data_is_fresh_not_stale() {
        // 在隔离的 TurboSlabAllocator 上测试，避免全局 HEAP_POOL 干扰
        let mut allocator = TurboSlabAllocator::new();
        let _ = allocator
            .alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]))
            .expect("alloc should succeed");
        allocator.reclaim_orphaned();
        // 复用槽位分配不同数据（TurboSlab 可能复用任意空闲槽位）
        let reused_idx = allocator
            .alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(99.0)]))
            .expect("alloc should succeed after reclaim");
        let ptr = allocator.get(reused_idx);
        assert!(!ptr.is_null(), "reused slot should be accessible");
        unsafe {
            match &*ptr {
                HeapObject::Array(arr) => {
                    assert_eq!(arr[0].as_number(), 99.0, "should read new data, not stale")
                }
                _ => panic!("expected Array"),
            }
        }
    }

    #[test]
    fn test_backward_compat() {
        // 旧 Value 的 heap_index 在槽位未回收时应仍能读取正确对象
        let idx = default_heap_alloc(HeapObject::Array(vec![Value::from_number(42.0)]));
        let ptr = default_heap_get(idx);
        assert!(!ptr.is_null(), "old heap_index should remain accessible before reclamation");
        unsafe {
            match &*ptr {
                HeapObject::Array(arr) => {
                    assert_eq!(arr[0].as_number(), 42.0, "data should match original allocation")
                }
                other => panic!("expected Array, got {other:?}"),
            }
        }
    }

    // ========================================================================
    // 回归测试 — BUG-A/B/D
    // ========================================================================

    use crate::traits::NuzoType;
    use nuzo_core::error::NuzoErrorKind;

    // BUG-B: print(arr) 之前显示 "<heap>"，因为 L1 nuzo_core::Value::Display
    // 对堆对象输出占位符，L1 无法访问 L2 的 HeapObject。
    #[test]
    fn test_concat_repr_array_shows_elements() {
        ensure_hooks();
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(10.0),
            Value::from_number(20.0),
            Value::from_number(30.0),
        ]));
        assert_eq!(arr.concat_repr(), "[10, 20, 30]");
    }

    #[test]
    fn test_concat_repr_empty_array() {
        ensure_hooks();
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        assert_eq!(arr.concat_repr(), "[]");
    }

    #[test]
    fn test_concat_repr_dict_shows_entries() {
        ensure_hooks();
        let mut dict = crate::nuzo_dict::NuzoDict::new();
        // NuzoDict::insert 要求 key_index: u32（已 intern 的字符串索引），
        // 不是 Value 本身。参考 heap.rs::test_obj_len_dict 的用法。
        let key = Value::from_string("a").string_index().unwrap();
        dict.insert(key, Value::from_number(1.0));
        let d = Value::from_heap_object_gc(HeapObject::Dict(dict));
        // 不强制具体格式（实现可能含空格），但必须包含键值，且不得为 "<heap>"
        let repr = d.concat_repr();
        assert!(!repr.contains("<heap>"), "dict repr should not be placeholder: {repr}");
        assert!(repr.contains("a"), "dict repr should contain key: {repr}");
        assert!(repr.contains('1'), "dict repr should contain value: {repr}");
    }

    // BUG-D: 字符串索引 s[i] 之前报 "index read does not support string"。
    // 修复后按 Unicode scalar 返回长度为 1 的字符串。
    #[test]
    fn test_string_get_index_ascii() {
        ensure_hooks();
        let s = Value::from_string("Hello");
        let h = s.get_index(Value::from_smi(0)).expect("index 0 should succeed");
        assert_eq!(h.as_string_opt().as_deref(), Some("H"));
        let o = s.get_index(Value::from_smi(4)).expect("index 4 should succeed");
        assert_eq!(o.as_string_opt().as_deref(), Some("o"));
    }

    #[test]
    fn test_string_get_index_unicode() {
        ensure_hooks();
        // 4 个 CJK 字符（每个 3 字节 UTF-8），验证按字符而非字节计数。
        let s = Value::from_string("你好世界");
        assert_eq!(s.get_index(Value::from_smi(0)).unwrap().as_string_opt().as_deref(), Some("你"));
        assert_eq!(s.get_index(Value::from_smi(1)).unwrap().as_string_opt().as_deref(), Some("好"));
        assert_eq!(s.get_index(Value::from_smi(3)).unwrap().as_string_opt().as_deref(), Some("界"));
    }

    #[test]
    fn test_string_get_index_out_of_bounds() {
        ensure_hooks();
        let s = Value::from_string("Hello");
        let err = s.get_index(Value::from_smi(5)).expect_err("index 5 should fail");
        assert!(
            matches!(err.kind, NuzoErrorKind::IndexOutOfBounds { .. }),
            "expected IndexOutOfBounds, got {err:?}"
        );
    }

    #[test]
    fn test_string_get_index_negative() {
        ensure_hooks();
        let s = Value::from_string("Hello");
        let err = s.get_index(Value::from_smi(-1)).expect_err("negative index should fail");
        assert!(
            matches!(err.kind, NuzoErrorKind::IndexOutOfBounds { .. }),
            "expected IndexOutOfBounds for negative, got {err:?}"
        );
    }

    #[test]
    fn test_string_get_index_non_integer() {
        ensure_hooks();
        let s = Value::from_string("Hello");
        let err = s.get_index(Value::from_number(1.5)).expect_err("non-integer index should fail");
        assert!(
            matches!(err.kind, NuzoErrorKind::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );
    }

    // ========================================================================
    // 回归测试 — TODO 债务修复（#2 try_as_smi / #5 is_nil / #9 is_number）
    // ========================================================================

    #[test]
    fn test_try_as_smi_returns_err_on_non_smi() {
        ensure_hooks();
        // Smi 输入：返回 Ok(值)，与 as_smi 结果一致
        assert_eq!(Value::from_smi(42).try_as_smi(), Ok(42));
        assert_eq!(Value::from_smi(-1).try_as_smi(), Ok(-1));
        assert_eq!(Value::from_smi(0).try_as_smi(), Ok(0));

        // 非 Smi 输入：返回 TypeMismatch 错误，而非 panic
        let non_smi_cases: [(&str, Value); 4] = [
            ("nil", NIL),
            ("bool", TRUE),
            ("string", Value::from_string("hello")),
            ("float", Value::from_number(1.5)),
        ];
        for (label, v) in non_smi_cases {
            let err = v.try_as_smi().expect_err("non-smi should be TypeMismatch");
            assert!(
                matches!(err.kind, NuzoErrorKind::TypeMismatch { .. }),
                "{label}: expected TypeMismatch, got {err:?}"
            );
        }
    }

    #[test]
    fn test_is_nil_predicate() {
        ensure_hooks();
        // NIL 谓词为 true
        assert!(NIL.is_nil());
        // 其他类型为 false（NaN-tagging 编码区分 nil 与 Smi 0 / Float 0.0）
        assert!(!TRUE.is_nil());
        assert!(!FALSE.is_nil());
        assert!(!Value::from_smi(0).is_nil());
        assert!(!Value::from_string("").is_nil());
        assert!(!Value::from_number(0.0).is_nil());
    }

    #[test]
    fn test_is_number_unified_path() {
        ensure_hooks();
        // Smi 是 number
        assert!(Value::from_smi(0).is_number());
        assert!(Value::from_smi(42).is_number());
        assert!(Value::from_smi(-1).is_number());
        // Float 是 number
        assert!(Value::from_number(0.0).is_number());
        assert!(Value::from_number(1.5).is_number());
        assert!(Value::from_number(-1.5).is_number());
        // 非 number 类型
        assert!(!NIL.is_number());
        assert!(!TRUE.is_number());
        assert!(!FALSE.is_number());
        assert!(!Value::from_string("42").is_number());
    }
}

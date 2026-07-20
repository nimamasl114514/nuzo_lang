//! # GC 模块 — 增量标记-清除垃圾回收器
//!
//! ## 子模块职责
//!
//! | 模块 | 职责 |
//! |------|------|
//! | [`mark`] | 三色标记阶段（tri-color marking、trace、write barrier） |
//! | [`sweep`] | 清除阶段（sweep、finalization、free list 维护） |
//! | [`alloc`] | 分配器（allocate、region bump、arena 管理） |
//! | [`heap`] | 堆访问层（HeapPool、scratch 区、ERSA 划痕区） |
//!
//! ## 公开 API
//!
//! - [`Gc`] — GC 主结构体
//! - [`Trace`] — 可追踪 trait（堆对象实现此 trait 以支持 GC）
//! - [`GcStats`] — GC 统计信息
//! - [`ChunkInfo`] — Chunk 调试信息
//! - [`is_scratch()`] — scratch 区检测

pub mod alloc;
pub mod heap;
pub mod mark;
pub mod sweep;

// Re-export key heap functions at gc module level for ergonomic access
pub(crate) use heap::install_scratch_aware_accessors;
pub(crate) use heap::update_gc_chunks_ptr;

// Re-export 公开类型和自由函数（方法通过 Gc 实例调用，无需重导出）
// 注意：impl Gc 中的方法（alloc/collect/sweep/mark_roots 等）不能通过 pub use 重导出
// 外部代码使用 gc.alloc(obj)、gc.collect() 等方式调用
pub use heap::ChunkInfo;
pub use heap::GcStats;
pub use heap::is_scratch;
pub use heap::{GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY};

// ============================================================================
// use 语句
// ============================================================================

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::Arc;

#[allow(unused_imports)]
use nuzo_core::Value;
use nuzo_values::HeapObject;
use nuzo_values::constants::HEAP_INDEX_MASK_NO_GC;

use nuzo_config::GcConfig;
use nuzo_core::GC_MIN_THRESHOLD;
use nuzo_proc_core::define_constants;
use nuzo_signal::{BusScope, Signal, SignalBus};

// 从子模块导入常量和辅助函数
use heap::{
    CHUNK_VEC_INITIAL_CAPACITY, COLD_STACK_INITIAL_CAPACITY, HOT_STACK_INITIAL_CAPACITY,
    SCRATCH_CAP,
};

// ============================================================================
// 硬编码常量（公开，被外部测试引用）
// ============================================================================

define_constants! {
    pub GC_MARK_RATE: usize = 8;
    pub GC_SWEEP_RATE: usize = 16;
    pub GC_CHUNK_SHIFT: u32 = 10;
    pub GC_NURSERY_THRESHOLD: usize = 1024 * 1024;
    pub GC_TENURED_MULTIPLIER: usize = 8;
    pub GC_PROMOTE_SURVIVAL_RATIO: f64 = 0.4;
    pub GC_COLD_AGE_THRESHOLD: u8 = 3;
    pub GC_DEEP_GC_INTERVAL: u8 = 10;
}

const DEFAULT_MARK_RATE: usize = GC_MARK_RATE;
const DEFAULT_SWEEP_RATE: usize = GC_SWEEP_RATE;

/// Number of placeholder chunks reserved at the start of the GC chunk array
/// to reserve the index space `[0, HEAP_POOL_INDEX_LIMIT)` for the HEAP_POOL.
/// GC allocation starts from `GC_FIRST_ACTIVE_CHUNK`, ensuring that
/// `global_idx >= HEAP_POOL_INDEX_LIMIT` for all GC-managed objects.
const GC_FIRST_ACTIVE_CHUNK: usize =
    nuzo_values::constants::HEAP_POOL_INDEX_LIMIT / (1 << GC_CHUNK_SHIFT);

// ============================================================================
// GC 阶段与类型枚举（pub(crate) 供子模块使用）
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GcPhase {
    Idle,
    Marking,
    Sweeping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GcType {
    Minor,
    Major,
    Deep,
}

// ============================================================================
// Trace Trait 定义及实现
// ============================================================================

pub trait Trace {
    fn trace(&self, gc: &mut super::Gc);
}

impl Trace for Value {
    #[inline(always)]
    fn trace(&self, gc: &mut super::Gc) {
        // Only GC-managed heap objects carry indices that belong to the GC heap.
        // HEAP_POOL objects are reference-counted and must not reach mark_index.
        if self.is_gc_managed() {
            gc.mark_index((self.into_raw_bits() & HEAP_INDEX_MASK_NO_GC) as u32);
        }
    }
}

impl Trace for HeapObject {
    #[inline]
    fn trace(&self, gc: &mut super::Gc) {
        self.trace_gc(&mut |idx| gc.mark_index(idx));
    }
}

impl Trace for Vec<Value> {
    #[inline(always)]
    fn trace(&self, gc: &mut super::Gc) {
        self.iter().for_each(|v| v.trace(gc));
    }
}
impl<T: Trace> Trace for Option<T> {
    #[inline(always)]
    fn trace(&self, gc: &mut super::Gc) {
        if let Some(i) = self {
            i.trace(gc);
        }
    }
}
impl<T: Trace + ?Sized> Trace for Arc<T> {
    #[inline(always)]
    fn trace(&self, gc: &mut super::Gc) {
        (**self).trace(gc);
    }
}
impl<T: Trace> Trace for Box<T> {
    #[inline(always)]
    fn trace(&self, gc: &mut super::Gc) {
        (**self).trace(gc);
    }
}
impl Trace for () {
    #[inline(always)]
    fn trace(&self, _gc: &mut super::Gc) {}
}

// ============================================================================
// Gc 主结构体定义（所有字段 pub(crate) 供子模块 impl 块使用）
// ============================================================================

/// 句柄式精确追踪垃圾回收器 (Handle-Based Tracing GC)
pub struct Gc {
    pub(crate) chunks: Vec<self::heap::Chunk>,
    pub(crate) active_chunk: usize,
    pub(crate) mark_epoch: u8,
    pub(crate) phase: GcPhase,
    /// Hot marking stack — small bounded buffer kept inside CPU cache.
    ///
    /// Entries spilled when `len >= HOT_STACK_CAP` (see `mark.rs`) are pushed
    /// to [`cold_stack`]. Marking drains `hot_stack` first, issuing prefetch
    /// hints for the next `PREFETCH_DISTANCE` entries to hide memory latency
    /// of pointer-chasing workloads (e.g. deep linked-list traces).
    pub(crate) hot_stack: Vec<u32>,
    /// Cold overflow stack for wave-front entries that did not fit in
    /// [`hot_stack`]. Drained only after `hot_stack` is empty.
    pub(crate) cold_stack: Vec<u32>,
    pub(crate) sweep_cursor: u32,

    pub(crate) allocated_bytes: usize,
    pub(crate) nursery_bytes: usize,
    pub(crate) tenured_bytes: usize,

    pub(crate) nursery_threshold: usize,
    pub(crate) tenured_threshold: usize,
    pub(crate) bytes_until_gc: isize,

    pub(crate) free_count: usize,
    pub(crate) mark_rate: usize,
    pub(crate) sweep_rate: usize,
    pub(crate) roots_fn: Option<fn(&mut Gc, *mut c_void)>,
    pub(crate) roots_userdata: *mut c_void,

    pub(crate) minor_gc_count: u64,
    pub(crate) major_gc_count: u64,
    pub(crate) deep_gc_count: u64,

    pub(crate) scratch_data: Box<[Option<UnsafeCell<HeapObject>>]>,
    pub(crate) scratch_top: u32,
    pub(crate) scratch_alloc_count: u64,
    pub(crate) scratch_promote_count: u64,
    pub(crate) scratch_reset_count: u64,
    /// S3 修复：scratch 对象的标记 epoch（与 scratch_data 并行，大小 SCRATCH_CAP）。
    ///
    /// `process_wave_front_step` 遇到 scratch 索引时，trace 其引用的对象以确保
    /// scratch 引用的堆对象参与标记（防止误回收）。此字段记录每个 scratch 槽
    /// 上次被 trace 的 mark_epoch，避免循环引用导致无限递归。
    /// 比较 `scratch_mark_epoch[off] == self.mark_epoch` 判断本 epoch 是否已处理。
    pub(crate) scratch_mark_epoch: Vec<u8>,

    pub(crate) config: GcConfig,

    /// GC 作用域信号总线（scoped SignalBus）
    ///
    /// 持有 `Arc<SignalBus>` 以支持多观察者广播模式。
    /// 通过 `GC_WILL_COLLECT_KEY` / `GC_DID_COLLECT_KEY` 查找信号。
    pub(crate) bus: Arc<SignalBus>,
}
// ============================================================================
// 构造器与配置 API (impl Gc)
// ============================================================================

impl Gc {
    /// 创建 GC 作用域的 SignalBus 并注册 GC 信号
    fn create_gc_bus() -> Arc<SignalBus> {
        use self::heap::{GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY};
        let bus = Arc::new(SignalBus::scoped(BusScope::Gc));
        let sig_will = Signal::named("gc_will_collect");
        let sig_did = Signal::named("gc_did_collect");
        let _ = bus.register(&GC_WILL_COLLECT_KEY, &sig_will);
        let _ = bus.register(&GC_DID_COLLECT_KEY, &sig_did);
        bus
    }

    pub fn new(threshold: usize) -> Self {
        let t = threshold.max(GC_MIN_THRESHOLD);
        let scratch_data: Box<[Option<UnsafeCell<HeapObject>>]> =
            std::iter::repeat_with(|| None).take(SCRATCH_CAP).collect();
        let mut gc = Self {
            chunks: Vec::with_capacity(CHUNK_VEC_INITIAL_CAPACITY),
            active_chunk: 0,
            free_count: 0,
            mark_epoch: 0,
            phase: GcPhase::Idle,
            hot_stack: Vec::with_capacity(HOT_STACK_INITIAL_CAPACITY),
            cold_stack: Vec::with_capacity(COLD_STACK_INITIAL_CAPACITY),
            sweep_cursor: 0,
            allocated_bytes: 0,
            nursery_bytes: 0,
            tenured_bytes: 0,
            nursery_threshold: GC_NURSERY_THRESHOLD.min(t),
            tenured_threshold: GC_NURSERY_THRESHOLD * GC_TENURED_MULTIPLIER,
            bytes_until_gc: GC_NURSERY_THRESHOLD.min(t) as isize,
            mark_rate: DEFAULT_MARK_RATE,
            sweep_rate: DEFAULT_SWEEP_RATE,
            roots_fn: None,
            roots_userdata: std::ptr::null_mut(),
            minor_gc_count: 0,
            major_gc_count: 0,
            deep_gc_count: 0,
            scratch_data,
            scratch_top: 0,
            scratch_alloc_count: 0,
            scratch_promote_count: 0,
            scratch_reset_count: 0,
            scratch_mark_epoch: vec![0u8; SCRATCH_CAP],
            config: GcConfig::default(),
            bus: Self::create_gc_bus(),
        };
        // Reserve placeholder chunks [0, GC_FIRST_ACTIVE_CHUNK) to align the
        // GC index space above HEAP_POOL_INDEX_LIMIT. Chunk 0..GC_FIRST_ACTIVE_CHUNK
        // are empty placeholders whose index range [0, HEAP_POOL_INDEX_LIMIT) belongs
        // to the HEAP_POOL; GC allocation starts from GC_FIRST_ACTIVE_CHUNK.
        for _ in 0..=GC_FIRST_ACTIVE_CHUNK {
            gc.chunks.push(self::heap::Chunk::new_nursery());
        }
        gc.active_chunk = GC_FIRST_ACTIVE_CHUNK;
        gc
    }

    pub fn with_default_threshold() -> Self {
        Self::with_config(GcConfig::default())
    }
    pub fn with_threshold(threshold: usize) -> Self {
        Self::new(threshold)
    }

    pub fn with_config(gc_config: GcConfig) -> Self {
        assert_eq!(
            gc_config.chunk_shift, GC_CHUNK_SHIFT,
            "GcConfig.chunk_shift must equal GC_CHUNK_SHIFT"
        );
        let threshold = gc_config.threshold.max(gc_config.min_threshold);
        let nursery_threshold = gc_config.nursery_threshold.min(threshold);
        let tenured_threshold = gc_config.nursery_threshold * gc_config.tenured_multiplier;
        let scratch_data: Box<[Option<UnsafeCell<HeapObject>>]> =
            std::iter::repeat_with(|| None).take(SCRATCH_CAP).collect();
        let mut gc = Self {
            chunks: Vec::with_capacity(CHUNK_VEC_INITIAL_CAPACITY),
            active_chunk: 0,
            free_count: 0,
            mark_epoch: 0,
            phase: GcPhase::Idle,
            hot_stack: Vec::with_capacity(HOT_STACK_INITIAL_CAPACITY),
            cold_stack: Vec::with_capacity(COLD_STACK_INITIAL_CAPACITY),
            sweep_cursor: 0,
            allocated_bytes: 0,
            nursery_bytes: 0,
            tenured_bytes: 0,
            nursery_threshold,
            tenured_threshold,
            bytes_until_gc: nursery_threshold as isize,
            mark_rate: gc_config.mark_rate,
            sweep_rate: gc_config.sweep_rate,
            roots_fn: None,
            roots_userdata: std::ptr::null_mut(),
            minor_gc_count: 0,
            major_gc_count: 0,
            deep_gc_count: 0,
            scratch_data,
            scratch_top: 0,
            scratch_alloc_count: 0,
            scratch_promote_count: 0,
            scratch_reset_count: 0,
            scratch_mark_epoch: vec![0u8; SCRATCH_CAP],
            config: gc_config,
            bus: Self::create_gc_bus(),
        };
        // Reserve placeholder chunks [0, GC_FIRST_ACTIVE_CHUNK) to align the
        // GC index space above HEAP_POOL_INDEX_LIMIT. Chunk 0..GC_FIRST_ACTIVE_CHUNK
        // are empty placeholders whose index range [0, HEAP_POOL_INDEX_LIMIT) belongs
        // to the HEAP_POOL; GC allocation starts from GC_FIRST_ACTIVE_CHUNK.
        for _ in 0..=GC_FIRST_ACTIVE_CHUNK {
            gc.chunks.push(self::heap::Chunk::new_nursery());
        }
        gc.active_chunk = GC_FIRST_ACTIVE_CHUNK;
        gc
    }

    pub fn with_mark_rate(mut self, rate: usize) -> Self {
        assert!((1..=1024).contains(&rate));
        self.mark_rate = rate;
        self
    }
    pub fn with_sweep_rate(mut self, rate: usize) -> Self {
        assert!((1..=2048).contains(&rate));
        self.sweep_rate = rate;
        self
    }

    /// 替换 GC 内部的信号总线（用于共享外部 SignalBus）
    ///
    /// # 示例
    /// ```ignore
    /// let shared_bus = Arc::new(SignalBus::scoped(BusScope::Gc));
    /// let gc = Gc::with_default_threshold().with_bus(shared_bus.clone());
    /// ```
    pub fn with_bus(mut self, bus: Arc<SignalBus>) -> Self {
        self.bus = bus;
        self
    }

    /// 获取 GC 信号总线的引用
    pub fn bus(&self) -> &Arc<SignalBus> {
        &self.bus
    }

    // ── 统计与查询 ──

    pub fn stats(&self) -> self::heap::GcStats {
        let total: usize = self.chunks.iter().map(|c| c.top as usize).sum();
        let alive: usize = self.chunks.iter().map(|c| c.alive_count as usize).sum();
        self::heap::GcStats {
            total_objects: total,
            live_objects: alive,
            dead_objects: total - alive,
            free_slots: self.free_count,
            allocated_bytes: self.allocated_bytes,
            threshold: self.threshold(),
        }
    }

    #[inline(always)]
    pub fn threshold(&self) -> usize {
        self.nursery_threshold
    }

    pub fn set_gc_threshold(&mut self, t: usize) {
        self.nursery_threshold = t.max(self.config.min_threshold);
        self.tenured_threshold = self.nursery_threshold * self.config.tenured_multiplier;
        self.bytes_until_gc = self.nursery_threshold as isize;
    }

    pub fn with_threshold_value(&mut self, t: usize) {
        self.set_gc_threshold(t);
    }
    pub fn register_roots_fn(
        &mut self,
        f: Option<fn(&mut Gc, *mut c_void)>,
        userdata: *mut c_void,
    ) {
        self.roots_fn = f;
        self.roots_userdata = userdata;
    }
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|c| c.top as usize).sum()
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|c| c.top == 0)
    }

    // ── 清除 / 重置 ──

    pub fn clear(&mut self) {
        self.chunks.clear();
        // Same placeholder chunk setup as new() — see comment there.
        for _ in 0..=GC_FIRST_ACTIVE_CHUNK {
            self.chunks.push(self::heap::Chunk::new_nursery());
        }
        self.active_chunk = GC_FIRST_ACTIVE_CHUNK;
        self::heap::update_gc_chunks_ptr(self);
        self.free_count = 0;
        self.allocated_bytes = 0;
        self.nursery_bytes = 0;
        self.tenured_bytes = 0;
        self.mark_epoch = 0;
        self.hot_stack.clear();
        self.cold_stack.clear();
        self.sweep_cursor = 0;
        self.phase = GcPhase::Idle;
        self.bytes_until_gc = self.nursery_threshold as isize;
        for slot in self.scratch_data.iter_mut() {
            *slot = None;
        }
        self.scratch_top = 0;
        self.scratch_alloc_count = 0;
        self.scratch_promote_count = 0;
        self.scratch_reset_count = 0;
        // S3: 重置 scratch 标记 epoch，使下次 GC 能正确 trace scratch 对象
        self.scratch_mark_epoch.fill(0);
        self.minor_gc_count = 0;
        self.major_gc_count = 0;
        self.deep_gc_count = 0;
    }

    // ── Chunk 信息 ──

    pub fn chunk_info(&self) -> Vec<self::heap::ChunkInfo> {
        self.chunks
            .iter()
            .enumerate()
            .map(|(ci, c)| self::heap::ChunkInfo {
                top: c.top,
                alive_count: c.alive_count,
                free_count: c.free_count,
                is_active: ci == self.active_chunk,
                generation: c.generation,
                is_dirty: c.is_dirty,
                age: c.age,
                is_cold: c.is_cold,
            })
            .collect()
    }

    // ── 内部辅助方法（供各子模块的 impl Gc 使用） ──

    #[inline(always)]
    pub(crate) fn refresh_chunk_pointers(&self) {
        self::heap::update_gc_chunks_ptr(self);
    }
}

// ============================================================================
// Default / Debug trait 实现
// ============================================================================

impl Default for Gc {
    fn default() -> Self {
        Self::with_default_threshold()
    }
}

impl std::fmt::Debug for Gc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gc")
            .field("chunks", &self.chunks.len())
            .field("allocated_bytes", &self.allocated_bytes)
            .field("nursery_threshold", &self.nursery_threshold)
            .field("tenured_threshold", &self.tenured_threshold)
            .field("mark_epoch", &self.mark_epoch)
            .field("phase", &self.phase)
            .finish()
    }
}
// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::NuzoDict;

    fn noop_roots(_gc: &mut Gc, _userdata: *mut c_void) {}

    #[test]
    fn test_alloc_and_get() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(42.0)]));
        match gc.get(idx).expect("gc.get should succeed for valid index") {
            HeapObject::Array(a) => assert_eq!(a[0], nuzo_core::Value::from_number(42.0)),
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn test_try_get() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::Range {
            start: 0.0,
            end: 10.0,
            range_end: nuzo_values::RangeEnd::Exclusive,
        });
        assert!(gc.try_get(idx).is_some());
        assert!(gc.try_get(u32::MAX).is_none());
    }

    #[test]
    fn test_get_mut_if_present() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::BuiltinFn {
            name: "test".into(),
            arity: 0,
            func: |_| Ok(nuzo_core::Value::from_number(0.0)),
        });
        assert!(gc.get_mut_if_present(idx).is_some());
        assert!(gc.get_mut_if_present(u32::MAX).is_none());
    }

    #[test]
    fn test_generational_minor_gc() {
        let mut gc = Gc::new(256);
        let long_lived = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        for _ in 0..20 {
            gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(2.0)]));
        }
        gc.collect_with_roots(std::iter::once(nuzo_core::Value::from_gc_index(long_lived)));
        match gc.get(long_lived).expect("gc.get should succeed for valid index") {
            HeapObject::Array(a) => assert_eq!(a[0], nuzo_core::Value::from_number(1.0)),
            _ => panic!(),
        }
    }

    #[test]
    fn test_minor_gc_reclaims_short_lived() {
        let mut gc = Gc::new(256);
        let pre_stats = gc.stats();
        let mut short_lived = Vec::new();
        for i in 0..10 {
            short_lived
                .push(gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(i as f64)])));
        }
        gc.collect_with_roots(std::iter::empty::<nuzo_core::Value>());
        let post_stats = gc.stats();
        assert!(post_stats.free_slots >= pre_stats.free_slots);
    }

    #[test]
    fn test_in_place_promotion() {
        let mut gc = Gc::new(512);
        let mut root_vals = Vec::new();
        for _ in 0..5 {
            let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
            root_vals.push(nuzo_core::Value::from_gc_index(idx));
        }
        gc.collect_with_roots(root_vals.into_iter());
        let info = gc.chunk_info();
        let total_alive: u32 = info.iter().map(|c| c.alive_count).sum();
        assert!(total_alive >= 5);
    }

    #[test]
    fn test_dual_threshold() {
        let gc = Gc::with_default_threshold();
        assert!(gc.nursery_threshold < gc.tenured_threshold);
        assert_eq!(gc.tenured_threshold, gc.nursery_threshold * GC_TENURED_MULTIPLIER);
    }

    #[test]
    fn test_set_threshold_updates_both() {
        let mut gc = Gc::with_default_threshold();
        gc.set_gc_threshold(2048);
        assert_eq!(gc.nursery_threshold, 2048);
        assert_eq!(gc.tenured_threshold, 2048 * GC_TENURED_MULTIPLIER);
    }

    #[test]
    fn test_scratch_alloc_and_promote() {
        let mut gc = Gc::with_default_threshold();
        let scratch_idx =
            gc.alloc_scratch(HeapObject::Array(vec![nuzo_core::Value::from_number(99.0)]));
        assert!(is_scratch(scratch_idx));
        match gc.get(scratch_idx).expect("gc.get should succeed for valid scratch index") {
            HeapObject::Array(a) => assert_eq!(a[0], nuzo_core::Value::from_number(99.0)),
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn test_safe_point_promotes_live_scratch() {
        let mut gc = Gc::with_default_threshold();
        let s1 = gc.alloc_scratch(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        let s2 = gc.alloc_scratch(HeapObject::Array(vec![nuzo_core::Value::from_number(2.0)]));
        let remap = gc.safe_point(|| vec![s1]);
        assert!(remap.iter().any(|&(old, _)| old == s1));
        assert!(!remap.iter().any(|&(old, _)| old == s2));
    }

    #[test]
    fn test_scratch_stats() {
        let mut gc = Gc::with_default_threshold();
        gc.alloc_scratch(HeapObject::BuiltinFn {
            name: "a".into(),
            arity: 0,
            func: |_| Ok(nuzo_core::Value::from_number(0.0)),
        });
        gc.alloc_scratch(HeapObject::BuiltinFn {
            name: "b".into(),
            arity: 0,
            func: |_| Ok(nuzo_core::Value::from_number(0.0)),
        });
        let (alloc, promote, reset) = gc.scratch_stats();
        assert_eq!(alloc, 2);
        assert_eq!(promote, 0);
        assert_eq!(reset, 0);
    }

    #[test]
    fn test_alloc_bulk() {
        let mut gc = Gc::with_default_threshold();
        let objs: Vec<HeapObject> = (0..5)
            .map(|i| HeapObject::Array(vec![nuzo_core::Value::from_number(i as f64)]))
            .collect();
        let indices = gc.alloc_bulk(objs);
        assert_eq!(indices.len(), 5);
    }

    #[test]
    fn test_alloc_uninit_commit_cycle() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc_uninit(32);
        assert!(!is_scratch(idx));
        gc.commit(idx, HeapObject::Box(nuzo_core::Value::from_number(77.0)));
        match gc.get(idx).expect("gc.get should succeed for valid index") {
            HeapObject::Box(v) => assert_eq!(*v, nuzo_core::Value::from_number(77.0)),
            other => panic!("expected Box, got {other:?}"),
        }
    }

    #[test]
    fn test_stats_consistency() {
        let mut gc = Gc::with_default_threshold();
        gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        gc.alloc(HeapObject::Dict(NuzoDict::new()));
        let s = gc.stats();
        assert_eq!(s.total_objects, 2);
        assert_eq!(s.live_objects, 2);
        assert!(s.allocated_bytes > 0);
    }

    #[test]
    fn test_clear_resets_state() {
        let mut gc = Gc::with_default_threshold();
        gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        assert_eq!(gc.len(), 1);
        gc.clear();
        assert!(gc.is_empty());
        assert_eq!(gc.deep_gc_count, 0);
    }

    #[test]
    fn test_mark_roots_sweep_cycle() {
        let mut gc = Gc::with_default_threshold();
        let keep = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(111.0)]));
        let _drop = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(999.0)]));
        gc.mark_roots(std::iter::once(nuzo_core::Value::from_gc_index(keep)));
        gc.sweep();
        assert!(gc.try_get(keep).is_some());
    }

    #[test]
    fn test_alloc_empty_bulk() {
        let mut gc = Gc::with_default_threshold();
        assert!(gc.alloc_bulk(Vec::new()).is_empty());
    }
    #[test]
    fn test_safe_point_no_scratch() {
        let mut gc = Gc::with_default_threshold();
        assert!(gc.safe_point(Vec::new).is_empty());
    }
    #[test]
    fn test_collect_with_roots_empty() {
        let mut gc = Gc::with_default_threshold();
        gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        gc.collect_with_roots(std::iter::empty::<nuzo_core::Value>());
        assert!(gc.is_empty() || gc.stats().free_slots > 0);
    }
    #[test]
    fn test_builder_pattern() {
        let gc = Gc::with_default_threshold().with_mark_rate(16).with_sweep_rate(32);
        assert_eq!(gc.len(), 0);
    }
    #[test]
    fn test_default_trait() {
        let _gc = Gc::default();
    }

    // v2 bitmap/cold/deep GC tests
    #[test]
    fn test_bitmap_set_is_marked_roundtrip() {
        let mut chunk = self::heap::Chunk::new_nursery();
        assert!(!chunk.is_marked(42));
        chunk.set_mark(42);
        assert!(chunk.is_marked(42));
    }
    #[test]
    fn test_bitmap_uninit_bits() {
        let mut chunk = self::heap::Chunk::new_nursery();
        chunk.set_uninit(10);
        assert!(chunk.is_uninit(10));
        chunk.clear_uninit(10);
        assert!(!chunk.is_uninit(10));
    }
    #[test]
    fn test_chunk_initial_cold_state() {
        let chunk = self::heap::Chunk::new_nursery();
        assert_eq!(chunk.age, 0);
        assert!(!chunk.is_cold);
    }
    #[test]
    fn test_clear_resets_deep_gc_count() {
        let mut gc = Gc::with_default_threshold();
        gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        gc.clear();
        assert_eq!(gc.deep_gc_count, 0);
    }
    #[test]
    fn test_bitmap_words_constant() {
        assert_eq!(self::heap::BITMAP_WORDS, 16);
    }
    #[test]
    fn test_sweep_resets_bitmap() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        gc.mark_roots(std::iter::once(nuzo_core::Value::from_gc_index(idx)));
        gc.sweep();
        assert!(gc.try_get(idx).is_some());
    }
    #[test]
    fn test_multiple_gc_cycles_increment_counts() {
        let mut gc = Gc::new(128);
        gc.register_roots_fn(Some(noop_roots), std::ptr::null_mut());
        for _ in 0..5 {
            gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
            gc.collect();
        }
        assert!(gc.minor_gc_count > 0);
    }
    #[test]
    fn test_sizes_u32_field() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(42.0); 100]));
        assert!(gc.try_get(idx).is_some());
        assert!(gc.stats().allocated_bytes > 0);
    }
    #[test]
    fn test_gc_with_default_threshold() {
        let gc = Gc::with_default_threshold();
        let cfg = nuzo_config::GcConfig::default();
        assert_eq!(gc.nursery_threshold, cfg.nursery_threshold.min(cfg.threshold));
    }
    #[test]
    fn test_gc_with_threshold_custom_value() {
        let gc = Gc::with_threshold(4096);
        assert_eq!(gc.nursery_threshold, 4096);
    }
    #[test]
    fn test_gc_with_mark_rate_builder() {
        let gc = Gc::with_default_threshold().with_mark_rate(16);
        assert_eq!(gc.mark_rate, 16);
    }
    #[test]
    fn test_gc_set_gc_threshold_clamps_to_min() {
        let mut gc = Gc::with_default_threshold();
        gc.set_gc_threshold(1);
        assert_eq!(gc.nursery_threshold, nuzo_core::GC_MIN_THRESHOLD);
    }
    // #[test] fn test_gc_alloc_scratch_returns_scratch_index() { let mut gc = Gc::with_default_threshold(); assert!(is_scratch(gc.alloc_scratch(HeapObject::Array(vec![nuzo_core::Value::from_number(7.0)]))); }
    #[test]
    fn test_is_scratch_boundary() {
        assert!(is_scratch(self::heap::SCRATCH_BASE));
        assert!(!is_scratch(0));
        assert!(!is_scratch(self::heap::SCRATCH_BASE - 1));
    }
    #[test]
    fn test_gc_alloc_with_size_basic() {
        let mut gc = Gc::with_default_threshold();
        let idx =
            gc.alloc_with_size(HeapObject::Array(vec![nuzo_core::Value::from_number(5.0)]), 8);
        match gc.get(idx).expect("gc.get should succeed for valid index") {
            HeapObject::Array(a) => assert_eq!(a[0], nuzo_core::Value::from_number(5.0)),
            other => panic!("expected Array, got {other:?}"),
        }
    }
    #[test]
    fn test_gc_promote_from_region_allocates() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc
            .promote_from_region(HeapObject::Array(vec![nuzo_core::Value::from_number(42.0)]), 16);
        assert!(!is_scratch(idx));
        match gc.get(idx).expect("gc.get should succeed for valid index") {
            HeapObject::Array(a) => assert_eq!(a[0], nuzo_core::Value::from_number(42.0)),
            other => panic!("expected Array, got {other:?}"),
        }
    }
    #[test]
    fn test_gc_mark_index_pushes_to_hot_stack() {
        let mut gc = Gc::with_default_threshold();
        let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(1.0)]));
        let before = gc.hot_stack.len();
        gc.mark_index(idx);
        assert_eq!(gc.hot_stack.len(), before + 1);
    }
    #[test]
    fn test_mark_index_ignores_heap_pool_index() {
        let mut gc = Gc::with_default_threshold();
        gc.mark_index((nuzo_values::constants::HEAP_POOL_INDEX_LIMIT as u32) / 2);
        assert!(
            gc.hot_stack.is_empty() && gc.cold_stack.is_empty(),
            "HEAP_POOL index must not be pushed to mark stacks"
        );
    }
    #[test]
    fn test_trace_skips_non_gc_heap_object() {
        let mut gc = Gc::with_default_threshold();
        let heap_val = unsafe { nuzo_core::Value::from_raw_bits(nuzo_core::tag::HEAP_TAG | 42) };
        assert!(heap_val.is_heap_object());
        assert!(!heap_val.is_gc_managed());
        gc.mark_roots(std::iter::once(heap_val));
        assert!(
            gc.hot_stack.is_empty() && gc.cold_stack.is_empty(),
            "HEAP_POOL object must not be traced as GC root"
        );
    }
    #[test]
    fn test_hot_stack_spills_to_cold_stack_when_full() {
        // Regression: mark_index uses deferred spill semantics — mark_index
        // only pushes to hot_stack (no spill); spill is triggered in
        // process_wave_front_step when hot_stack.len() >= HOT_STACK_CAP.
        //
        // This avoids spill overhead in mark_index (high-frequency call),
        // keeping mark-only workloads (G5) fast while still maintaining
        // hot_stack cache locality for deep graph traversal (G3) and
        // wide-tree traversal.
        //
        // 推演（HOT_STACK_CAP=128, SPILL_COUNT=64）：
        //   mark_index push 1..133 → hot=133, cold=0 (no spill in mark_index)
        //   process_wave_front_step:
        //     spill → hot=133-64=69, cold=64
        //     pop   → hot=68
        let mut gc = Gc::with_default_threshold();
        let cap = crate::gc::mark::HOT_STACK_CAP;
        const EXTRA: usize = 5;
        // Allocate enough objects to trigger spill in process_wave_front_step
        let mut indices = Vec::with_capacity(cap + EXTRA);
        for i in 0..(cap + EXTRA) as u32 {
            let idx = gc.alloc(HeapObject::Array(vec![nuzo_core::Value::from_number(i as f64)]));
            indices.push(idx);
        }
        // Phase 1: mark_index does NOT spill (deferred to process_wave_front_step)
        for idx in &indices {
            gc.mark_index(*idx);
        }
        assert_eq!(
            gc.hot_stack.len(),
            cap + EXTRA,
            "mark_index must not spill; hot_stack holds all pushed entries"
        );
        assert_eq!(
            gc.cold_stack.len(),
            0,
            "cold_stack must be empty before process_wave_front_step"
        );
        // Phase 2: process_wave_front_step triggers spill then pop
        gc.process_wave_front_step();
        // After spill: hot = 133-64 = 69; after pop: hot = 68
        assert_eq!(
            gc.hot_stack.len(),
            cap + EXTRA - 64 - 1,
            "hot_stack should be (cap + extra - spill - 1) after spill + pop"
        );
        assert_eq!(
            gc.cold_stack.len(),
            64,
            "cold_stack should hold SPILL_COUNT entries after spill"
        );
        // 所有条目必须保留（hot + cold == total pushed - 1 popped）
        assert_eq!(
            gc.hot_stack.len() + gc.cold_stack.len(),
            cap + EXTRA - 1,
            "total entries must be conserved (popped item excluded)"
        );
    }
    #[test]
    fn test_gc_register_roots_fn_sets_callback() {
        let mut gc = Gc::with_default_threshold();
        gc.register_roots_fn(Some(noop_roots), std::ptr::null_mut());
        gc.register_roots_fn(None, std::ptr::null_mut());
    }
    #[test]
    fn test_update_gc_chunks_ptr_updates_thread_local() {
        let gc = Gc::with_default_threshold();
        self::heap::update_gc_chunks_ptr(&gc);
        use self::heap::GC_CHUNKS_LEN;
        assert_eq!(GC_CHUNKS_LEN.with(|p| p.get()), gc.chunks.len());
    }
    #[test]
    fn test_gc_will_collect_signal_key_name() {
        assert!(!self::heap::GC_WILL_COLLECT_KEY.name().is_empty());
    }
    #[test]
    fn test_gc_did_collect_signal_key_name() {
        assert!(!self::heap::GC_DID_COLLECT_KEY.name().is_empty());
    }
    #[test]
    fn test_gc_signals_are_distinct() {
        assert_ne!(self::heap::GC_WILL_COLLECT_KEY.name(), self::heap::GC_DID_COLLECT_KEY.name());
    }
    #[test]
    fn test_gc_bus_has_signals_registered() {
        let gc = Gc::with_default_threshold();
        assert!(gc.bus().get(&self::heap::GC_WILL_COLLECT_KEY).is_ok());
        assert!(gc.bus().get(&self::heap::GC_DID_COLLECT_KEY).is_ok());
    }
    #[test]
    fn test_gc_bus_scope_is_gc() {
        let gc = Gc::with_default_threshold();
        assert_eq!(gc.bus().scope(), nuzo_signal::BusScope::Gc);
    }

    // ---- 栈溢出回归测试 ----
    // 回归：scratch_data 初始化必须用堆分配（repeat_with），不能用
    // Box::new(std::array::from_fn(|_| None))，否则 debug 模式下 352KB 栈临时
    // 变量 + 其他栈帧 > 2MB 默认栈会栈溢出（STATUS_STACK_OVERFLOW 0xC00000FD）。
    // 见 docs/fix-nuzo-vm-stack-overflow-and-doctests.md Phase D。

    #[test]
    fn test_scratch_data_init_no_stack_overflow_default_thread() {
        for _ in 0..10 {
            let _gc = Gc::with_default_threshold();
        }
        let _gc = Gc::with_threshold(4096);
        let _gc = Gc::new(8192);
    }

    #[test]
    fn test_scratch_data_init_in_multi_thread_default_stack() {
        use std::thread;
        let handles: Vec<_> = (0..4)
            .map(|_| {
                thread::spawn(|| {
                    for _ in 0..5 {
                        let _gc = Gc::with_default_threshold();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread panicked or stack overflowed");
        }
    }
}

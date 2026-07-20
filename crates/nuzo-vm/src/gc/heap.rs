//! # 堆访问层 (Heap Access Layer)
//!
//! 职责：Chunk 数据结构、位图操作、四路堆索引（scratch/pool/GC-chunks/Arena）、
//! 线程局部存储管理、scratch-aware 访问器安装、信号定义、公开工具函数。
//!
//! ## 包含类型与函数
//! - [`Chunk`] — GC 内存块（位图标记 + free list + generation/age/cold 元数据）
//! - [`GcStats`] — GC 统计信息
//! - [`ChunkInfo`] — Chunk 调试信息（供 [`super::Gc::chunk_info`] 返回）
//! - [`is_scratch()`] — 判断索引是否属于 scratch 区
//! - [`install_scratch_aware_accessors()`] — 安装四路索引堆访问器
//! - [`update_gc_chunks_ptr()`] — 更新线程局部 chunk 指针
//! - `GC_WILL_COLLECT_KEY` / `GC_DID_COLLECT_KEY` — GC 生命周期信号键（SignalBus 模式）

use std::cell::{Cell, UnsafeCell};

use nuzo_signal::{GcDidCollectInfo, GcWillCollectInfo, declare_signal};
use nuzo_values::{HeapObject, InternalError, NuzoError};

// ============================================================================
// 常量（pub(crate) 供所有 GC 子模块使用）
// ============================================================================

/// 位图常量
pub(crate) const BITS_PER_BITMAP_WORD: usize = 64;
pub(crate) const BITMAP_WORDS: usize = (1 << super::GC_CHUNK_SHIFT) / BITS_PER_BITMAP_WORD; // 1024 / 64 = 16

/// ERSA (Epoch-Reset Scratch Arena) 常量
pub(crate) const SCRATCH_CAP: usize = 4096;
pub(crate) const SCRATCH_BASE: u32 = 0x8000_0000;
pub(crate) const SCRATCH_MASK: u32 = SCRATCH_CAP as u32 - 1;

/// Chunk 相关常量
pub(crate) const NULL_PTR: u32 = u32::MAX;
pub(crate) const CHUNK_SHIFT: u32 = super::GC_CHUNK_SHIFT;
/// GC arena chunk size (number of slots per chunk). Distinct from
/// `object::SCOW_CHUNK_SIZE` (SCOW small-object chunk size, = 8).
pub(crate) const GC_CHUNK_SIZE: usize = 1 << CHUNK_SHIFT;
pub(crate) const CHUNK_MASK: u32 = GC_CHUNK_SIZE as u32 - 1;

/// 容量预分配常量
/// hot_stack 初始容量：mark::HOT_STACK_CAP (128) + SPILL_COUNT (64) = 192。
/// spill 语义下 mark_index 先 push 再判断 len >= HOT_STACK_CAP，所以
/// hot_stack 会瞬时达到 129 项再 spill 回 65 项。预分配 192 确保 push
/// 路径永不触发 realloc（fast path 零分配）。
pub(crate) const HOT_STACK_INITIAL_CAPACITY: usize = 192;
pub(crate) const COLD_STACK_INITIAL_CAPACITY: usize = 4096;
pub(crate) const CHUNK_VEC_INITIAL_CAPACITY: usize = 4;

// 注：Hot/Cold 栈分段常量（HOT_STACK_CAP、PREFETCH_DISTANCE）定义在
// `mark.rs` 中，因为它们是标记逻辑的一部分，而非堆数据层常量。

// ============================================================================
// 索引计算辅助函数
// ============================================================================

#[inline(always)]
pub(crate) fn chunk_id(idx: u32) -> usize {
    (idx >> CHUNK_SHIFT) as usize
}
#[inline(always)]
pub(crate) fn offset(idx: u32) -> usize {
    (idx & CHUNK_MASK) as usize
}

// ============================================================================
// 公开工具函数
// ============================================================================

/// 判断堆索引是否属于 scratch 划痕区（>= SCRATCH_BASE）。
#[inline(always)]
pub fn is_scratch(idx: u32) -> bool {
    idx >= SCRATCH_BASE
}

#[inline(always)]
pub(crate) fn scratch_off(idx: u32) -> usize {
    (idx & SCRATCH_MASK) as usize
}
#[inline(always)]
pub(crate) fn unlikely(b: bool) -> bool {
    b
}

// ============================================================================
// 线程局部存储（四路索引支持）
// ============================================================================

thread_local! {
    static SCRATCH_PTR: Cell<*const Option<UnsafeCell<HeapObject>>> = const { Cell::new(std::ptr::null()) };
    pub(crate) static GC_CHUNKS_PTR: Cell<*const Chunk> = const { Cell::new(std::ptr::null()) };
    pub(crate) static GC_CHUNKS_LEN: Cell<usize> = const { Cell::new(0) };
    static ARENA_PTR: Cell<*mut crate::arena::RegionAllocator> = const { Cell::new(std::ptr::null_mut()) };
}

fn scratch_aware_get(idx: u32) -> *const HeapObject {
    if (nuzo_values::constants::ARENA_BASE..nuzo_core::tag::SCRATCH_BASE).contains(&idx) {
        let arena_ptr = ARENA_PTR.with(|p| p.get());
        if !arena_ptr.is_null() {
            let offset = idx & nuzo_values::constants::ARENA_MASK;
            // SAFETY: arena_ptr is non-null and valid (installed by install_scratch_aware_accessors)
            unsafe {
                if let Some(obj) = (*arena_ptr).get_arena_object(offset) {
                    return obj as *const HeapObject;
                }
            }
        }
        return std::ptr::null();
    }
    if is_scratch(idx) {
        let base = SCRATCH_PTR.with(|p| p.get());
        if !base.is_null() {
            let off = scratch_off(idx);
            // SAFETY: base is non-null and valid; off < SCRATCH_CAP (masked by SCRATCH_MASK)
            unsafe {
                let slot = base.add(off);
                if let Some(ref cell) = *slot {
                    return cell.get() as *const HeapObject;
                }
            }
        }
        return std::ptr::null();
    }
    if idx < GC_CHUNK_SIZE as u32 {
        nuzo_values::value::default_heap_get(idx)
    } else {
        let chunks_ptr = GC_CHUNKS_PTR.with(|p| p.get());
        let chunks_len = GC_CHUNKS_LEN.with(|p| p.get());
        if !chunks_ptr.is_null() {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid < chunks_len {
                // SAFETY: chunks_ptr is valid and cid < chunks_len (checked above);
                // off < GC_CHUNK_SIZE (masked by CHUNK_MASK)
                unsafe {
                    let chunk = &*chunks_ptr.add(cid);
                    let data_slice = &*chunk.data.get();
                    if let Some(ref obj) = data_slice[off] {
                        return obj as *const HeapObject;
                    }
                }
            }
        }
        std::ptr::null()
    }
}

fn scratch_aware_get_mut(idx: u32) -> *mut HeapObject {
    if (nuzo_values::constants::ARENA_BASE..nuzo_core::tag::SCRATCH_BASE).contains(&idx) {
        let arena_ptr = ARENA_PTR.with(|p| p.get());
        if !arena_ptr.is_null() {
            let offset = idx & nuzo_values::constants::ARENA_MASK;
            // SAFETY: arena_ptr is non-null and valid (installed by install_scratch_aware_accessors)
            unsafe {
                if let Some(obj) = (*arena_ptr).get_arena_object_mut(offset) {
                    return obj as *mut HeapObject;
                }
            }
        }
        return std::ptr::null_mut();
    }
    if is_scratch(idx) {
        let base = SCRATCH_PTR.with(|p| p.get());
        if !base.is_null() {
            let off = scratch_off(idx);
            // SAFETY: base is non-null and valid; off < SCRATCH_CAP (masked by SCRATCH_MASK)
            unsafe {
                let slot = base.add(off);
                if let Some(ref cell) = *slot {
                    return cell.get();
                }
            }
        }
        return std::ptr::null_mut();
    }
    if idx < GC_CHUNK_SIZE as u32 {
        nuzo_values::value::default_heap_get_mut(idx)
    } else {
        let chunks_ptr = GC_CHUNKS_PTR.with(|p| p.get());
        let chunks_len = GC_CHUNKS_LEN.with(|p| p.get());
        if !chunks_ptr.is_null() {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid < chunks_len {
                // SAFETY: chunks_ptr is valid and cid < chunks_len (checked above);
                // off < GC_CHUNK_SIZE (masked by CHUNK_MASK)
                unsafe {
                    let chunk = &*chunks_ptr.add(cid);
                    let data_slice = &mut *chunk.data.get();
                    if let Some(ref mut obj) = data_slice[off] {
                        return obj as *mut HeapObject;
                    }
                }
            }
        }
        std::ptr::null_mut()
    }
}

thread_local! {
    static GC_HEAP_GC_PTR: Cell<*mut super::Gc> = const { Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn set_gc_heap_gc_ptr(gc: *mut super::Gc) {
    GC_HEAP_GC_PTR.with(|cell| cell.set(gc));
}

pub(crate) fn clear_gc_heap_gc_ptr() {
    GC_HEAP_GC_PTR.with(|cell| cell.set(std::ptr::null_mut()));
}

/// GC-backed allocator used by `Value::from_heap_object_gc` when a VM is active.
/// Falls back to the default HEAP_POOL if no VM is installed on this thread.
pub(crate) fn gc_heap_alloc(obj: HeapObject) -> u32 {
    GC_HEAP_GC_PTR.with(|ptr| {
        let gc_ptr = ptr.get();
        if gc_ptr.is_null() {
            return nuzo_values::value::default_heap_alloc(obj);
        }
        // SAFETY: gc_ptr is non-null (checked above); valid Gc pointer installed by set_gc_heap_gc_ptr
        unsafe { &mut *gc_ptr }.alloc(obj)
    })
}

pub(crate) fn install_scratch_aware_accessors(
    scratch_base: *const Option<UnsafeCell<HeapObject>>,
    arena: *mut crate::arena::RegionAllocator,
) {
    SCRATCH_PTR.with(|p| p.set(scratch_base));
    ARENA_PTR.with(|p| p.set(arena));
    nuzo_values::register_heap_accessors(
        nuzo_values::value::default_heap_alloc,
        scratch_aware_get,
        scratch_aware_get_mut,
        None,
    );
    nuzo_values::register_gc_heap_alloc(gc_heap_alloc);
}

pub(crate) fn update_gc_chunks_ptr(gc: &super::Gc) {
    GC_CHUNKS_PTR.with(|p| p.set(gc.chunks.as_ptr()));
    GC_CHUNKS_LEN.with(|p| p.set(gc.chunks.len()));
}

declare_signal!(GC_WILL_COLLECT_KEY, GcWillCollectInfo, nuzo_signal::BusScope::Gc);
declare_signal!(GC_DID_COLLECT_KEY, GcDidCollectInfo, nuzo_signal::BusScope::Gc);

#[derive(Debug, Clone)]
pub struct GcStats {
    pub total_objects: usize,
    pub live_objects: usize,
    pub dead_objects: usize,
    pub free_slots: usize,
    pub allocated_bytes: usize,
    pub threshold: usize,
}

impl std::fmt::Display for GcStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GcStats {{ total={}, live={}, dead={}, free={}, bytes={}, threshold={} }}",
            self.total_objects,
            self.live_objects,
            self.dead_objects,
            self.free_slots,
            self.allocated_bytes,
            self.threshold
        )
    }
}

#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub top: u32,
    pub alive_count: u32,
    pub free_count: u32,
    pub is_active: bool,
    pub generation: u8,
    pub is_dirty: bool,
    pub age: u8,
    pub is_cold: bool,
}

#[repr(align(64))]
pub(crate) struct Chunk {
    pub(crate) mark_bits: Box<[u64; BITMAP_WORDS]>,
    pub(crate) uninit_bits: Box<[u64; BITMAP_WORDS]>,
    pub(crate) micro_dirty_bits: Box<[u64; BITMAP_WORDS]>,
    pub(crate) next_frees: Box<[u32; GC_CHUNK_SIZE]>,
    pub(crate) sizes: Box<[u32; GC_CHUNK_SIZE]>,
    pub(crate) data: UnsafeCell<Box<[Option<HeapObject>; GC_CHUNK_SIZE]>>,
    pub(crate) top: u32,
    pub(crate) alive_count: u32,
    pub(crate) free_list: u32,
    pub(crate) free_count: u32,
    pub(crate) generation: u8,
    pub(crate) is_dirty: bool,
    pub(crate) age: u8,
    pub(crate) is_cold: bool,
    pub(crate) last_mark_epoch: u8,
}

impl Chunk {
    #[inline]
    pub(crate) fn new_nursery() -> Self {
        Self {
            mark_bits: vec![0; BITMAP_WORDS].into_boxed_slice().try_into().expect("BITMAP_WORDS"),
            uninit_bits: vec![0; BITMAP_WORDS].into_boxed_slice().try_into().expect("BITMAP_WORDS"),
            micro_dirty_bits: vec![0; BITMAP_WORDS]
                .into_boxed_slice()
                .try_into()
                .expect("BITMAP_WORDS"),
            next_frees: vec![NULL_PTR; GC_CHUNK_SIZE]
                .into_boxed_slice()
                .try_into()
                .expect("GC_CHUNK_SIZE"),
            sizes: vec![0; GC_CHUNK_SIZE].into_boxed_slice().try_into().expect("GC_CHUNK_SIZE"),
            data: UnsafeCell::new(
                (0..GC_CHUNK_SIZE)
                    .map(|_| None)
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
                    .try_into()
                    .expect("GC_CHUNK_SIZE"),
            ),
            top: 0,
            alive_count: 0,
            free_list: NULL_PTR,
            free_count: 0,
            generation: 0,
            is_dirty: false,
            age: 0,
            is_cold: false,
            last_mark_epoch: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn set_mark(&mut self, off: usize) {
        self.mark_bits[off >> 6] |= 1 << (off & 63);
    }

    #[inline(always)]
    pub(crate) fn is_marked(&self, off: usize) -> bool {
        (self.mark_bits[off >> 6] & (1 << (off & 63))) != 0
    }

    #[inline(always)]
    pub(crate) fn set_uninit(&mut self, off: usize) {
        self.uninit_bits[off >> 6] |= 1 << (off & 63);
    }

    #[inline(always)]
    pub(crate) fn clear_uninit(&mut self, off: usize) {
        self.uninit_bits[off >> 6] &= !(1 << (off & 63));
    }

    #[inline(always)]
    pub(crate) fn is_uninit(&self, off: usize) -> bool {
        (self.uninit_bits[off >> 6] & (1 << (off & 63))) != 0
    }

    #[inline(always)]
    pub(crate) fn set_micro_dirty(&mut self, off: usize) {
        self.micro_dirty_bits[off >> 6] |= 1 << (off & 63);
    }

    #[inline]
    pub(crate) fn clear_micro_dirty(&mut self) {
        self.micro_dirty_bits.fill(0);
    }

    #[inline(always)]
    #[allow(dead_code)] // GC 调试 API，保留供写屏障验证使用
    pub(crate) fn has_micro_dirty(&self) -> bool {
        self.micro_dirty_bits.iter().any(|&w| w != 0)
    }
}

use super::Gc;

impl Gc {
    #[inline(always)]
    pub fn get(&self, idx: u32) -> Result<&HeapObject, NuzoError> {
        if is_scratch(idx) {
            let off = scratch_off(idx);
            if off >= self.scratch_top as usize {
                return Err(heap_not_found(idx));
            }
            // SAFETY: off < scratch_top (checked above); scratch_data has SCRATCH_CAP elements
            unsafe {
                self.scratch_data
                    .get_unchecked(off)
                    .as_ref()
                    .ok_or_else(|| heap_not_found(idx))
                    .map(|c| &*c.get())
            }
        } else {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid >= self.chunks.len() {
                return Err(heap_not_found(idx));
            }
            // SAFETY: cid < self.chunks.len() (checked above)
            let data_slice = unsafe { &*self.chunks.get_unchecked(cid).data.get() };
            // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK)
            unsafe { data_slice.get_unchecked(off) }.as_ref().ok_or_else(|| heap_not_found(idx))
        }
    }

    #[inline(always)]
    pub fn get_mut(&mut self, idx: u32) -> Result<&mut HeapObject, NuzoError> {
        if is_scratch(idx) {
            let off = scratch_off(idx);
            if off >= self.scratch_top as usize {
                return Err(heap_not_found(idx));
            }
            // SAFETY: off < scratch_top (checked above); scratch_data has SCRATCH_CAP elements
            unsafe {
                self.scratch_data
                    .get_unchecked_mut(off)
                    .as_mut()
                    .ok_or_else(|| heap_not_found(idx))
                    .map(|c| &mut *c.get())
            }
        } else {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid >= self.chunks.len() {
                return Err(heap_not_found(idx));
            }
            // SAFETY: cid < self.chunks.len() (checked above)
            let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
            if chunk.generation == 1 || chunk.generation == 2 {
                chunk.is_dirty = true;
                chunk.set_micro_dirty(off);
            }
            // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
            let data_slice = unsafe { &mut *chunk.data.get() };
            // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK)
            unsafe { data_slice.get_unchecked_mut(off) }.as_mut().ok_or_else(|| heap_not_found(idx))
        }
    }

    #[inline]
    pub fn get_mut_if_present(&mut self, idx: u32) -> Option<&mut HeapObject> {
        if is_scratch(idx) {
            let off = scratch_off(idx);
            if off >= self.scratch_top as usize {
                return None;
            }
            // SAFETY: off < scratch_top (checked above); scratch_data has SCRATCH_CAP elements
            unsafe { self.scratch_data.get_unchecked_mut(off).as_mut().map(|c| &mut *c.get()) }
        } else {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid >= self.chunks.len() {
                return None;
            }
            // SAFETY: cid < self.chunks.len() (checked above)
            let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
            if chunk.generation >= 1 {
                chunk.is_dirty = true;
                chunk.set_micro_dirty(off);
            }
            // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
            let data_slice = unsafe { &mut *chunk.data.get() };
            // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK)
            unsafe { data_slice.get_unchecked_mut(off) }.as_mut()
        }
    }

    #[inline(always)]
    pub fn try_get(&self, idx: u32) -> Option<&HeapObject> {
        if is_scratch(idx) {
            let off = scratch_off(idx);
            if off >= self.scratch_top as usize {
                return None;
            }
            // SAFETY: off < scratch_top (checked above); scratch_data has SCRATCH_CAP elements
            unsafe { self.scratch_data.get_unchecked(off).as_ref().map(|c| &*c.get()) }
        } else {
            let cid = chunk_id(idx);
            let off = offset(idx);
            if cid >= self.chunks.len() {
                return None;
            }
            // SAFETY: cid < self.chunks.len() (checked above)
            let data_slice = unsafe { &*self.chunks.get_unchecked(cid).data.get() };
            // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK)
            unsafe { data_slice.get_unchecked(off) }.as_ref()
        }
    }
}

#[cold]
#[inline(never)]
fn heap_not_found(idx: u32) -> NuzoError {
    NuzoError::internal(InternalError::HeapObjectNotFound { idx }, None)
}

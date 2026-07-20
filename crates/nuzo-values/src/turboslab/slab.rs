//! TurboSlab：连续内存页 + 位图
//!
//! 位图语义：1 = 空闲，0 = 占用。
//! 每个 slab 对应一个大小类（SizeClass），内部对象等距排列，
//! 通过 trailing_zeros 标量扫描定位空闲槽位（保留 SIMD 扩展点）。

use std::alloc::{self, Layout};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

use super::RemoteFreeNode;

const PAGE_SIZE: usize = 4096;

/// 大小类：按 HeapObject 变体分桶
#[derive(Clone, Copy, Debug)]
pub struct SizeClass {
    /// 该类对象大小（含 Arc 头等元数据）
    pub object_size: usize,
    /// 每 slab 容纳对象数量
    pub objects_per_slab: usize,
    /// slab 总大小（页对齐，4096 倍数）
    pub slab_size: usize,
}

impl SizeClass {
    /// 构造一个大小类，自动将 slab_size 向上取整到页大小倍数
    pub fn new(object_size: usize, objects_per_slab: usize) -> Self {
        assert!(object_size > 0, "object_size must be > 0");
        assert!(objects_per_slab > 0, "objects_per_slab must be > 0");
        let raw_size = object_size * objects_per_slab;
        let slab_size = raw_size.next_multiple_of(PAGE_SIZE);
        Self { object_size, objects_per_slab, slab_size }
    }
}

/// Slab：连续内存页 + 位图
pub struct TurboSlab {
    base: *mut u8,
    layout: Layout,
    bitmap: Vec<u64>,
    free_count: usize,
    owner_cpu: usize,
    remote_free_head: AtomicPtr<RemoteFreeNode>,
    #[allow(dead_code)] // T4 共享 partial 链表读取
    next: AtomicPtr<TurboSlab>,
    #[allow(dead_code)] // NUMA 扩展点
    numa_node: Option<u8>,
    #[allow(dead_code)] // 缓存着色扩展点
    color_offset: usize,
    object_size: usize,
    objects_per_slab: usize,
    /// 在所属 TurboCache::all_slabs 中的索引，用于缓存内扁平地址映射。
    slab_index: usize,
}

// SAFETY: TurboSlab only contains raw pointers and atomics that are safe to send
// across threads. The pointer fields (base, remote_free_head, next) are only
// accessed through &self methods with proper atomic synchronization. The bitmap
// and free_count are only mutated through &mut self, so no data races occur.
unsafe impl Send for TurboSlab {}
// SAFETY: All shared mutable state is protected by atomic operations
// (remote_free_head uses CAS, next uses atomic load/store). The bitmap and
// free_count are only mutated through &mut self, which guarantees exclusive
// access. The base pointer is never mutated after construction.
unsafe impl Sync for TurboSlab {}

impl TurboSlab {
    /// 分配一页对齐的连续内存，初始化位图全为 1（空闲）。
    pub fn new(size_class: &SizeClass, owner_cpu: usize) -> Self {
        let align = PAGE_SIZE;
        let size = size_class.slab_size.next_multiple_of(align);
        let layout = Layout::from_size_align(size, align).expect("slab layout overflow");
        // SAFETY: layout is constructed from size_class.slab_size (which is a
        // multiple of PAGE_SIZE, guaranteed > 0 by SizeClass::new assertions)
        // and PAGE_SIZE alignment. Layout::from_size_align has already verified
        // the layout is valid. The returned pointer is checked for null below.
        let base = unsafe { alloc::alloc(layout) };
        if base.is_null() {
            alloc::handle_alloc_error(layout);
        }

        let bitmap_len = size_class.objects_per_slab.div_ceil(64);
        let mut bitmap = vec![u64::MAX; bitmap_len];

        // 最后一个 word 中超出 objects_per_slab 的位必须置 0（占用），
        // 防止 allocate() 返回越界索引。
        let used_bits = size_class.objects_per_slab % 64;
        if used_bits != 0 {
            let mask = (1u64 << used_bits) - 1;
            bitmap[bitmap_len - 1] = mask;
        }

        Self {
            base,
            layout,
            bitmap,
            free_count: size_class.objects_per_slab,
            owner_cpu,
            remote_free_head: AtomicPtr::new(null_mut()),
            next: AtomicPtr::new(null_mut()),
            numa_node: None,
            color_offset: 0,
            object_size: size_class.object_size,
            objects_per_slab: size_class.objects_per_slab,
            slab_index: 0,
        }
    }

    /// 设置 slab 在所属 TurboCache::all_slabs 中的索引。
    pub fn set_slab_index(&mut self, idx: usize) {
        self.slab_index = idx;
    }

    /// 返回 slab 在所属 TurboCache::all_slabs 中的索引。
    pub fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// 将指定索引标记为占用（0），不查找空闲位。
    ///
    /// # Panics
    /// - idx 越界
    /// - 对应位已经是占用状态
    pub fn mark_occupied(&mut self, idx: u32) {
        let idx = idx as usize;
        assert!(idx < self.objects_per_slab, "mark_occupied out of bounds");
        let word_idx = idx / 64;
        let bit_pos = idx % 64;
        let mask = 1u64 << bit_pos;
        assert!(self.bitmap[word_idx] & mask != 0, "mark_occupied on already-occupied slot");
        self.bitmap[word_idx] &= !mask;
        self.free_count -= 1;
    }

    /// 从位图中找到第一个空闲位，标记为占用（0），返回对象索引。
    pub fn allocate(&mut self) -> Option<u32> {
        for (word_idx, word) in self.bitmap.iter_mut().enumerate() {
            if *word == 0 {
                continue;
            }
            let bit_pos = word.trailing_zeros();
            let idx = word_idx * 64 + bit_pos as usize;
            if idx >= self.objects_per_slab {
                continue;
            }
            *word &= !(1u64 << bit_pos);
            self.free_count -= 1;
            return Some(idx as u32);
        }
        None
    }

    /// 将指定索引对应的位图位置 1（空闲），并增加空闲计数。
    pub fn deallocate(&mut self, idx: u32) {
        let idx = idx as usize;
        assert!(idx < self.objects_per_slab, "deallocate out of bounds");
        let word_idx = idx / 64;
        let bit_pos = idx % 64;
        let mask = 1u64 << bit_pos;
        self.bitmap[word_idx] |= mask;
        self.free_count += 1;
    }

    /// 指定位移是否已被占用。
    pub fn is_occupied(&self, idx: u32) -> bool {
        let idx = idx as usize;
        assert!(idx < self.objects_per_slab, "is_occupied out of bounds");
        let word_idx = idx / 64;
        let bit_pos = idx % 64;
        (self.bitmap[word_idx] & (1u64 << bit_pos)) == 0
    }

    /// 返回指定索引对应的内存指针：base + idx * object_size。
    pub fn object_ptr(&self, idx: u32) -> *mut u8 {
        let idx = idx as usize;
        assert!(idx < self.objects_per_slab, "object_ptr out of bounds");
        // SAFETY: base is allocated with layout for objects_per_slab * object_size
        // bytes. idx is validated to be < objects_per_slab, so
        // base + idx * object_size is within the allocated region.
        unsafe { self.base.add(idx * self.object_size) }
    }

    /// 返回当前已占用对象的索引迭代器（供 reclaim 扫描）。
    pub fn occupied_indices(&self) -> impl Iterator<Item = u32> + '_ {
        OccupiedIndicesIter { slab: self, word_idx: 0, bits: 0 }
    }

    /// 当前空闲对象数量。
    pub fn free_count(&self) -> usize {
        self.free_count
    }

    /// 归属 CPU 编号。
    pub fn owner_cpu(&self) -> usize {
        self.owner_cpu
    }

    /// 将远程释放节点原子地压入本 slab 的 Treiber 栈。
    ///
    /// 使用 CAS 循环处理并发竞争与伪失败。`node.next` 会指向当前 head。
    /// 成功返回 `true`。
    ///
    /// # Safety
    /// - `node` 必须是由 `Box::into_raw` 产生的有效且独占的指针。
    pub unsafe fn push_remote_free(&self, node: *mut RemoteFreeNode) -> bool {
        // SAFETY: caller guarantees node is a valid pointer produced by
        // Box::into_raw, so dereferencing to read the next field is safe.
        let node_ref = unsafe { &*node };
        loop {
            let head = self.remote_free_head.load(Ordering::Acquire);
            node_ref.next.store(head, Ordering::Relaxed);
            match self.remote_free_head.compare_exchange_weak(
                head,
                node,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// 从本 slab 的远程释放栈中弹出一个节点。
    ///
    /// 使用 CAS 循环保证多核并发安全。弹出后调用方必须负责通过
    /// `Box::from_raw` 回收节点内存。
    ///
    /// # Safety
    ///
    /// 调用方必须确保：
    /// - `self` 的生命周期内的 slab 内存有效
    /// - 返回的裸指针由 `Box::from_raw` 正确回收，避免内存泄漏
    pub unsafe fn pop_remote_free(&self) -> Option<*mut RemoteFreeNode> {
        loop {
            let head = self.remote_free_head.load(Ordering::Acquire);
            if head.is_null() {
                return None;
            }
            // SAFETY: head was loaded from remote_free_head with Acquire
            // ordering, which is only ever set to a valid pointer (or null)
            // by push_remote_free via CAS. If non-null, head points to a
            // RemoteFreeNode that is still in the stack.
            let next = unsafe { (*head).next.load(Ordering::Relaxed) };
            match self.remote_free_head.compare_exchange_weak(
                head,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(head),
                Err(_) => continue,
            }
        }
    }

    /// 内存基址（调试用）。
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    /// 对象大小。
    pub fn object_size(&self) -> usize {
        self.object_size
    }

    /// 每 slab 对象上限。
    pub fn objects_per_slab(&self) -> usize {
        self.objects_per_slab
    }
}

impl Drop for TurboSlab {
    fn drop(&mut self) {
        if !self.base.is_null() {
            // SAFETY: self.base was allocated with self.layout in new(),
            // and has not been modified since. Drop is called only once,
            // so this dealloc is safe. After dealloc, we null out the pointer
            // to prevent double-free.
            unsafe { alloc::dealloc(self.base, self.layout) };
            self.base = null_mut();
        }
    }
}

struct OccupiedIndicesIter<'a> {
    slab: &'a TurboSlab,
    word_idx: usize,
    bits: u64,
}

impl<'a> Iterator for OccupiedIndicesIter<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        loop {
            if self.bits == 0 {
                if self.word_idx >= self.slab.bitmap.len() {
                    return None;
                }
                let word_start = self.word_idx * 64;
                let valid_count = if word_start + 64 > self.slab.objects_per_slab {
                    self.slab.objects_per_slab - word_start
                } else {
                    64
                };
                if valid_count == 0 {
                    return None;
                }
                let valid_mask =
                    if valid_count == 64 { u64::MAX } else { (1u64 << valid_count) - 1 };
                // 占用位为 0，取反后得到占用位的 1-mask。
                self.bits = (!self.slab.bitmap[self.word_idx]) & valid_mask;
                self.word_idx += 1;
            }
            if self.bits == 0 {
                continue;
            }
            let bit_pos = self.bits.trailing_zeros();
            let idx = (self.word_idx - 1) * 64 + bit_pos as usize;
            self.bits &= self.bits - 1; // 清除最低 set bit
            return Some(idx as u32);
        }
    }
}

/// 标量位图扫描：将所有空闲对象索引压入本地栈。
///
/// SIMD 扩展点：当 profile 证明此处为热点时，可将 `trailing_zeros()` 循环
/// 替换为 AVX2 `_mm256_tzcnt_64` 或等效向量化扫描，但保留相同的
/// `1=空闲 / 0=占用` 语义与输出顺序约束。
pub fn fill_local_stack_from_bitmap(slab: &TurboSlab, stack: &mut Vec<u32>) {
    for (word_idx, &word) in slab.bitmap.iter().enumerate() {
        if word == 0 {
            continue; // 全占用，无空闲位
        }
        let word_start = word_idx * 64;
        let valid_count = if word_start + 64 > slab.objects_per_slab {
            slab.objects_per_slab - word_start
        } else {
            64
        };
        if valid_count == 0 {
            break;
        }
        let valid_mask = if valid_count == 64 { u64::MAX } else { (1u64 << valid_count) - 1 };
        let mut bits = word & valid_mask;
        while bits != 0 {
            let bit_pos = bits.trailing_zeros(); // SIMD 扩展点
            let obj_idx = word_idx * 64 + bit_pos as usize;
            stack.push(obj_idx as u32);
            bits &= bits - 1; // 清除最低空闲位
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slab_alloc_dealloc() {
        let sc = SizeClass::new(64, 128);
        let mut slab = TurboSlab::new(&sc, 0);

        let idx = slab.allocate().expect("first alloc should succeed");
        assert_eq!(idx, 0);
        assert!(slab.is_occupied(idx));
        assert_eq!(slab.free_count(), 127);

        slab.deallocate(idx);
        assert!(!slab.is_occupied(idx));
        assert_eq!(slab.free_count(), 128);

        // 再次分配应复用索引 0
        let idx2 = slab.allocate().expect("reuse should succeed");
        assert_eq!(idx2, 0);
    }

    #[test]
    fn test_bitmap_scan() {
        // 干净 slab：fill_local_stack_from_bitmap 应返回全部空闲索引
        let sc = SizeClass::new(16, 70);
        let slab = TurboSlab::new(&sc, 0);
        let mut stack = Vec::new();
        fill_local_stack_from_bitmap(&slab, &mut stack);
        assert_eq!(stack.len(), 70);
        for (i, &idx) in stack.iter().enumerate() {
            assert_eq!(idx as usize, i);
        }

        // 占用 0、1，释放 2：扫描结果只应包含剩余空闲索引
        let mut slab2 = TurboSlab::new(&SizeClass::new(16, 8), 0);
        let _ = slab2.allocate().unwrap(); // 0
        let _ = slab2.allocate().unwrap(); // 1
        let idx2 = slab2.allocate().unwrap(); // 2
        slab2.deallocate(idx2); // free 2

        let mut stack2 = Vec::new();
        fill_local_stack_from_bitmap(&slab2, &mut stack2);
        assert_eq!(stack2, vec![2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_slab_full() {
        let sc = SizeClass::new(16, 8);
        let mut slab = TurboSlab::new(&sc, 0);

        let mut indices = Vec::new();
        for _ in 0..8 {
            indices.push(slab.allocate().expect("should allocate until full"));
        }
        assert_eq!(indices.len(), 8);
        assert!(slab.allocate().is_none(), "slab should be full");

        // 释放一个后应能再分配
        slab.deallocate(indices[3]);
        let idx = slab.allocate().expect("should allocate after free");
        assert_eq!(idx, indices[3]);
    }

    #[test]
    fn test_drop_dealloc() {
        let base = {
            let sc = SizeClass::new(64, 16);
            let mut slab = TurboSlab::new(&sc, 0);
            for _ in 0..8 {
                let idx = slab.allocate().unwrap();
                assert!(slab.is_occupied(idx));
            }
            assert!(!slab.base().is_null());
            slab.base()
        };
        // 出作用域后 Drop 被调用；这里只能确认无 panic，
        // 实际 dealloc 由 std::alloc 保证。
        assert!(!base.is_null());
    }

    #[test]
    fn test_occupied_indices() {
        let sc = SizeClass::new(16, 70);
        let mut slab = TurboSlab::new(&sc, 0);

        let a = slab.allocate().unwrap();
        let b = slab.allocate().unwrap();
        let c = slab.allocate().unwrap();
        slab.deallocate(b);

        let occupied: Vec<u32> = slab.occupied_indices().collect();
        assert_eq!(occupied, vec![0, 2]);

        slab.deallocate(a);
        slab.deallocate(c);
        assert!(slab.occupied_indices().next().is_none());
    }

    #[test]
    fn test_object_ptr_arithmetic() {
        let sc = SizeClass::new(64, 16);
        let slab = TurboSlab::new(&sc, 0);
        let base = slab.base();
        for i in 0..16u32 {
            let ptr = slab.object_ptr(i);
            let expected = unsafe { base.add(i as usize * 64) };
            assert_eq!(ptr, expected);
        }
    }
}

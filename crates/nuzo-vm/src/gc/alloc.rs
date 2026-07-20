//! # 分配器 (Allocator)
//! 职责：GC 管理内存的分配路径，包括 bump allocation、free list 复用、
//! 慢路径 GC 触发、scratch arena 分配、Arena 逃逸提升。

use crate::gc::Gc;
use crate::gc::heap::{
    Chunk, GC_CHUNK_SIZE, NULL_PTR, SCRATCH_BASE, SCRATCH_CAP, chunk_id, is_scratch, offset,
    scratch_off, unlikely,
};
use nuzo_values::HeapObject;

impl Gc {
    #[inline(always)]
    pub fn alloc(&mut self, obj: HeapObject) -> u32 {
        let size = obj.size_estimate();
        self.alloc_with_size(obj, size)
    }

    #[inline(always)]
    pub fn alloc_with_size(&mut self, obj: HeapObject, size: usize) -> u32 {
        self.pace_incremental();
        self.bytes_until_gc -= size as isize;
        if self.bytes_until_gc <= 0 {
            return self.alloc_slow_path(obj, size);
        }
        self.alloc_bump_fast(obj, size)
    }

    pub fn alloc_bulk(&mut self, objects: Vec<HeapObject>) -> Vec<u32> {
        if objects.is_empty() {
            return Vec::new();
        }
        self.pace_incremental();
        let total_size: usize = objects.iter().map(|o| o.size_estimate()).sum();
        self.bytes_until_gc -= total_size as isize;
        if self.bytes_until_gc <= 0 {
            self.collect();
            self.bytes_until_gc = self.nursery_threshold as isize;
        }
        let mut indices = Vec::with_capacity(objects.len());
        for obj in objects {
            let size = obj.size_estimate();
            indices.push(self.alloc_bump_fast(obj, size));
        }
        indices
    }

    pub fn alloc_uninit(&mut self, size: usize) -> u32 {
        debug_assert!(
            size <= u32::MAX as usize,
            "GC uninit slot size {} exceeds u32::MAX (would truncate on storage)",
            size
        );
        self.pace_incremental();
        self.bytes_until_gc -= size as isize;
        if self.bytes_until_gc <= 0 {
            self.collect();
            self.bytes_until_gc = self.nursery_threshold as isize;
        }
        self.alloc_uninit_bump(size)
    }

    pub fn commit(&mut self, idx: u32, obj: HeapObject) {
        let cid = chunk_id(idx);
        let off = offset(idx);
        assert!(cid < self.chunks.len(), "commit: invalid chunk index {cid}");
        // SAFETY: cid < self.chunks.len() asserted above
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        assert!(chunk.is_uninit(off), "commit: slot {idx} is not in uninit state");
        // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
        let data_slice = unsafe { &mut *chunk.data.get() };
        // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK); slot is uninit
        unsafe {
            *data_slice.get_unchecked_mut(off) = Some(obj);
        }
        chunk.clear_uninit(off);
        chunk.set_mark(off);
    }

    #[inline(always)]
    fn alloc_bump_fast(&mut self, obj: HeapObject, size: usize) -> u32 {
        let cid = self.active_chunk;
        // SAFETY: active_chunk is always a valid chunk index (set during init and alloc_slow_path_inner)
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        if chunk.free_list == NULL_PTR && chunk.top < GC_CHUNK_SIZE as u32 {
            let off = chunk.top as usize;
            chunk.top += 1;
            chunk.alive_count += 1;
            let global_idx = ((cid as u32) << crate::gc::heap::CHUNK_SHIFT) | (off as u32);
            // SAFETY: off = old chunk.top < GC_CHUNK_SIZE (checked above);
            // all get_unchecked_mut indices are off < GC_CHUNK_SIZE
            unsafe {
                debug_assert!(global_idx < SCRATCH_BASE);
                chunk.set_mark(off);
                chunk.last_mark_epoch = self.mark_epoch;
                *chunk.sizes.get_unchecked_mut(off) = size as u32;
                let data_slice = &mut *chunk.data.get();
                *data_slice.get_unchecked_mut(off) = Some(obj);
                *chunk.next_frees.get_unchecked_mut(off) = NULL_PTR;
                self.allocated_bytes += size;
                self.nursery_bytes += size;
                return global_idx;
            }
        }
        self.alloc_slow_path_inner(obj, size, cid)
    }

    #[cold]
    #[inline(never)]
    fn alloc_slow_path(&mut self, obj: HeapObject, size: usize) -> u32 {
        self.collect();
        self.bytes_until_gc = self.nursery_threshold as isize;
        self.alloc_bump_fast(obj, size)
    }

    #[cold]
    #[inline(never)]
    fn alloc_slow_path_inner(&mut self, obj: HeapObject, size: usize, mut cid: usize) -> u32 {
        // SAFETY: cid is active_chunk (valid) or a searched valid chunk index
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        if chunk.free_list != NULL_PTR {
            let off = chunk.free_list as usize;
            // SAFETY: off comes from free_list, which only contains valid slot offsets
            let next = unsafe { *chunk.next_frees.get_unchecked(off) };
            chunk.free_list = next;
            chunk.free_count = chunk.free_count.saturating_sub(1);
            self.free_count = self.free_count.saturating_sub(1);
            chunk.alive_count += 1;
            let global_idx = ((cid as u32) << crate::gc::heap::CHUNK_SHIFT) | (off as u32);
            // SAFETY: off < GC_CHUNK_SIZE (free_list slots are always valid offsets)
            unsafe {
                chunk.set_mark(off);
                chunk.last_mark_epoch = self.mark_epoch;
                *chunk.sizes.get_unchecked_mut(off) = size as u32;
                let data_slice = &mut *chunk.data.get();
                *data_slice.get_unchecked_mut(off) = Some(obj);
                *chunk.next_frees.get_unchecked_mut(off) = NULL_PTR;
                self.allocated_bytes += size;
                if chunk.generation == 0 {
                    self.nursery_bytes += size;
                } else {
                    self.tenured_bytes += size;
                }
                return global_idx;
            }
        }
        let len = self.chunks.len();
        let mut found = false;
        for i in 1..=len {
            let check_id = (cid + i) % len;
            if check_id == 0 {
                continue;
            }
            let c = &self.chunks[check_id];
            if c.generation == 0 && (c.free_list != NULL_PTR || c.top < GC_CHUNK_SIZE as u32) {
                cid = check_id;
                found = true;
                break;
            }
        }
        if !found {
            self.chunks.push(Chunk::new_nursery());
            cid = self.chunks.len() - 1;
            self.refresh_chunk_pointers();
        }
        self.active_chunk = cid;
        self.alloc_bump_fast(obj, size)
    }

    fn alloc_uninit_bump(&mut self, size: usize) -> u32 {
        let cid = self.active_chunk;
        // SAFETY: active_chunk is always a valid chunk index
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        if chunk.free_list == NULL_PTR && chunk.top < GC_CHUNK_SIZE as u32 {
            let off = chunk.top as usize;
            chunk.top += 1;
            chunk.alive_count += 1;
            // SAFETY: off = old chunk.top < GC_CHUNK_SIZE (checked above);
            // all get_unchecked_mut indices are off < GC_CHUNK_SIZE
            unsafe {
                let global_idx = ((cid as u32) << crate::gc::heap::CHUNK_SHIFT) | (off as u32);
                chunk.set_uninit(off);
                *chunk.sizes.get_unchecked_mut(off) = size as u32;
                let data_slice = &mut *chunk.data.get();
                *data_slice.get_unchecked_mut(off) = None;
                *chunk.next_frees.get_unchecked_mut(off) = NULL_PTR;
                self.allocated_bytes += size;
                self.nursery_bytes += size;
                return global_idx;
            }
        }
        self.alloc_uninit_slow(size, cid)
    }

    #[cold]
    #[inline(never)]
    fn alloc_uninit_slow(&mut self, size: usize, mut cid: usize) -> u32 {
        // SAFETY: cid is active_chunk (valid) or a searched valid chunk index
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        if chunk.free_list != NULL_PTR {
            let off = chunk.free_list as usize;
            // SAFETY: off comes from free_list, which only contains valid slot offsets
            let next = unsafe { *chunk.next_frees.get_unchecked(off) };
            chunk.free_list = next;
            chunk.free_count = chunk.free_count.saturating_sub(1);
            self.free_count = self.free_count.saturating_sub(1);
            chunk.alive_count += 1;
            // SAFETY: off < GC_CHUNK_SIZE (free_list slots are always valid offsets)
            unsafe {
                let global_idx = ((cid as u32) << crate::gc::heap::CHUNK_SHIFT) | (off as u32);
                chunk.set_uninit(off);
                *chunk.sizes.get_unchecked_mut(off) = size as u32;
                let data_slice = &mut *chunk.data.get();
                *data_slice.get_unchecked_mut(off) = None;
                *chunk.next_frees.get_unchecked_mut(off) = NULL_PTR;
                self.allocated_bytes += size;
                if chunk.generation == 0 {
                    self.nursery_bytes += size;
                } else {
                    self.tenured_bytes += size;
                }
                return global_idx;
            }
        }
        let len = self.chunks.len();
        let mut found = false;
        for i in 1..=len {
            let check_id = (cid + i) % len;
            if check_id == 0 {
                continue;
            }
            let c = &self.chunks[check_id];
            if c.generation == 0 && (c.free_list != NULL_PTR || c.top < GC_CHUNK_SIZE as u32) {
                cid = check_id;
                found = true;
                break;
            }
        }
        if !found {
            self.chunks.push(Chunk::new_nursery());
            cid = self.chunks.len() - 1;
            self.refresh_chunk_pointers();
        }
        self.active_chunk = cid;
        self.alloc_uninit_bump(size)
    }

    // ERSA Scratch Arena
    #[inline(always)]
    pub fn alloc_scratch(&mut self, obj: HeapObject) -> u32 {
        if unlikely(self.scratch_top >= SCRATCH_CAP as u32) {
            return self.alloc(obj);
        }
        let idx = self.scratch_top;
        self.scratch_top += 1;
        self.scratch_alloc_count += 1;
        use crate::gc::heap::SCRATCH_BASE;
        // SAFETY: idx < SCRATCH_CAP (checked above); scratch_data has SCRATCH_CAP elements
        unsafe {
            *self.scratch_data.get_unchecked_mut(idx as usize) =
                Some(std::cell::UnsafeCell::new(obj));
        }
        SCRATCH_BASE + idx
    }

    #[inline(always)]
    pub fn alloc_scratch_with_size(&mut self, obj: HeapObject, _size: usize) -> u32 {
        self.alloc_scratch(obj)
    }

    // Arena escape promotion
    #[inline(always)]
    pub fn promote_from_region(&mut self, obj: HeapObject, size: usize) -> u32 {
        self.alloc_with_size(obj, size)
    }

    fn scratch_take(&mut self, idx: u32) -> Option<HeapObject> {
        let off = scratch_off(idx);
        if off >= self.scratch_top as usize {
            return None;
        }
        // SAFETY: off < scratch_top (checked above); scratch_data has SCRATCH_CAP elements
        unsafe { self.scratch_data.get_unchecked_mut(off).take().map(|cell| cell.into_inner()) }
    }

    pub fn safe_point<F>(&mut self, mut scan_roots: F) -> Vec<(u32, u32)>
    where
        F: FnMut() -> Vec<u32>,
    {
        if self.scratch_top == 0 {
            return Vec::new();
        }
        let mut live_set = std::collections::HashSet::new();
        for idx in scan_roots() {
            if is_scratch(idx) && scratch_off(idx) < self.scratch_top as usize {
                live_set.insert(idx);
            }
        }
        let mut remap: Vec<(u32, u32)> = Vec::with_capacity(live_set.len());
        for &old_idx in &live_set {
            if let Some(obj) = self.scratch_take(old_idx) {
                let size = obj.size_estimate();
                let new_idx = self.alloc_with_size(obj, size);
                remap.push((old_idx, new_idx));
                self.scratch_promote_count += 1;
            }
        }
        self.scratch_top = 0;
        self.scratch_reset_count += 1;
        remap.sort_by_key(|(o, _)| *o);
        // S2 修复：传递性重写——提升后的对象内部可能仍引用其他已提升的 scratch
        // 对象（如 scratch Array A 引用 scratch Array B，两者均被提升到堆上）。
        // 若不重写 A 内部对 B 的引用，A 会持有旧的 scratch 索引 → 悬垂/UAF
        // （scratch_top 已重置为 0，旧索引指向的 slot 可能被新分配覆盖）。
        //
        // 此循环对所有已提升对象调用 remap_scratch_indices，把内部对 scratch
        // 的引用替换为新的持久堆索引。remap 已按 old_idx 排序（binary_search 要求）。
        //
        // 注意：这只重写被提升对象内部的引用。调用方仍需重写根集合（寄存器、
        // 全局变量、frame_data 等）中的 scratch 索引——这些 Value 不在 GC 堆中，
        // 无法通过本循环覆盖。
        for &(_, new_idx) in &remap {
            if let Some(heap_obj) = self.get_mut_if_present(new_idx) {
                heap_obj.remap_scratch_indices(&remap);
            }
        }
        remap
    }

    #[inline(always)]
    pub fn scratch_data_ptr(&self) -> *const Option<std::cell::UnsafeCell<HeapObject>> {
        self.scratch_data.as_ptr()
    }
    pub fn scratch_stats(&self) -> (u64, u64, u64) {
        (self.scratch_alloc_count, self.scratch_promote_count, self.scratch_reset_count)
    }
}

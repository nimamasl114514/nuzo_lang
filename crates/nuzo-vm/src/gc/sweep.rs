//! # 清除阶段 (Sweep Phase)
//! 职责：清除死亡对象、维护 free list、位运算跳跃扫描、冷区跳过、GC 阈值自适应。

use crate::gc::heap::{
    BITMAP_WORDS, BITS_PER_BITMAP_WORD, GC_CHUNK_SIZE, GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY,
    NULL_PTR, chunk_id, offset,
};
use crate::gc::{Gc, GcPhase, GcType, Trace};
use nuzo_core::Value;
use nuzo_signal::{GcDidCollectInfo, GcWillCollectInfo};
use nuzo_values::HeapObject;
use std::ptr::NonNull;

/// Micro-scan target: (chunk_id, dirty bits snapshot, pointer to chunk data).
///
/// The third element is a `NonNull` pointing to a `Box<[Option<HeapObject>; GC_CHUNK_SIZE]>`
/// owned by `chunk.data` (an `UnsafeCell`). Using `NonNull` instead of `*mut` documents
/// the non-null invariant enforced by `UnsafeCell::get()` (which returns a pointer to
/// the cell's content — never null for a live chunk).
///
/// # Safety obligations for consumers
/// - The pointer is valid only while the owning `Gc` is alive and the chunk is not
///   reallocated (chunks vector does not grow during micro-scan).
/// - Aliasing: the pointer is created from `&mut self.chunks[cid]` and read back as
///   `&**data_ptr` (shared borrow). This is sound because the GC is single-threaded
///   and no other mutation of `chunk.data` occurs during micro-scan.
type MicroScanTarget =
    (usize, [u64; BITMAP_WORDS], NonNull<Box<[Option<HeapObject>; GC_CHUNK_SIZE]>>);

impl Gc {
    #[inline(always)]
    pub(crate) fn lazy_sweep_step(&mut self) {
        let cid = chunk_id(self.sweep_cursor);
        let off = offset(self.sweep_cursor);
        if cid >= self.chunks.len() {
            self.phase = GcPhase::Idle;
            return;
        }
        // SAFETY: cid < self.chunks.len() checked by the guard at L24
        let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
        let is_dead = !chunk.is_marked(off) && !chunk.is_uninit(off);
        if is_dead {
            // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
            let data_slice = unsafe { &mut *chunk.data.get() };
            // SAFETY: off < chunk.top (bitmap iteration only visits in-use slots)
            if let Some(obj) = unsafe { data_slice.get_unchecked_mut(off).take() } {
                // SAFETY: off < chunk.top (same invariant as above)
                let size = unsafe { *chunk.sizes.get_unchecked(off) } as usize;
                self.allocated_bytes = self.allocated_bytes.saturating_sub(size);
                if chunk.generation == 0 {
                    self.nursery_bytes = self.nursery_bytes.saturating_sub(size);
                } else {
                    self.tenured_bytes = self.tenured_bytes.saturating_sub(size);
                }
                chunk.alive_count = chunk.alive_count.saturating_sub(1);
                drop(obj);
                // SAFETY: off < chunk.top (bitmap iteration guarantee)
                unsafe {
                    *chunk.next_frees.get_unchecked_mut(off) = chunk.free_list;
                }
                chunk.free_list = off as u32;
                chunk.free_count += 1;
                self.free_count += 1;
            }
        } else {
            // SAFETY: off < chunk.top (slot is alive/marked, so offset is valid)
            unsafe {
                *chunk.next_frees.get_unchecked_mut(off) = NULL_PTR;
            }
        }
        self.sweep_cursor += 1;
        if off == GC_CHUNK_SIZE - 1 && chunk.alive_count == 0 && chunk.top == GC_CHUNK_SIZE as u32 {
            self.free_count = self.free_count.saturating_sub(chunk.free_count as usize);
            chunk.top = 0;
            chunk.free_list = NULL_PTR;
            chunk.free_count = 0;
        }
    }

    pub fn sweep(&mut self) {
        while !self.wave_front_is_empty() {
            self.process_wave_front_step();
        }
        self.free_count = 0;
        for cid in 0..self.chunks.len() {
            let chunk = &mut self.chunks[cid];
            if chunk.is_cold {
                chunk.is_dirty = false;
                continue;
            }
            chunk.free_list = NULL_PTR;
            chunk.free_count = 0;
            let mut last_free: u32 = NULL_PTR;
            let top = chunk.top as usize;
            // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
            let data_slice = unsafe { &mut *chunk.data.get() };
            let data_ptr = data_slice.as_mut_ptr();
            let size_ptr = chunk.sizes.as_ptr();
            let next_ptr = chunk.next_frees.as_mut_ptr();
            for w in 0..BITMAP_WORDS {
                let uninit_bits = chunk.uninit_bits[w];
                let mark_bits = chunk.mark_bits[w];
                let mut dead_bits = !(mark_bits | uninit_bits);
                let base_off = w * BITS_PER_BITMAP_WORD;
                if base_off >= top {
                    break;
                }
                let limit = if base_off + BITS_PER_BITMAP_WORD > top {
                    top - base_off
                } else {
                    BITS_PER_BITMAP_WORD
                };
                if limit < BITS_PER_BITMAP_WORD {
                    let mask = (1u64 << limit) - 1;
                    dead_bits &= mask;
                }
                while dead_bits != 0 {
                    let bit = dead_bits.trailing_zeros() as usize;
                    let off = base_off + bit;
                    // SAFETY: off < top (dead_bits only has bits set within [0, top))
                    if let Some(obj) = unsafe { (*data_ptr.add(off)).take() } {
                        // SAFETY: off < top (same as above)
                        let size = unsafe { *size_ptr.add(off) } as usize;
                        self.allocated_bytes = self.allocated_bytes.saturating_sub(size);
                        if chunk.generation == 0 {
                            self.nursery_bytes = self.nursery_bytes.saturating_sub(size);
                        } else {
                            self.tenured_bytes = self.tenured_bytes.saturating_sub(size);
                        }
                        drop(obj);
                    }
                    if last_free != NULL_PTR {
                        // SAFETY: last_free < top (it was a previously visited dead slot)
                        unsafe {
                            *next_ptr.add(last_free as usize) = off as u32;
                        }
                    } else {
                        chunk.free_list = off as u32;
                    }
                    last_free = off as u32;
                    chunk.free_count += 1;
                    self.free_count += 1;
                    chunk.alive_count = chunk.alive_count.saturating_sub(1);
                    dead_bits &= dead_bits - 1;
                }
            }
            chunk.mark_bits.fill(0);
            chunk.uninit_bits.fill(0);
            if chunk.alive_count == 0 && chunk.top == GC_CHUNK_SIZE as u32 {
                self.free_count = self.free_count.saturating_sub(chunk.free_count as usize);
                chunk.top = 0;
                chunk.free_list = NULL_PTR;
                chunk.free_count = 0;
            }
            chunk.is_dirty = false;
        }
        self.sweep_cursor = (self.chunks.len() as u32) << crate::gc::heap::CHUNK_SHIFT;
        self.phase = GcPhase::Idle;
    }

    fn emit_gc_will_collect_signal(&self) {
        if let Ok(sig) = self.bus.get(&GC_WILL_COLLECT_KEY)
            && !sig.is_empty()
        {
            let total_slots: usize = self.chunks.iter().map(|c| c.top as usize).sum();
            sig.emit(&GcWillCollectInfo {
                live_count: total_slots.saturating_sub(self.free_count),
                threshold: self.threshold(),
            });
        }
    }

    fn emit_gc_did_collect_signal(&self, freed: usize, start: web_time::Instant) {
        if let Ok(sig) = self.bus.get(&GC_DID_COLLECT_KEY)
            && !sig.is_empty()
        {
            sig.emit(&GcDidCollectInfo {
                freed_count: freed,
                elapsed: start.elapsed(),
                new_threshold: self.threshold(),
            });
        }
    }

    pub fn collect(&mut self) {
        self.emit_gc_will_collect_signal();
        let start = web_time::Instant::now();
        let pre_free = self.free_count;
        let gc_type = if self.tenured_bytes > self.tenured_threshold {
            if self.major_gc_count.is_multiple_of(self.config.deep_gc_interval as u64) {
                GcType::Deep
            } else {
                GcType::Major
            }
        } else {
            GcType::Minor
        };
        self.mark_epoch = self.mark_epoch.wrapping_add(1);
        self.hot_stack.clear();
        self.cold_stack.clear();
        self.phase = GcPhase::Marking;
        self.sweep_cursor = GC_CHUNK_SIZE as u32;
        for chunk in &mut self.chunks {
            chunk.mark_bits.fill(0);
        }
        if let Some(f) = self.roots_fn {
            f(self, self.roots_userdata);
        }
        let mut micro_scan_targets: Vec<MicroScanTarget> = Vec::new();
        for cid in 0..self.chunks.len() {
            let chunk = &mut self.chunks[cid];
            if chunk.generation >= 1 && chunk.is_dirty {
                // SAFETY: chunk.data is an UnsafeCell containing a Box<[...]>; the
                // UnsafeCell::get() pointer is never null for a live chunk. We wrap
                // it in NonNull to enforce the invariant. The pointer is used below
                // within the same collect() call while self.chunks is not reallocated.
                let data_ptr = unsafe { NonNull::new_unchecked(chunk.data.get()) };
                micro_scan_targets.push((cid, *chunk.micro_dirty_bits, data_ptr));
            }
        }
        for (_, dirty_snapshot, data_ptr) in &micro_scan_targets {
            for (w, &snapshot_word) in dirty_snapshot.iter().enumerate() {
                let mut dirty_word = snapshot_word;
                while dirty_word != 0 {
                    let bit = dirty_word.trailing_zeros() as usize;
                    let off = w * BITS_PER_BITMAP_WORD + bit;
                    // SAFETY: data_ptr is a valid NonNull pointing to a Box<[...]> owned
                    // by chunk.data. The Box's dereferenced target is a valid array.
                    // off < chunk.top because micro_dirty_bits only tracks written slots,
                    // and slots are only written at indices < chunk.top.
                    // No concurrent mutation occurs: GC is single-threaded and chunks
                    // vector does not grow during this loop.
                    unsafe {
                        let slice: &[Option<HeapObject>; GC_CHUNK_SIZE] = &*data_ptr.as_ptr();
                        if let Some(obj) = slice[off].as_ref() {
                            obj.trace(self);
                        }
                    }
                    dirty_word &= dirty_word - 1;
                }
            }
        }
        for (cid, _, _) in micro_scan_targets {
            self.chunks[cid].clear_micro_dirty();
        }
        if gc_type == GcType::Major || gc_type == GcType::Deep {
            for cid in 0..self.chunks.len() {
                let chunk = &mut self.chunks[cid];
                if chunk.generation >= 1 && (!chunk.is_cold || gc_type == GcType::Deep) {
                    // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
                    let data_slice = unsafe { &*chunk.data.get() };
                    for off in 0..chunk.top as usize {
                        // SAFETY: off < chunk.top (loop bound)
                        if let Some(obj) = unsafe { data_slice.get_unchecked(off).as_ref() } {
                            obj.trace(self);
                        }
                    }
                } else if chunk.is_cold && gc_type != GcType::Deep {
                    chunk.mark_bits.fill(u64::MAX);
                }
            }
        }
        if gc_type == GcType::Deep {
            let mut cold_micro_targets: Vec<MicroScanTarget> = Vec::new();
            for cid in 0..self.chunks.len() {
                let chunk = &mut self.chunks[cid];
                if chunk.is_cold && chunk.is_dirty {
                    // SAFETY: Same as micro_scan_targets above — chunk.data is a live
                    // UnsafeCell and the pointer is non-null.
                    let data_ptr = unsafe { NonNull::new_unchecked(chunk.data.get()) };
                    cold_micro_targets.push((cid, *chunk.micro_dirty_bits, data_ptr));
                }
            }
            for (_, dirty_snapshot, data_ptr) in &cold_micro_targets {
                for (w, &snapshot_word) in dirty_snapshot.iter().enumerate() {
                    let mut dirty_word = snapshot_word;
                    while dirty_word != 0 {
                        let bit = dirty_word.trailing_zeros() as usize;
                        let off = w * BITS_PER_BITMAP_WORD + bit;
                        // SAFETY: Same as micro_scan_targets loop above — NonNull from
                        // chunk.data, off < chunk.top (micro_dirty_bits invariant).
                        unsafe {
                            let slice: &[Option<HeapObject>; GC_CHUNK_SIZE] = &*data_ptr.as_ptr();
                            if let Some(obj) = slice[off].as_ref() {
                                obj.trace(self);
                            }
                        }
                        dirty_word &= dirty_word - 1;
                    }
                }
            }
            for (cid, dirty_snapshot, _) in cold_micro_targets {
                let chunk = &mut self.chunks[cid];
                chunk.clear_micro_dirty();
                for (w, &snapshot_word) in dirty_snapshot.iter().enumerate() {
                    chunk.mark_bits[w] |= !snapshot_word;
                }
            }
        }
        self.sweep();
        if gc_type == GcType::Minor {
            for cid in 0..self.chunks.len() {
                let chunk = &mut self.chunks[cid];
                if chunk.generation == 0 && chunk.top > 0 {
                    let survival_ratio = chunk.alive_count as f64 / chunk.top as f64;
                    if survival_ratio > self.config.promote_survival_ratio {
                        chunk.generation = 1;
                        self.tenured_bytes += self.nursery_bytes;
                        self.nursery_bytes = 0;
                    }
                }
            }
            self.minor_gc_count += 1;
        } else {
            for cid in 0..self.chunks.len() {
                let chunk = &mut self.chunks[cid];
                if chunk.generation == 1 {
                    chunk.age = chunk.age.saturating_add(1);
                    if chunk.age >= self.config.cold_age_threshold {
                        chunk.is_cold = true;
                    }
                }
            }
            if gc_type == GcType::Deep {
                for cid in 0..self.chunks.len() {
                    self.chunks[cid].is_cold = false;
                    self.chunks[cid].age = 0;
                }
                self.deep_gc_count += 1;
            } else {
                self.major_gc_count += 1;
            }
        }
        self.adapt_threshold(gc_type);
        self.emit_gc_did_collect_signal(self.free_count.saturating_sub(pre_free), start);
    }

    pub fn collect_with_roots(&mut self, roots: impl Iterator<Item = Value>) {
        self.emit_gc_will_collect_signal();
        let start = web_time::Instant::now();
        let pre = self.free_count;
        self.mark_roots(roots);
        self.sweep();
        self.adapt_threshold(GcType::Major);
        self.emit_gc_did_collect_signal(self.free_count.saturating_sub(pre), start);
    }

    pub(crate) fn adapt_threshold(&mut self, gc_type: GcType) {
        if gc_type == GcType::Major || gc_type == GcType::Deep {
            let total: usize = self.chunks.iter().map(|c| c.top as usize).sum();
            if total == 0 {
                return;
            }
            let ratio = total.saturating_sub(self.free_count) as f64 / total as f64;
            if ratio < self.config.survival_ratio_threshold {
                self.tenured_threshold = (self.tenured_threshold * self.config.growth_factor)
                    .max(self.nursery_threshold * 2);
            }
        }
        self.bytes_until_gc = self.nursery_threshold as isize;
    }
}

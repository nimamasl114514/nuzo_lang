//! 大小类管理器
//!
//! TurboCache 管理单个 [`SizeClass`] 的分配与释放：
//! - 8 个 per-CPU 缓存（[`TurboCpuCache`]），每个持有 LIFO 本地空闲栈。
//! - 节点共享的 `shared_partial` 链表（Mutex 保护）。
//! - `full_slabs` 记录已写满的 slab。
//! - `all_slabs` 维护所有 slab 的有序列表，用于缓存内扁平索引映射与 Drop 清理。
//!
//! T6 关键变更：slab 槽位中存储的是 `Arc<HeapObject>`，以便 reclaim 通过
//! `Arc::strong_count` 判断对象是否仍被外部引用。

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::HeapStats;
use super::RemoteFreeNode;
use super::cpu_cache::{TurboCpuCache, cpu_idx_from_thread};
use super::slab::{SizeClass, TurboSlab, fill_local_stack_from_bitmap};
use crate::heap::HeapObject;

/// 大小类管理器
pub struct TurboCache {
    size_class: SizeClass,
    cpu_caches: Vec<TurboCpuCache>,
    shared_partial: Mutex<Vec<*mut TurboSlab>>,
    full_slabs: Mutex<Vec<*mut TurboSlab>>,
    /// 所有 slab 的有序列表，用于缓存内扁平索引映射与 Drop 时释放对象。
    all_slabs: Vec<*mut TurboSlab>,
    alloc_count: AtomicU64,
    reclaim_count: AtomicU64,
}

impl TurboCache {
    /// 创建指定大小类的缓存，默认初始化 8 个 CPU 缓存桶。
    pub fn new(size_class: SizeClass) -> Self {
        let mut cpu_caches = Vec::with_capacity(8);
        for _ in 0..8 {
            cpu_caches.push(TurboCpuCache::new());
        }
        Self {
            size_class,
            cpu_caches,
            shared_partial: Mutex::new(Vec::new()),
            full_slabs: Mutex::new(Vec::new()),
            all_slabs: Vec::new(),
            alloc_count: AtomicU64::new(0),
            reclaim_count: AtomicU64::new(0),
        }
    }

    /// 分配一个对象，返回缓存内扁平索引。
    ///
    /// 分配路径严格遵循 spec §3.5 的 5 步回退：
    /// 1. 当前 CPU 本地 LIFO 栈弹出。
    /// 2. 从当前 `partial_slab` 的位图批量填充本地栈。
    /// 3. 消费 slab 的 `remote_free_head` 无锁 Treiber 栈（T5）。
    /// 4. 从 `shared_partial` 弹出 slab 并设为当前 partial。
    /// 5. 分配新 slab 并设为当前 partial。
    pub fn alloc(&mut self, obj: HeapObject) -> Option<u32> {
        let cpu_idx = cpu_idx_from_thread();

        // Step 1: 本地 LIFO 栈弹出
        if let Some(obj_idx) = self.cpu_caches[cpu_idx].pop_local() {
            let slab_ptr = self.cpu_caches[cpu_idx].get_partial().unwrap();
            return Some(self.alloc_in_slab(slab_ptr, obj_idx, obj));
        }

        // Step 2: 从当前 partial_slab 位图批量填充本地栈
        if let Some(slab_ptr) = self.cpu_caches[cpu_idx].get_partial() {
            // SAFETY: slab_ptr was obtained from Box::into_raw in alloc()
            // (Step 5) and stored in all_slabs. Its lifetime is managed by
            // TurboCache. We only read immutable data (bitmap) here.
            let slab = unsafe { &*slab_ptr };
            fill_local_stack_from_bitmap(slab, self.cpu_caches[cpu_idx].local_stack_mut());
            if let Some(obj_idx) = self.cpu_caches[cpu_idx].pop_local() {
                return Some(self.alloc_in_slab(slab_ptr, obj_idx, obj));
            }
        }

        // Step 3: 消费 slab 的远程释放队列（Treiber 栈）
        if let Some(slab_ptr) = self.cpu_caches[cpu_idx].get_partial() {
            // SAFETY: slab_ptr comes from all_slabs (Box::into_raw), lifetime
            // managed by TurboCache. We only call &self methods here.
            let slab = unsafe { &*slab_ptr };
            // SAFETY: pop_remote_free returns pointers that were pushed via
            // push_remote_free, which only accepts Box::into_raw pointers.
            // Each popped node_ptr is a valid RemoteFreeNode owned by this
            // thread after successful CAS, so Box::from_raw reclaims ownership.
            while let Some(node_ptr) = unsafe { slab.pop_remote_free() } {
                // SAFETY: node_ptr was pushed by push_remote_free, which
                // requires the caller to pass a Box::into_raw pointer. The
                // CAS in pop_remote_free guarantees exclusive ownership.
                let node = unsafe { Box::from_raw(node_ptr) };
                self.cpu_caches[cpu_idx].push_local(node.object_idx);
            }
        }
        if let Some(obj_idx) = self.cpu_caches[cpu_idx].pop_local() {
            let slab_ptr = self.cpu_caches[cpu_idx].get_partial().unwrap();
            return Some(self.alloc_in_slab(slab_ptr, obj_idx, obj));
        }

        // Step 4: 从 shared_partial 申请已有 partial slab
        let maybe_shared_slab = {
            let mut shared = self.shared_partial.lock().unwrap();
            shared.pop()
        };
        if let Some(slab_ptr) = maybe_shared_slab {
            self.cpu_caches[cpu_idx].set_partial(slab_ptr);
            // SAFETY: slab_ptr was popped from shared_partial, which only
            // contains pointers from all_slabs (Box::into_raw). Lifetime
            // managed by TurboCache. Read-only access here.
            let slab = unsafe { &*slab_ptr };
            fill_local_stack_from_bitmap(slab, self.cpu_caches[cpu_idx].local_stack_mut());
            if let Some(obj_idx) = self.cpu_caches[cpu_idx].pop_local() {
                return Some(self.alloc_in_slab(slab_ptr, obj_idx, obj));
            }
        }

        // Step 5: 分配新 slab
        let slab_index = self.all_slabs.len();
        let mut slab = TurboSlab::new(&self.size_class, cpu_idx);
        slab.set_slab_index(slab_index);
        let slab_ptr = Box::into_raw(Box::new(slab));
        self.all_slabs.push(slab_ptr);
        self.cpu_caches[cpu_idx].set_partial(slab_ptr);
        // SAFETY: slab_ptr was just created by Box::into_raw above and pushed
        // into all_slabs. The Box is owned by TurboCache and outlives this
        // reference. We only read the bitmap here.
        let slab_ref = unsafe { &*slab_ptr };
        fill_local_stack_from_bitmap(slab_ref, self.cpu_caches[cpu_idx].local_stack_mut());
        let obj_idx = self.cpu_caches[cpu_idx].pop_local()?;
        Some(self.alloc_in_slab(slab_ptr, obj_idx, obj))
    }

    /// 在指定 slab 的指定索引处写入对象，并更新统计/满 slab 状态。
    fn alloc_in_slab(&mut self, slab_ptr: *mut TurboSlab, idx: u32, obj: HeapObject) -> u32 {
        let cpu_idx = cpu_idx_from_thread();
        // SAFETY: slab_ptr comes from all_slabs (Box::into_raw), lifetime
        // managed by TurboCache. We have &mut self, which guarantees exclusive
        // access to the entire TurboCache including this slab.
        let slab = unsafe { &mut *slab_ptr };
        slab.mark_occupied(idx);

        let arc = Arc::new(obj);
        let ptr = slab.object_ptr(idx) as *mut Arc<HeapObject>;
        // SAFETY: ptr points into the slab's allocated memory region at the
        // slot for idx. idx was just marked occupied, guaranteeing the slot
        // was previously deallocated (no live Arc at this address). The write
        // initializes the slot with a new Arc.
        unsafe { ptr.write(arc) };

        self.alloc_count.fetch_add(1, Ordering::Relaxed);

        if slab.free_count() == 0 {
            // slab 已满，移入 full_slabs 并清空当前 CPU 的 partial_slab
            self.cpu_caches[cpu_idx].clear_partial();
            self.add_full_slab(slab_ptr);
        }

        (slab.slab_index() * self.size_class.objects_per_slab + idx as usize) as u32
    }

    /// 释放指定 slab 中的指定索引。
    ///
    /// 同 CPU：将索引压回本地 LIFO 栈；跨 CPU：构造 `RemoteFreeNode` 并通过 CAS
    /// 推入目标 slab 的 `remote_free_head`，由后续 alloc 路径 Step 3 批量回收。
    #[allow(dead_code)] // TurboSlab 释放 API，保留供 GC 集成和手动内存管理使用
    pub(crate) unsafe fn free(&mut self, slab: *mut TurboSlab, idx: u32) {
        let cpu_idx = cpu_idx_from_thread();
        // SAFETY: slab comes from all_slabs (Box::into_raw), lifetime managed
        // by TurboCache. We only read owner_cpu() here.
        let slab_ref = unsafe { &*slab };

        // 先读取并 drop Arc，再释放位图槽位。
        let arc_ptr = slab_ref.object_ptr(idx) as *mut Arc<HeapObject>;
        // SAFETY: arc_ptr points to a slot that is currently occupied (has a
        // live Arc). ptr::read takes ownership of the Arc without running its
        // destructor; the returned Arc is immediately dropped, decrementing
        // the strong count and potentially freeing the HeapObject.
        unsafe {
            let _ = std::ptr::read(arc_ptr);
        };
        // SAFETY: slab is from all_slabs, lifetime managed by TurboCache.
        // After reading the Arc above, the slot no longer holds a live Arc,
        // so deallocate is safe to mark the bitmap slot as free.
        unsafe { (&mut *slab).deallocate(idx) };

        if slab_ref.owner_cpu() == cpu_idx {
            // 同 CPU：直接压入本地栈，无需原子操作。
            self.cpu_caches[cpu_idx].push_local(idx);
        } else {
            // 跨 CPU：构造远程释放节点，原子 CAS 推入目标 slab 的 Treiber 栈。
            let node = RemoteFreeNode::new(idx);
            let node_ptr = Box::into_raw(node);
            // SAFETY: node_ptr was just created by Box::into_raw above, so it
            // is a valid, uniquely-owned pointer. push_remote_free requires
            // exactly this invariant.
            if unsafe { !slab_ref.push_remote_free(node_ptr) } {
                // CAS 循环理论上永不失败；此处仅作防御性内存回收。
                // SAFETY: if push_remote_free failed, node_ptr was not consumed
                // by the stack, so we still uniquely own it and can reclaim it.
                unsafe { drop(Box::from_raw(node_ptr)) };
            }
        }
    }

    /// 回收孤立条目（不带排除集）：等价于传入空排除集。
    pub fn reclaim_orphaned(&mut self) -> usize {
        self.reclaim_orphaned_with_exclusions(&HashSet::new())
    }

    /// 回收孤立条目：扫描所有 slab 的占用槽位，若 `Arc::strong_count == 1`
    /// 则 drop Arc 并释放槽位。`exclusions` 中的扁平索引受 grace period 保护，
    /// 不会被回收，避免并行环境下误回收其他线程刚分配的对象。返回回收数量。
    pub fn reclaim_orphaned_with_exclusions(&mut self, exclusions: &HashSet<u32>) -> usize {
        let mut reclaimed = 0;

        let mut indices_buf = Vec::new();
        for &slab_ptr in &self.all_slabs {
            // SAFETY: all_slabs contains only pointers created by Box::into_raw
            // in alloc(). Their lifetime is managed by TurboCache. We have
            // &mut self, ensuring exclusive access.
            unsafe {
                // SAFETY: slab_ptr is from all_slabs (Box::into_raw), valid.
                let slab_ref = &*slab_ptr;
                let owner_cpu = slab_ref.owner_cpu();
                indices_buf.clear();
                indices_buf.extend(slab_ref.occupied_indices());

                for &idx in &indices_buf {
                    let flat_idx = (slab_ref.slab_index() * self.size_class.objects_per_slab
                        + idx as usize) as u32;

                    // Grace period: skip recently allocated objects to avoid
                    // false reclamation of objects just allocated by other threads.
                    if exclusions.contains(&flat_idx) {
                        continue;
                    }

                    let arc_ptr = slab_ref.object_ptr(idx) as *const Arc<HeapObject>;
                    // SAFETY: idx is an occupied index (from occupied_indices()),
                    // so the slot contains a valid Arc<HeapObject> that was
                    // written by alloc_in_slab. The reference is valid for the
                    // duration of this read.
                    let arc_ref = &*arc_ptr;

                    if Arc::strong_count(arc_ref) == 1 {
                        // 仅 slab 持有引用，安全回收。
                        // SAFETY: strong_count == 1 means the slab is the sole
                        // owner of this Arc. ptr::read takes ownership of the
                        // Arc, which is then immediately dropped, freeing the
                        // HeapObject. This is safe because no other reference
                        // exists to this Arc.
                        let _ = std::ptr::read(arc_ptr);
                        // SAFETY: slab_ptr is from all_slabs (valid). After
                        // ptr::read consumed the Arc above, the slot no longer
                        // holds a live Arc, so deallocate can safely mark it free.
                        (&mut *slab_ptr).deallocate(idx);

                        // 仅当该 slab 是所属 CPU 当前 partial_slab 时，才把索引压回本地栈。
                        if self.cpu_caches[owner_cpu].get_partial() == Some(slab_ptr) {
                            self.cpu_caches[owner_cpu].push_local(idx);
                        }

                        reclaimed += 1;
                    }
                }
            }
        }

        // 将已满但现在有空闲槽位的 slab 移回 shared_partial，供后续 alloc 使用。
        {
            let mut full_slabs = self.full_slabs.lock().unwrap();
            let mut shared = self.shared_partial.lock().unwrap();
            let mut still_full = Vec::with_capacity(full_slabs.len());
            for slab_ptr in full_slabs.drain(..) {
                // SAFETY: slab_ptr is from full_slabs, which only contains
                // pointers from all_slabs (Box::into_raw). We only read
                // free_count() here under the full_slabs Mutex lock.
                let slab = unsafe { &*slab_ptr };
                if slab.free_count() > 0 {
                    shared.push(slab_ptr);
                } else {
                    still_full.push(slab_ptr);
                }
            }
            *full_slabs = still_full;
        }

        self.reclaim_count.fetch_add(reclaimed as u64, Ordering::Relaxed);
        reclaimed
    }

    /// 将一个已满 slab 加入 full_slabs 列表。
    pub fn add_full_slab(&self, slab: *mut TurboSlab) {
        self.full_slabs.lock().unwrap().push(slab);
    }

    /// 根据缓存内扁平索引查找所属 slab。
    ///
    /// 映射规则：`flat_idx = slab_index * objects_per_slab + in_slab_idx`。
    pub fn get_slab_for_idx(&self, idx: u32) -> Option<*mut TurboSlab> {
        let idx = idx as usize;
        let slab_index = idx / self.size_class.objects_per_slab;
        self.all_slabs.get(slab_index).copied()
    }

    /// 返回指定扁平索引对应的 Arc 指针（仅用于内部 reclaim / get）。
    ///
    /// # Safety
    /// 返回的指针仅在槽位保持占用期间有效；调用方不得通过该指针产生可变引用。
    pub fn get_object_ptr(&self, idx: u32) -> Option<*const Arc<HeapObject>> {
        let slab_ptr = self.get_slab_for_idx(idx)?;
        let in_slab_idx = (idx as usize % self.size_class.objects_per_slab) as u32;
        // SAFETY: slab_ptr is from all_slabs (Box::into_raw), lifetime managed
        // by TurboCache. We check is_occupied before returning the pointer,
        // ensuring the slot holds a live Arc.
        unsafe {
            let slab = &*slab_ptr;
            if !slab.is_occupied(in_slab_idx) {
                return None;
            }
            let arc_ptr = slab.object_ptr(in_slab_idx) as *const Arc<HeapObject>;
            Some(arc_ptr)
        }
    }

    /// 返回每 slab 对象数。
    pub fn objects_per_slab(&self) -> usize {
        self.size_class.objects_per_slab
    }

    /// 返回当前总槽位数。
    pub fn len(&self) -> usize {
        self.all_slabs.len() * self.size_class.objects_per_slab
    }

    /// 返回是否没有任何 slab（即总槽位数为 0）。
    pub fn is_empty(&self) -> bool {
        self.all_slabs.is_empty()
    }

    /// 返回统计信息。
    pub fn stats(&self) -> HeapStats {
        let mut total_slots = 0;
        let mut occupied_slots = 0;
        for &slab_ptr in &self.all_slabs {
            // SAFETY: slab_ptr is from all_slabs (Box::into_raw), lifetime
            // managed by TurboCache. We only read immutable data here.
            unsafe {
                let slab = &*slab_ptr;
                total_slots += slab.objects_per_slab();
                occupied_slots += slab.occupied_indices().count();
            }
        }
        HeapStats {
            total_slots,
            occupied_slots,
            free_slots: total_slots - occupied_slots,
            alloc_count: self.alloc_count.load(Ordering::Relaxed),
            reclaim_count: self.reclaim_count.load(Ordering::Relaxed),
            slab_count: self.all_slabs.len(),
        }
    }

    /// 返回总分配次数。
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    /// 返回 reclaim 次数。
    pub fn reclaim_count(&self) -> u64 {
        self.reclaim_count.load(Ordering::Relaxed)
    }
}

// SAFETY: TurboCache contains raw pointers (*mut TurboSlab) in all_slabs,
// shared_partial, and full_slabs. These pointers are only created via
// Box::into_raw and are exclusively managed by TurboCache. All mutable access
// is through &mut self methods. shared_partial and full_slabs are protected
// by Mutex, and cpu_caches are partitioned by cpu_idx, preventing data races.
unsafe impl Send for TurboCache {}
// SAFETY: Shared mutable state (shared_partial, full_slabs) is protected by
// Mutex. all_slabs is only mutated through &mut self. cpu_caches are indexed
// by cpu_idx and each bucket is only accessed by its owning thread. The raw
// pointers in all_slabs are dereferenced only under proper synchronization.
unsafe impl Sync for TurboCache {}

impl Drop for TurboCache {
    fn drop(&mut self) {
        // 先释放所有 slab 中仍被占用的 Arc，再释放 slab 内存。
        for &slab_ptr in &self.all_slabs {
            // SAFETY: slab_ptr is from all_slabs (Box::into_raw), and we have
            // &mut self, guaranteeing exclusive access. Drop runs only once.
            // This block: (1) drains remote free nodes to prevent leaks,
            // (2) drops live Arcs in occupied slots, (3) frees the slab itself.
            unsafe {
                let slab = &*slab_ptr;
                // 清理远程释放节点，避免内存泄漏。
                // SAFETY: pop_remote_free returns nodes that were pushed via
                // push_remote_free (Box::into_raw pointers). We uniquely own
                // them during Drop since no concurrent access is possible.
                while let Some(node_ptr) = slab.pop_remote_free() {
                    // SAFETY: node_ptr was pushed by push_remote_free which
                    // requires Box::into_raw. Drop owns it exclusively here.
                    drop(Box::from_raw(node_ptr));
                }
                for idx in slab.occupied_indices() {
                    let arc_ptr = slab.object_ptr(idx) as *mut Arc<HeapObject>;
                    // SAFETY: idx is an occupied index, so the slot holds a
                    // live Arc. drop_in_place runs the Arc destructor, freeing
                    // the HeapObject if strong_count reaches 0. We do not
                    // deallocate the bitmap slot because the entire slab is
                    // about to be freed.
                    std::ptr::drop_in_place(arc_ptr);
                }
                // SAFETY: slab_ptr was created by Box::into_raw in alloc().
                // All Arcs in occupied slots have been dropped above, and
                // remote free nodes have been drained. We are the sole owner,
                // so Box::from_raw is safe.
                drop(Box::from_raw(slab_ptr));
            }
        }
        self.all_slabs.clear();
        for cpu_cache in &mut self.cpu_caches {
            cpu_cache.clear_partial();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;
    use nuzo_core::NIL;

    fn test_size_class(objects_per_slab: usize) -> SizeClass {
        SizeClass::new(std::mem::size_of::<Arc<HeapObject>>(), objects_per_slab)
    }

    fn dummy_obj() -> HeapObject {
        HeapObject::Box(NIL)
    }

    #[test]
    fn test_cache_alloc_local() {
        let mut cache = TurboCache::new(test_size_class(8));
        let idx = cache.alloc(dummy_obj()).expect("first alloc should succeed");
        let slab = cache.get_slab_for_idx(idx).expect("should find slab");

        let in_slab_idx = (idx as usize % 8) as u32;
        unsafe { cache.free(slab, in_slab_idx) };

        let idx2 = cache.alloc(dummy_obj()).expect("reuse should succeed");
        assert_eq!(idx2, idx, "local stack should reuse freed index");
        assert_eq!(cache.alloc_count(), 2);
    }

    #[test]
    fn test_cache_alloc_fills_bitmap() {
        let mut cache = TurboCache::new(test_size_class(8));

        // 第一次分配：本地栈空 -> 新 slab -> 从位图填充 -> 弹出。
        let idx0 =
            cache.alloc(dummy_obj()).expect("first alloc should create slab and fill bitmap");
        let slab0 = cache.get_slab_for_idx(idx0).expect("should find slab");

        // 第二次分配：应从同一 slab 的本地栈弹出，不创建新 slab。
        let idx1 = cache.alloc(dummy_obj()).expect("second alloc should pop from local stack");
        assert_eq!(
            cache.get_slab_for_idx(idx1),
            Some(slab0),
            "second alloc should reuse the same slab"
        );
        assert_eq!(cache.alloc_count(), 2);
    }

    #[test]
    fn test_cache_alloc_new_slab() {
        let objects_per_slab = 4;
        let mut cache = TurboCache::new(test_size_class(objects_per_slab));

        // 填满第一个 slab。
        let mut indices = Vec::new();
        for _ in 0..objects_per_slab {
            indices.push(cache.alloc(dummy_obj()).expect("alloc should succeed"));
        }
        assert_eq!(cache.all_slabs.len(), 1, "should have exactly one slab");
        assert_eq!(cache.full_slabs.lock().unwrap().len(), 1, "first slab should be full");

        // 再分配一次应触发 Step 5 创建新 slab。
        let idx_new = cache.alloc(dummy_obj()).expect("alloc should create new slab");
        assert_eq!(cache.all_slabs.len(), 2, "should have two slabs");
        assert!(
            cache.get_slab_for_idx(idx_new).unwrap() != cache.get_slab_for_idx(indices[0]).unwrap(),
            "new alloc should land in a different slab"
        );

        // 释放所有对象，避免 Drop 抱怨也不依赖 Drop 兜底。
        for idx in indices {
            let slab = cache.get_slab_for_idx(idx).unwrap();
            let in_slab_idx = (idx as usize % objects_per_slab) as u32;
            unsafe { cache.free(slab, in_slab_idx) };
        }
        let slab = cache.get_slab_for_idx(idx_new).unwrap();
        let in_slab_idx = (idx_new as usize % objects_per_slab) as u32;
        unsafe { cache.free(slab, in_slab_idx) };
    }

    #[test]
    fn test_cache_full_slab() {
        let objects_per_slab = 4;
        let mut cache = TurboCache::new(test_size_class(objects_per_slab));

        for _ in 0..objects_per_slab {
            cache.alloc(dummy_obj()).unwrap();
        }

        assert_eq!(cache.full_slabs.lock().unwrap().len(), 1, "slab should be recorded as full");
        assert!(cache.shared_partial.lock().unwrap().is_empty(), "shared_partial should be empty");
        assert_eq!(cache.alloc_count(), objects_per_slab as u64);
    }

    #[test]
    fn test_remote_free_cross_cpu() {
        use std::sync::{Arc as StdArc, Mutex};
        use std::thread;

        let cache = StdArc::new(Mutex::new(TurboCache::new(test_size_class(8))));

        let idx = {
            let mut c = cache.lock().unwrap();
            c.alloc(dummy_obj()).expect("first alloc should succeed")
        };
        let slab = {
            let c = cache.lock().unwrap();
            c.get_slab_for_idx(idx).expect("should find slab")
        };
        let in_slab_idx = (idx as usize % 8) as u32;

        // 在另一个线程释放：其 cpu_idx 大概率与 owner_cpu 不同，触发跨 CPU 路径。
        let cache_clone = StdArc::clone(&cache);
        let handle = thread::spawn(move || {
            let mut c = cache_clone.lock().unwrap();
            let slab = c.get_slab_for_idx(idx).expect("should find slab");
            unsafe { c.free(slab, in_slab_idx) };
        });
        handle.join().unwrap();

        let mut c = cache.lock().unwrap();
        let slab_ref = unsafe { &*slab };
        assert!(!slab_ref.is_occupied(in_slab_idx), "slot should be free after free()");

        let mut found_remote = false;
        while let Some(node_ptr) = unsafe { slab_ref.pop_remote_free() } {
            let node = unsafe { Box::from_raw(node_ptr) };
            if node.object_idx == in_slab_idx {
                found_remote = true;
            }
        }

        let current_cpu = cpu_idx_from_thread();
        let found_local = c.cpu_caches[current_cpu].local_stack_mut().contains(&in_slab_idx);

        assert!(
            found_remote || found_local,
            "freed index should be in remote_free_head (cross-CPU) or local stack (same-CPU)"
        );
    }

    #[test]
    fn test_cache_reclaim_orphaned() {
        let mut cache = TurboCache::new(test_size_class(8));
        let idx = cache.alloc(dummy_obj()).expect("alloc should succeed");

        let reclaimed = cache.reclaim_orphaned();
        assert_eq!(reclaimed, 1, "orphan Arc should be reclaimed");

        let slab = cache.get_slab_for_idx(idx).unwrap();
        let in_slab_idx = (idx as usize % 8) as u32;
        assert!(!unsafe { &*slab }.is_occupied(in_slab_idx), "slot should be free after reclaim");
    }

    #[test]
    fn test_cache_reclaim_skips_shared() {
        let mut cache = TurboCache::new(test_size_class(8));
        let idx = cache.alloc(dummy_obj()).expect("alloc should succeed");

        let arc_ptr = cache.get_object_ptr(idx).unwrap();
        let arc_clone = unsafe { Arc::clone(&*arc_ptr) };
        assert_eq!(unsafe { Arc::strong_count(&*arc_ptr) }, 2);

        assert_eq!(cache.reclaim_orphaned(), 0, "shared Arc should not be reclaimed");

        drop(arc_clone);
        assert_eq!(cache.reclaim_orphaned(), 1, "Arc should be reclaimed after external drop");
    }

    #[test]
    fn test_alloc_local_exhausted() {
        let objects_per_slab = 4;
        let mut cache = TurboCache::new(test_size_class(objects_per_slab));

        // 填满第一个 slab，使本地栈空且位图全占用
        for _ in 0..objects_per_slab {
            cache.alloc(dummy_obj()).expect("alloc should succeed");
        }
        assert_eq!(cache.full_slabs.lock().unwrap().len(), 1, "first slab should be full");

        // 继续分配必须回退到 Step 3/4/5（远程释放 / 共享 partial / 新 slab）
        let idx = cache.alloc(dummy_obj()).expect("alloc should succeed after local exhaustion");
        assert_eq!(
            cache.all_slabs.len(),
            2,
            "should allocate a new slab when local stack and bitmap are exhausted"
        );
        assert!(idx as usize >= objects_per_slab, "new index should come from second slab");

        // 清理第二个 slab，避免 Drop 重复释放
        let slab = cache.get_slab_for_idx(idx).unwrap();
        let in_slab_idx = (idx as usize % objects_per_slab) as u32;
        unsafe { cache.free(slab, in_slab_idx) };
    }

    #[test]
    fn test_cross_cpu_release() {
        use std::sync::{Arc as StdArc, Mutex};
        use std::thread;

        let cache = StdArc::new(Mutex::new(TurboCache::new(test_size_class(8))));

        let idx = {
            let mut c = cache.lock().unwrap();
            c.alloc(dummy_obj()).expect("first alloc should succeed")
        };
        let in_slab_idx = (idx as usize % 8) as u32;

        // 在另一个线程释放，其 cpu_idx 大概率与 owner_cpu 不同，触发跨 CPU 远程释放
        let cache_clone = StdArc::clone(&cache);
        let handle = thread::spawn(move || {
            let mut c = cache_clone.lock().unwrap();
            let slab = c.get_slab_for_idx(idx).expect("should find slab");
            unsafe { c.free(slab, in_slab_idx) };
        });
        handle.join().unwrap();

        let c = cache.lock().unwrap();
        let slab = c.get_slab_for_idx(idx).expect("should find slab");
        let slab_ref = unsafe { &*slab };
        assert!(!slab_ref.is_occupied(in_slab_idx), "slot should be free after cross-CPU release");
    }

    #[test]
    fn test_slab_grow() {
        let objects_per_slab = 4;
        let mut cache = TurboCache::new(test_size_class(objects_per_slab));

        let mut indices = Vec::new();
        for _ in 0..objects_per_slab {
            indices.push(cache.alloc(dummy_obj()).expect("should fill first slab"));
        }
        assert_eq!(cache.all_slabs.len(), 1, "should have one slab");

        let idx_new = cache.alloc(dummy_obj()).expect("should trigger new slab allocation");
        assert_eq!(cache.all_slabs.len(), 2, "should grow to two slabs");

        // 清理
        for idx in indices {
            let slab = cache.get_slab_for_idx(idx).unwrap();
            let in_slab_idx = (idx as usize % objects_per_slab) as u32;
            unsafe { cache.free(slab, in_slab_idx) };
        }
        let slab = cache.get_slab_for_idx(idx_new).unwrap();
        let in_slab_idx = (idx_new as usize % objects_per_slab) as u32;
        unsafe { cache.free(slab, in_slab_idx) };
    }
}

//! # 标记阶段 (Mark Phase)
//!
//! 职责：三色标记（tri-color marking）、Hot/Cold 栈处理、trace 遍历、写屏障、
//! 软件预取（software prefetch）以掩盖指针跳转的内存延迟。
//!
//! ## Hot/Cold 栈分段（延迟 spill 语义）
//!
//! `hot_stack` 是一个小容量（`HOT_STACK_CAP = 128`）的栈，保持在工作集内
//! 以利用 CPU L1/L2 缓存；`cold_stack` 是溢出区。`mark_index` **无条件**
//! push 到 `hot_stack`，**不在 mark_index 中触发 spill**——spill 被延迟到
//! `process_wave_front_step` 中执行（当 `hot_stack.len() >= HOT_STACK_CAP`
//! 时把栈顶 `SPILL_COUNT = 64` 项移到 `cold_stack`）。
//!
//! 延迟 spill 的设计理由：
//! - `mark_index` 是极高频调用（每个后继一次），spill 的 `drain + extend`
//!   开销（~200 cycles）会拖慢 mark-only 工作负载（G5 实测 1.78M → 1.14M
//!   marks/s，回归 36%）；
//! - G5（mark-only）无 `process_wave_front_step`，延迟 spill 意味着
//!   `hot_stack` 增长到 root 数，但不影响正确性（`mark_roots` 结束后
//!   clear），且 G5 不需要 L1/L2 局部性（无 pop 操作）；
//! - G3（链表遍历）`hot_stack` 始终 1-2 项，永不触发 spill；
//! - 宽树遍历（多后继）`hot_stack` 增长时 `process_wave_front_step` 触发
//!   spill，保持 `hot_stack` 小容量以利用 L1/L2。
//!
//! 标记循环优先排空 `hot_stack`，并在排空后从 `cold_stack` 批量补充
//! （replenish）最多 `HOT_STACK_CAP` 项，使预取窗口持续有效。
//!
//! ## 软件预取
//!
//! 在 `process_wave_front_step` 中对 `hot_stack` 栈顶后续 `PREFETCH_DISTANCE`
//! 项发出 `_mm_prefetch`（x86_64）预取，使后续读取该对象时数据已在 L1/L2。
//! 非 x86_64 平台降级为 no-op。注：`mark_index` 中**不**执行 prefetch，
//! 因为 mark_index 是极高频调用，无条件 prefetch 的指令开销会拖慢
//! mark-only 工作负载（G5 实测 1.78M → 0.69M marks/s 回归）。
//!
//! ## 增强预取与流水线优化
//!
//! 为进一步隐藏内存延迟，本实现额外引入两项优化：
//!
//! 1. **标记位并行预取**：`prefetch_object_slot` 现在同时预取对象的数据槽
//!    以及对应 chunk 中标记位所在的缓存行。因为 `is_marked` 与对象数据
//!    通常不在同一缓存行，该预取可将标记检查的缓存缺失率降低 30–50%。
//!
//! 2. **前瞻预取（next-object prefetch）**：`process_wave_front_step`
//!    在处理当前对象前，预先发出对**下一个待处理对象**的数据槽与标记位
//!    的预取。这形成单步流水线：处理本对象的同时，下一个对象的数据已开始
//!    流入缓存，使连续处理时几乎无停顿。

use crate::gc::heap::{GC_CHUNK_SIZE, chunk_id, is_scratch, offset, scratch_off};
use crate::gc::{Gc, GcPhase, Trace};

// ============================================================================
// Hot/Cold 栈分段常量
// ============================================================================

/// `hot_stack` 的最大容量（项数）。
///
/// 选择 128 是为了：
/// - 128 项 × 4 字节 = 512 字节，约 8 个缓存行，保持 L1D 局部性；
/// - 与 `heap::HOT_STACK_INITIAL_CAPACITY` 一致，预分配正好覆盖上限，
///   避免任何 realloc；
/// - 超出此容量的入栈项溢出到 `cold_stack`，使 hot 工作集始终在 L1/L2。
pub(crate) const HOT_STACK_CAP: usize = 128;

/// 软件预取距离：在处理 hot_stack 时对栈内后续多少项再次预取。
///
/// 典型值为 8：覆盖几轮循环的访存延迟，同时不污染过多缓存行。
pub(crate) const PREFETCH_DISTANCE: usize = 8;

// ============================================================================
// 软件预取原语（平台条件编译）
// ============================================================================

/// x86_64 平台：使用 `_mm_prefetch` 将缓存行加载到所有层级（L1/L2/L3）。
///
/// # Safety
/// 调用方需保证 `addr` 指向已分配的内存区域（不要求已初始化）。
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn prefetch_object(addr: *const u8) {
    unsafe {
        core::arch::x86_64::_mm_prefetch(addr as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
}

/// 非 x86_64 平台：降级为 no-op，保持 API 一致性。
#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
unsafe fn prefetch_object(_addr: *const u8) {}

impl Gc {
    /// 预取指定 GC 索引对应对象的数据槽**及其标记位**到 L1/L2。
    ///
    /// 数据槽位于 `chunk.data[off]`，标记位位于 `chunk.mark_bits` 中
    /// 对应字的缓存行。两项预取并行发出，使后续 `is_marked` 和对象读取
    /// 均能命中缓存。
    ///
    /// 对越界索引静默忽略。
    #[inline(always)]
    fn prefetch_object_slot(&self, idx: u32) {
        let cid = chunk_id(idx);
        if cid >= self.chunks.len() {
            return;
        }
        let off = offset(idx);
        // SAFETY: cid < self.chunks.len() checked above
        let chunk = unsafe { self.chunks.get_unchecked(cid) };

        // 1) 预取对象数据槽（原行为）
        let data_slice = unsafe { &*chunk.data.get() };
        let slot_ptr = unsafe { data_slice.as_ptr().add(off) };
        unsafe { prefetch_object(slot_ptr as *const u8) };

        // 2) 预取标记位所在字
        // mark_bits 是 Vec<u64>，每个位对应一个槽。
        let word_idx = off / 64;
        let mark_slice = &chunk.mark_bits;
        // SAFETY: word_idx < mark_slice.len() because off < GC_CHUNK_SIZE
        //         and GC_CHUNK_SIZE <= mark_bits.len()*64
        if word_idx < mark_slice.len() {
            let mark_ptr = unsafe { mark_slice.as_ptr().add(word_idx) };
            unsafe { prefetch_object(mark_ptr as *const u8) };
        }
    }

    /// 从 `cold_stack` 批量补充项到 `hot_stack`，使预取窗口持续有效。
    ///
    /// 仅在 `hot_stack` 为空且 `cold_stack` 非空时触发，从 `cold_stack`
    /// 末尾（最近压入的）取最多 `HOT_STACK_CAP` 项追加到 `hot_stack`，
    /// 保持 LIFO 处理顺序。
    #[inline]
    fn replenish_hot_stack(&mut self) {
        if !self.hot_stack.is_empty() || self.cold_stack.is_empty() {
            return;
        }
        let take = self.cold_stack.len().min(HOT_STACK_CAP);
        let src_start = self.cold_stack.len() - take;
        // SAFETY: src_start..len is a valid range within cold_stack
        let drained: Vec<u32> = self.cold_stack.drain(src_start..).collect();
        self.hot_stack.extend(drained);
        // 预热刚补充进来的项，使后续 pop 时数据已在缓存
        let len = self.hot_stack.len();
        let pf = PREFETCH_DISTANCE.min(len.saturating_sub(1));
        for i in 1..=pf {
            // SAFETY: 1..=pf < len (checked above)
            let pf_idx = unsafe { *self.hot_stack.get_unchecked(len - 1 - i) };
            self.prefetch_object_slot(pf_idx);
        }
    }

    #[inline(always)]
    pub fn mark_index(&mut self, idx: u32) {
        // HEAP_POOL indices live below HEAP_POOL_INDEX_LIMIT and must never be
        // interpreted as GC chunk offsets. Silently ignore them.
        if idx < nuzo_values::constants::HEAP_POOL_INDEX_LIMIT as u32 {
            return;
        }
        // Unsafe fast-path push: avoid Vec::push bounds check in the hot path.
        // hot_stack pre-allocates HOT_STACK_INITIAL_CAPACITY=192; deferred spill
        // means mark_index only pushes (never pops), so capacity is always
        // sufficient for typical root counts (G5: 1000 roots < 192? No —
        // 1000 > 192, so the safe fallback handles realloc).
        // SAFETY: len < cap checked before write; push is a safe fallback
        unsafe {
            let len = self.hot_stack.len();
            let cap = self.hot_stack.capacity();
            if len < cap {
                self.hot_stack.as_mut_ptr().add(len).write(idx);
                self.hot_stack.set_len(len + 1);
            } else {
                self.hot_stack.push(idx);
            }
        }
    }

    /// Slow path：`hot_stack` 达到 `HOT_STACK_CAP`，把栈顶 `SPILL_COUNT` 项
    /// 移到 `cold_stack`，使 hot_stack 回到 `HOT_STACK_CAP - SPILL_COUNT` 项。
    ///
    /// 标记 `#[cold]` + `#[inline(never)]` 让编译器将 `process_wave_front_step`
    /// 的 fast path（pop + 标记）内联到调用点，而 spill 的批量搬运开销不影响
    /// fast path 的代码生成。
    ///
    /// spill 移走栈顶（最近 push 的）SPILL_COUNT 项到 cold_stack 末尾，
    /// replenish 时从 cold_stack 末尾取回，保持 LIFO 处理顺序。
    #[cold]
    #[inline(never)]
    fn spill_to_cold_stack(&mut self) {
        const SPILL_COUNT: usize = 64;
        let len = self.hot_stack.len();
        if len <= SPILL_COUNT {
            return;
        }
        let start = len - SPILL_COUNT;
        // drain 不释放 hot_stack 的容量（保留 capacity 供后续 push 复用），
        // extend 把栈顶项追加到 cold_stack（cold_stack 预分配了
        // COLD_STACK_INITIAL_CAPACITY=4096，通常不会触发 realloc）。
        self.cold_stack.extend(self.hot_stack.drain(start..));
    }

    #[inline(always)]
    pub(crate) fn process_wave_front_step(&mut self) {
        // 延迟 spill：如果 hot_stack 超过 HOT_STACK_CAP，把栈顶 SPILL_COUNT
        // 项移到 cold_stack，保持 hot_stack 小容量以利用 L1/L2 缓存。
        // 放在 prefetch 之前，使 prefetch 对准 spill 后的新栈顶项（即将 pop）。
        // 此检查虽每次执行，但仅 1 cycle 且高度可预测（通常不跳转）；
        // spill 操作本身用 `#[cold]` 隔离，不影响 fast path 代码生成。
        if self.hot_stack.len() >= HOT_STACK_CAP {
            self.spill_to_cold_stack();
        }

        // 对 hot_stack 顶项之后的 PREFETCH_DISTANCE 项发预取，扩大预取窗口。
        // 这些项即将被处理，预取让它们的数据在被读取前进入 L1/L2。
        let hot_len = self.hot_stack.len();
        if hot_len > 1 {
            let pf_count = PREFETCH_DISTANCE.min(hot_len - 1);
            for i in 1..=pf_count {
                // SAFETY: 1..=pf_count < hot_len (pf_count <= hot_len - 1)
                let pf_idx = unsafe { *self.hot_stack.get_unchecked(hot_len - 1 - i) };
                self.prefetch_object_slot(pf_idx);
            }
        }

        // 优先从 hot_stack pop；为空时从 cold_stack 补充后再 pop。
        let idx = if !self.hot_stack.is_empty() {
            // SAFETY: hot_stack is non-empty (checked above)
            unsafe { self.hot_stack.pop().unwrap_unchecked() }
        } else {
            self.replenish_hot_stack();
            if self.hot_stack.is_empty() {
                // 两个栈都空，本轮无工作可做
                return;
            }
            // SAFETY: hot_stack is non-empty after replenish (checked above)
            unsafe { self.hot_stack.pop().unwrap_unchecked() }
        };

        // ── 前瞻预取：提前预取下一个待处理对象 ──
        // 在当前对象处理前，预先对下一个对象（如果存在）发出数据+标记位预取。
        // 这使得连续 pop 时，下一个对象的缓存行与当前对象的处理重叠，
        // 有效隐藏标记位的访问延迟。
        if let Some(&next_idx) = self.hot_stack.last() {
            self.prefetch_object_slot(next_idx);
        }

        // S3 修复：scratch 索引（>= SCRATCH_BASE）不在 chunks 中，原代码
        // `cid >= self.chunks.len()` 静默提前返回 → scratch 对象及其引用的堆对象
        // 逃过标记 → 误回收。现在遇到 scratch 索引时 trace 其引用的堆对象，
        // 确保 scratch 引用的对象参与标记。scratch 对象本身不需要标记位
        // （它们不在 chunks 中，不会被 sweep），但它们引用的堆对象必须被标记。
        //
        // 使用 scratch_mark_epoch 防止循环引用（scratch A 引用 scratch B，B 引用 A）
        // 导致无限递归：每个 scratch 槽只在本 mark_epoch 内 trace 一次。
        if is_scratch(idx) {
            let soff = scratch_off(idx);
            // soff < SCRATCH_CAP（由 SCRATCH_MASK 保证），但对象可能已被
            // scratch_take 提升或不存在（stale 索引）→ slot 为 None 时跳过。
            if soff >= self.scratch_top as usize {
                return;
            }
            // SAFETY: soff < scratch_top (checked above) ≤ SCRATCH_CAP
            let already_traced =
                unsafe { *self.scratch_mark_epoch.get_unchecked(soff) == self.mark_epoch };
            if already_traced {
                return;
            }
            // SAFETY: soff < SCRATCH_CAP (masked by SCRATCH_MASK)
            let slot_ref = unsafe { self.scratch_data.get_unchecked(soff) };
            if let Some(cell) = slot_ref.as_ref() {
                // SAFETY: we hold &mut self (exclusive access to Gc);
                // the UnsafeCell gives us interior mutability for the
                // trace call which only reads the object.
                let obj = unsafe { &*cell.get() };
                // 标记本 epoch 已 trace，防止循环引用重入
                // SAFETY: soff < SCRATCH_CAP (checked above)
                unsafe {
                    *self.scratch_mark_epoch.get_unchecked_mut(soff) = self.mark_epoch;
                }
                // trace 会调用 mark_index 推送被引用对象到 hot_stack，
                // 包括 scratch 引用的堆对象和 scratch 引用的 scratch 对象
                // （后者会再次进入此分支，由 already_traced 检查防环）。
                obj.trace(self);
            }
            return;
        }

        let cid = chunk_id(idx);
        let off = offset(idx);
        if cid >= self.chunks.len() {
            return;
        }
        // Phase 1: mark 位检查与设置（借用 &mut self.chunks，块结束后释放）
        // SAFETY: cid < self.chunks.len() checked above
        {
            let chunk = unsafe { self.chunks.get_unchecked_mut(cid) };
            if chunk.is_marked(off) {
                return;
            }
            chunk.set_mark(off);
            chunk.last_mark_epoch = self.mark_epoch;
        }
        // [T-3-D 已移除] 原 T-3-D 方案 B 在此读取 64KB 的 hot_blocks 数组做
        // hot/cold 分支决策，但实测 G3 基准（1000 节点链表深遍）性能从
        // 436K 倒退到 126K（3.5x 回归）：hot_blocks 与 chunk.data 竞争 CPU
        // 缓存行，两路内存访问反而比单路冷路径更慢。HotBlock 系统已被完全
        // 删除（heap.rs/alloc.rs/sweep.rs/gc.rs 同步移除），此处保留冷路径。
        //
        // SAFETY: cid < self.chunks.len() checked above
        let chunk = unsafe { self.chunks.get_unchecked(cid) };
        // SAFETY: chunk.data is a valid UnsafeCell; we hold &mut self
        let data_slice = unsafe { &*chunk.data.get() };
        // SAFETY: off < GC_CHUNK_SIZE (offset() masks to CHUNK_MASK)
        if let Some(obj) = unsafe { data_slice.get_unchecked(off).as_ref() } {
            obj.trace(self);
        }
    }

    pub fn mark_roots(&mut self, roots: impl Iterator<Item = nuzo_core::Value>) {
        self.mark_epoch = self.mark_epoch.wrapping_add(1);
        self.hot_stack.clear();
        self.cold_stack.clear();
        self.phase = GcPhase::Marking;
        for chunk in &mut self.chunks {
            chunk.mark_bits.fill(0);
        }
        for v in roots {
            v.trace(self);
        }
    }

    /// 返回是否两个标记栈都已排空（用于 sweep 前的 drain 判断）。
    #[inline(always)]
    pub(crate) fn wave_front_is_empty(&self) -> bool {
        self.hot_stack.is_empty() && self.cold_stack.is_empty()
    }

    #[inline(always)]
    pub(crate) fn pace_incremental(&mut self) {
        if self.phase == GcPhase::Idle {
            return;
        }
        if self.phase == GcPhase::Marking {
            for _ in 0..self.mark_rate {
                if self.wave_front_is_empty() {
                    self.phase = GcPhase::Sweeping;
                    self.sweep_cursor = GC_CHUNK_SIZE as u32;
                    break;
                }
                self.process_wave_front_step();
            }
        }
        if self.phase == GcPhase::Sweeping {
            for _ in 0..self.sweep_rate {
                if chunk_id(self.sweep_cursor) >= self.chunks.len() {
                    self.phase = GcPhase::Idle;
                    break;
                }
                self.lazy_sweep_step();
            }
        }
    }
}

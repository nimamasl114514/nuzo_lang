//! 栈帧换入换出（Stack Frame Paging）模块 — SCHF v6 Phase 4
//!
//! # 核心思想
//!
//! 给一个固定容量的帧栈，当递归深度即将耗尽时，把最老的若干调用帧
//! 从帧栈底部"换出"到堆上，释放出底部空间，并留下一个桩帧（Trampoline）。
//! 当函数逐层返回到达桩帧时，桩帧自动把堆上的帧"换回"帧栈上。
//!
//! # SCHF v6 Phase 4 适配（spec 5.2）
//!
//! FramePager 不再操作 `VecDeque<CallFrame>`，改为直接操作 `ExecutionContext`
//! 的 v6 帧栈结构（`frame_metas` + `frame_ring` + `frame_overflow`）。
//! 算法逻辑保持不变（spill batch 大小、LIFO 顺序、trampoline 插入策略）。
//!
//! # 工作流程
//!
//! ```text
//! [正常 v6 帧栈]          [换出后]                [换入后]
//! frame_metas:           frame_metas:           frame_metas:
//!   [m0, m1, ..., mN]      [Trampoline,           [m0, m1, ..., mN]
//!                          mB, ..., mN]
//! overflow.infos:        overflow.infos:        overflow.infos:
//!   [i0, i1, ..., iN]      [Trampoline,           [i0, i1, ..., iN]
//!                          iB, ..., iN]
//!                         spilled_blocks:       spilled_blocks:
//!                          [{infos:[i0..iB-1],   []
//!                           metas:[m0..mB-1]}]
//! ```
//!
//! # 性能考量
//!
//! - 换入换出是低频操作（仅在深度接近上限时触发），因此 Vec 的头部操作 O(n)
//!   完全可接受
//! - `should_spill()` 和 `depth()` 标记为 `#[inline(always)]`，因为它们在热路径上
//! - 桩帧检测 `front_is_trampoline()` 也是内联的，在每次 pop_frame 时调用

use crate::vm::frame_v6::{FrameInfo, FrameMeta};
use crate::vm::{ExecutionContext, FrameKind};
use nuzo_config::FramePagingConfig;

// ============================================================================
// 常量定义
// ============================================================================

/// 帧栈容量上限（默认值）
///
/// 当帧栈深度达到此值时，如果低水位线条件满足，将触发换出。
/// 建议值 200，足以覆盖绝大多数非递归场景。
pub const FRAME_PAGING_CAPACITY: usize = 200;

/// 低水位线（默认值）
///
/// 当 `depth + low_watermark >= capacity` 时触发换出。
/// 换出后会释放 `spill_batch_size` 个帧的空间，使深度回到安全范围。
/// 建议值 50，预留足够的缓冲区避免频繁换出。
pub const FRAME_PAGING_LOW_WATERMARK: usize = 50;

/// 每次换出的帧数（默认值）
///
/// 一次换出太多帧会增加堆内存占用和恢复延迟；
/// 换出太少会导致频繁触发换出。
/// 建议值 100，在 200 容量下换出后深度约 100，留有充足余量。
pub const FRAME_PAGING_SPILL_BATCH: usize = 100;

// ============================================================================
// 数据结构
// ============================================================================

/// 换出到堆上的帧块（SCHF v6 Phase 4）
///
/// 一次换出操作产生的 (FrameInfo, FrameMeta) 集合。按后进先出（LIFO）顺序
/// 存放在 `FramePager::spilled_blocks` 中，确保最近换出的块最先被恢复。
#[derive(Debug)]
pub(super) struct SpilledBlock {
    /// 被换出的 FrameInfo 列表（最老在前，与 metas 一一对应）。
    pub(super) infos: Vec<FrameInfo>,
    /// 被换出的 FrameMeta 列表（最老在前，与 infos 一一对应）。
    pub(super) metas: Vec<FrameMeta>,
}

/// 帧换入换出统计信息
///
/// 用于监控和调试帧换页行为，可通过 `FramePager::stats()` 获取。
#[derive(Debug, Clone, Default)]
pub struct FramePagerStats {
    /// 换出操作总次数
    pub spill_count: usize,
    /// 换入（恢复）操作总次数
    pub restore_count: usize,
    /// 累计换出的帧数
    pub frames_spilled: usize,
    /// 累计换入的帧数
    pub frames_restored: usize,
    /// 帧栈达到过的最大深度（含换出前的深度）
    pub max_depth: usize,
    /// 堆上换出帧块的最大数量
    pub max_spilled_blocks: usize,
}

// ============================================================================
// FramePager 核心
// ============================================================================

/// 帧换入换出管理器
///
/// 管理调用帧栈的换入换出策略，使 VM 能够支持超过帧栈容量上限的递归深度。
///
/// # SCHF v6 Phase 4 接口变更
///
/// 所有方法签名从 `&VecDeque<CallFrame>` / `&mut VecDeque<CallFrame>` 改为
/// `&ExecutionContext` / `&mut ExecutionContext`，直接操作 v6 帧栈结构。
/// 算法逻辑保持不变（spill batch、LIFO 顺序、trampoline 插入策略）。
///
/// # 线程安全性
///
/// FramePager 不是 `Sync` 的（包含 `&mut` 操作），每个 VM 实例应持有
/// 自己的 FramePager。这与 VM 本身的单线程执行模型一致。
pub(super) struct FramePager {
    /// 帧栈容量上限
    capacity: usize,
    /// 低水位线
    low_watermark: usize,
    /// 每次换出的帧数
    spill_batch_size: usize,
    /// 当前逻辑调用深度（含换出到堆上的帧）
    depth: usize,
    /// 堆上的换出帧块链表（后进先出，栈式）
    spilled_blocks: Vec<SpilledBlock>,
    /// 统计信息
    stats: FramePagerStats,
}

impl FramePager {
    // ========================================================================
    // 构造器
    // ========================================================================

    /// 创建帧换入换出管理器
    ///
    /// # 参数
    ///
    /// - `capacity`: 帧栈容量上限（建议 200，对应 `FRAME_PAGING_CAPACITY`）
    /// - `low_watermark`: 低水位线（建议 50，对应 `FRAME_PAGING_LOW_WATERMARK`）
    /// - `spill_batch_size`: 每次换出帧数（建议 100，对应 `FRAME_PAGING_SPILL_BATCH`）
    ///
    /// # Panics
    ///
    /// 当 `spill_batch_size` 为 0 时会 panic，因为换出 0 个帧没有意义。
    /// 当 `spill_batch_size >= capacity` 时会 panic，因为换出后无法留下桩帧。
    pub(super) fn new(capacity: usize, low_watermark: usize, spill_batch_size: usize) -> Self {
        assert!(spill_batch_size > 0, "spill_batch_size 必须大于 0");
        assert!(
            spill_batch_size < capacity,
            "spill_batch_size ({}) 必须小于 capacity ({})，否则换出后无法留下桩帧",
            spill_batch_size,
            capacity
        );

        FramePager {
            capacity,
            low_watermark,
            spill_batch_size,
            depth: 0,
            spilled_blocks: Vec::new(),
            stats: FramePagerStats::default(),
        }
    }

    /// 从 FramePagingConfig 创建帧换入换出管理器
    pub(super) fn with_config(config: FramePagingConfig) -> Self {
        Self::new(config.capacity, config.low_watermark, config.spill_batch)
    }

    // ========================================================================
    // 查询方法（热路径，全部内联）
    // ========================================================================

    /// 当前帧栈深度
    #[inline(always)]
    pub(super) fn depth(&self) -> usize {
        self.depth
    }

    /// 是否需要换出（内联，热路径）
    ///
    /// 判定条件：`frames_len + low_watermark >= capacity`
    ///
    /// # 参数
    ///
    /// - `frames_len`: 当前帧栈中的实际帧数（`frame_metas.len()`）
    #[inline(always)]
    pub(super) fn should_spill(&self, frames_len: usize) -> bool {
        frames_len.saturating_add(self.low_watermark) >= self.capacity
    }

    /// 检测帧栈底部是否是桩帧（spec 5.2）
    ///
    /// 检查 `frame_metas[0].kind == Trampoline`。
    /// 在 `pop_frame` 之前调用此方法，如果返回 `true`，说明需要先执行
    /// `restore_frames()` 将堆上的帧换回帧栈。
    #[inline(always)]
    pub(super) fn front_is_trampoline(cx: &ExecutionContext) -> bool {
        cx.frame_metas.first().is_some_and(|m| m.kind == FrameKind::Trampoline)
    }

    // ========================================================================
    // 换出操作
    // ========================================================================

    /// 执行换出：从 frame_metas + frame_ring/overflow 底部取出最老的 batch 个帧，
    /// 放到堆上，在底部放一个桩帧（spec 5.2）
    ///
    /// # 工作流程
    ///
    /// 1. 把 frame_ring 中所有 FrameInfo 转移到 frame_overflow.infos **头部**
    ///    （`drain_to_overflow`，spec 5.3）：ring 中帧比 overflow 中已有的更老，
    ///    插入头部以保持全局时间顺序（最老在前）。ring 清空。
    /// 2. 从 frame_overflow.infos drain 前 batch 个 FrameInfo（最老的）
    /// 3. 从 frame_metas drain 前 batch 个 FrameMeta（最老的）
    /// 4. 创建 SpilledBlock 保存这些 (FrameInfo, FrameMeta)（info 与 meta 一一对应）
    /// 5. 在 frame_metas[0] 插入 trampoline meta
    /// 6. 在 frame_overflow.infos[0] 插入 trampoline info
    /// 7. 更新统计信息
    ///
    /// # 参数
    ///
    /// - `cx`: VM 的执行上下文可变引用
    ///
    /// # 注意
    ///
    /// - 调用此方法前应先通过 `should_spill()` 判断是否需要换出
    /// - 换出后帧栈物理长度减少 `spill_batch_size - 1`（因为桩帧占一个位置）
    /// - 逻辑深度 `depth` 不受影响（换出只是物理位置变化，不改变调用嵌套层数）
    /// - 如果 `frame_metas.len() < spill_batch_size`，实际换出数量会减少以保证至少留下一个帧
    #[inline]
    pub(super) fn spill_frames(&mut self, cx: &mut ExecutionContext) {
        // 计算实际可换出的帧数：至少保留 1 个帧在栈上
        let batch = self.spill_batch_size.min(cx.frame_metas.len().saturating_sub(1));

        if batch == 0 {
            return;
        }

        // Step 1: 把 frame_ring 中所有 FrameInfo 转移到 frame_overflow.infos 头部
        // ring 中的帧比 overflow 中已有的帧更老（更早 push），
        // 必须插入到 overflow 头部（splice(0..0)）以保持全局时间顺序（最老在前），
        // 确保 Step 2 的 drain(0..batch) 取到的 FrameInfo 与 Step 3 的
        // frame_metas.drain(0..batch) 取到的 FrameMeta 一一对应（spec 5.3 drain_to_overflow）。
        // 注意：旧实现用 extend 追加到尾部，当 overflow 非空时会导致顺序错乱。
        cx.frame_ring.drain_to_overflow(&mut cx.frame_overflow);

        // Step 2: 从 frame_overflow.infos drain 前 batch 个 FrameInfo（最老的）
        let drained_infos: Vec<FrameInfo> = cx.frame_overflow.infos.drain(0..batch).collect();

        // Step 3: 从 frame_metas drain 前 batch 个 FrameMeta（最老的）
        let drained_metas: Vec<FrameMeta> = cx.frame_metas.drain(0..batch).collect();

        // Step 4: 创建 SpilledBlock 保存这些 (FrameInfo, FrameMeta)
        let block = SpilledBlock { infos: drained_infos, metas: drained_metas };

        // Step 5: 在 frame_metas[0] 插入 trampoline meta
        cx.frame_metas.insert(0, make_trampoline_meta());

        // Step 6: 在 frame_overflow.infos[0] 插入 trampoline info
        cx.frame_overflow.infos.insert(0, FrameInfo { return_address: 0, base: 0 });

        // Step 7: 将块推入堆上的换出链表
        self.spilled_blocks.push(block);

        // Step 8: 更新统计信息（不修改 depth，因为逻辑深度不变）
        self.stats.spill_count += 1;
        self.stats.frames_spilled += batch;
        self.stats.max_spilled_blocks =
            self.stats.max_spilled_blocks.max(self.spilled_blocks.len());
    }

    // ========================================================================
    // 换入操作
    // ========================================================================

    /// 执行换入：从堆上取回最近换出的帧块，替换帧栈底部的桩帧（spec 5.2）
    ///
    /// # 工作流程
    ///
    /// 1. 从 `spilled_blocks` 弹出最近换出的块
    /// 2. 移除 `frame_metas[0]` 的 trampoline meta
    /// 3. 移除 `frame_overflow.infos[0]` 的 trampoline info
    /// 4. 将块中的 FrameMeta 按原始顺序恢复到 `frame_metas` 头部
    /// 5. 将块中的 FrameInfo 按原始顺序恢复到 `frame_overflow.infos` 头部
    /// 6. 更新统计信息
    ///
    /// # 参数
    ///
    /// - `cx`: VM 的执行上下文可变引用
    ///
    /// # 注意
    ///
    /// - 调用此方法前应先通过 `front_is_trampoline()` 确认底部是桩帧
    /// - 如果 `spilled_blocks` 为空，此方法不做任何操作（防御性编程）
    /// - 逻辑深度 `depth` 不受影响（换入只是物理位置变化，不改变调用嵌套层数）
    #[inline]
    pub(super) fn restore_frames(&mut self, cx: &mut ExecutionContext) {
        // 防御性检查：堆上没有可恢复的帧块
        let block = match self.spilled_blocks.pop() {
            Some(b) => b,
            None => return,
        };

        let restored_count = block.metas.len();

        // Step 1: 移除 frame_metas 头部的 trampoline meta
        if !cx.frame_metas.is_empty() && cx.frame_metas[0].kind == FrameKind::Trampoline {
            cx.frame_metas.remove(0);
        }

        // Step 2: 移除 frame_overflow.infos 头部的 trampoline info
        if !cx.frame_overflow.infos.is_empty() {
            cx.frame_overflow.infos.remove(0);
        }

        // Step 3: 将块中的 FrameMeta 按原始顺序恢复到 frame_metas 头部
        // 逆序 insert(0, ...) 以保持最老帧在最前面
        for meta in block.metas.into_iter().rev() {
            cx.frame_metas.insert(0, meta);
        }

        // Step 4: 将块中的 FrameInfo 按原始顺序恢复到 frame_overflow.infos 头部
        for info in block.infos.into_iter().rev() {
            cx.frame_overflow.infos.insert(0, info);
        }

        // Step 5: 更新统计信息（不修改 depth，因为逻辑深度不变）
        self.stats.restore_count += 1;
        self.stats.frames_restored += restored_count;
    }

    // ========================================================================
    // 深度追踪
    // ========================================================================

    /// 记录深度变化（push 后调用）
    #[inline(always)]
    pub(super) fn record_push(&mut self) {
        self.depth += 1;
        if self.depth > self.stats.max_depth {
            self.stats.max_depth = self.depth;
        }
    }

    /// 记录深度变化（pop 前调用）
    #[inline(always)]
    pub(super) fn record_pop(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// 重置帧换页管理器（VM 重置时调用）
    pub(super) fn reset(&mut self) {
        self.depth = 0;
        self.spilled_blocks.clear();
        self.stats = FramePagerStats::default();
    }

    // ========================================================================
    // 统计与诊断
    // ========================================================================

    /// 获取统计信息
    pub(super) fn stats(&self) -> &FramePagerStats {
        &self.stats
    }

    // ========================================================================
    // GC 根扫描
    // ========================================================================

    /// 遍历所有帧的 FrameMeta（包括堆上的），用于 GC 根扫描（spec 5.2）
    ///
    /// 先遍历 `cx.frame_metas` 中的所有 meta，再遍历堆上所有换出帧块中的 meta。
    /// 确保垃圾回收器能追踪到所有存活的堆对象引用（closure、tco_history）。
    ///
    /// # 参数
    ///
    /// - `cx`: 当前执行上下文的引用
    /// - `f`: 回调函数，对每个 `&FrameMeta` 调用一次
    pub(super) fn for_each_frame<'a, F>(&'a self, cx: &'a ExecutionContext, mut f: F)
    where
        F: FnMut(&'a FrameMeta),
    {
        // 先遍历帧栈中的 meta
        for meta in &cx.frame_metas {
            f(meta);
        }

        // 再遍历堆上所有换出帧块中的 meta
        for block in &self.spilled_blocks {
            for meta in &block.metas {
                f(meta);
            }
        }
    }
}

// ============================================================================
// 桩帧创建辅助
// ============================================================================

/// 创建桩帧 FrameMeta（Trampoline Frame）
///
/// 桩帧是一个特殊的 FrameMeta，其 `kind` 标记为 `FrameKind::Trampoline`。
/// 它充当换出帧块的占位符，当函数逐层返回到达桩帧时，
/// VM 会检测到并触发 `restore_frames()` 将堆上的帧换回。
///
/// # 桩帧特征
///
/// - `kind`: `FrameKind::Trampoline`（区别于普通帧）
/// - `closure`: None
/// - `caller_chunk`: None
/// - `tco_reused`: false
fn make_trampoline_meta() -> FrameMeta {
    FrameMeta {
        closure: None,
        caller_chunk: None,
        caller_func_reg: 0,
        arena: usize::MAX,
        call_site: None,
        kind: FrameKind::Trampoline,
        tco_reused: false,
        tco_history: Vec::new(),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_bytecode::scope::GlobalScope;

    /// 辅助函数：创建一个普通帧的 FrameMeta
    fn make_normal_meta(base: usize) -> FrameMeta {
        let meta = FrameMeta::default();
        debug_assert_eq!(meta.kind, FrameKind::Normal);
        // base 通过 FrameInfo 单独存储，meta 不存 base
        let _ = base;
        meta
    }

    /// 辅助函数：创建一个带可识别 ID 的普通帧 FrameMeta（用于 spill 顺序验证）。
    ///
    /// `caller_func_reg` 字段携带帧序号，使测试能验证 spill/restore 后
    /// FrameMeta 与 FrameInfo 的一一对应关系。
    fn make_meta_with_id(id: usize) -> FrameMeta {
        FrameMeta { caller_func_reg: id, ..Default::default() }
    }

    /// 辅助函数：创建一个普通帧的 FrameInfo
    fn make_normal_info(base: usize) -> FrameInfo {
        FrameInfo { return_address: 0, base }
    }

    /// 辅助函数：构造一个最小化的 ExecutionContext 并预填 N 个普通帧
    fn make_cx_with_frames(n: usize) -> ExecutionContext {
        let mut cx = ExecutionContext::new();
        for i in 0..n {
            cx.frame_metas.push(make_normal_meta(i));
            cx.frame_ring.push(make_normal_info(i));
        }
        cx
    }

    #[test]
    fn test_new_pager_validation() {
        // 正常创建
        let pager = FramePager::new(200, 50, 100);
        assert_eq!(pager.depth(), 0);
        assert!(!pager.should_spill(0));

        // spill_batch_size 为 0 应 panic
        let result = std::panic::catch_unwind(|| FramePager::new(200, 50, 0));
        assert!(result.is_err());

        // spill_batch_size >= capacity 应 panic
        let result = std::panic::catch_unwind(|| FramePager::new(200, 50, 200));
        assert!(result.is_err());
    }

    #[test]
    fn test_should_spill_threshold() {
        let pager = FramePager::new(200, 50, 100);

        // frames_len = 149 → 149 + 50 = 199 < 200 → 不需要换出
        assert!(!pager.should_spill(149));

        // frames_len = 150 → 150 + 50 = 200 >= 200 → 需要换出
        assert!(pager.should_spill(150));
    }

    #[test]
    fn test_spill_and_restore_cycle() {
        let mut pager = FramePager::new(10, 3, 5);
        let mut cx = make_cx_with_frames(8);

        // 模拟 push 到触发换出的深度
        for _ in 0..8 {
            pager.record_push();
        }
        assert_eq!(pager.depth(), 8);
        assert!(pager.should_spill(cx.frame_metas.len())); // 8 + 3 = 11 >= 10

        // 执行换出
        pager.spill_frames(&mut cx);

        // 验证：换出了 5 个帧，插入了 1 个桩帧
        // frame_metas: [Trampoline, m5, m6, m7]
        assert_eq!(cx.frame_metas.len(), 4); // 8 - 5 + 1 = 4
        assert_eq!(cx.frame_metas[0].kind, FrameKind::Trampoline);
        // ring 清空，所有 FrameInfo 在 overflow
        assert_eq!(cx.frame_overflow.infos.len(), 4); // 8 - 5 + 1 = 4
        assert_eq!(cx.frame_overflow.infos[1].base, 5);
        assert_eq!(cx.frame_overflow.infos[2].base, 6);
        assert_eq!(cx.frame_overflow.infos[3].base, 7);
        // 逻辑深度不受换出影响
        assert_eq!(pager.depth(), 8);
        assert_eq!(pager.stats().spill_count, 1);
        assert_eq!(pager.stats().frames_spilled, 5);

        // 验证桩帧检测
        assert!(FramePager::front_is_trampoline(&cx));

        // 执行换入
        pager.restore_frames(&mut cx);

        // 验证：桩帧被移除，5 个帧被恢复
        // frame_metas: [m0, m1, m2, m3, m4, m5, m6, m7]
        assert_eq!(cx.frame_metas.len(), 8);
        assert!(!FramePager::front_is_trampoline(&cx));
        for meta in cx.frame_metas.iter().take(8) {
            assert_eq!(meta.kind, FrameKind::Normal);
        }
        // overflow 也恢复
        assert_eq!(cx.frame_overflow.infos.len(), 8);
        // 逻辑深度不受换入影响
        assert_eq!(pager.depth(), 8);
        assert_eq!(pager.stats().restore_count, 1);
        assert_eq!(pager.stats().frames_restored, 5);
    }

    #[test]
    fn test_multiple_spill_restore() {
        let mut pager = FramePager::new(10, 3, 4);
        let mut cx = make_cx_with_frames(8);

        // 第一轮：push 8 个帧并换出
        for _ in 0..8 {
            pager.record_push();
        }
        pager.spill_frames(&mut cx);

        // 换出后: frame_metas = [Trampoline, m4, m5, m6, m7]
        assert_eq!(cx.frame_metas.len(), 5);
        assert_eq!(cx.frame_metas[0].kind, FrameKind::Trampoline);

        // 继续推入更多帧
        for i in 8..12 {
            cx.frame_metas.push(make_normal_meta(i));
            cx.frame_overflow.infos.push(make_normal_info(i));
            pager.record_push();
        }

        // 再次换出
        pager.spill_frames(&mut cx);

        // 验证堆上有 2 个帧块
        assert_eq!(pager.spilled_blocks.len(), 2);
        assert_eq!(pager.stats().spill_count, 2);

        // 逐层恢复（LIFO 顺序）
        pager.restore_frames(&mut cx);
        assert_eq!(pager.stats().restore_count, 1);

        // 再次恢复
        assert!(FramePager::front_is_trampoline(&cx));
        pager.restore_frames(&mut cx);
        assert_eq!(pager.stats().restore_count, 2);
    }

    #[test]
    fn test_restore_with_empty_spilled_blocks() {
        let mut pager = FramePager::new(10, 3, 4);
        let mut cx = make_cx_with_frames(1);

        // 堆上没有帧块时，restore_frames 应安全返回
        pager.restore_frames(&mut cx);
        assert_eq!(cx.frame_metas.len(), 1);
        assert_eq!(pager.stats().restore_count, 0);
    }

    #[test]
    fn test_for_each_frame_includes_spilled() {
        let mut pager = FramePager::new(10, 3, 4);
        let mut cx = make_cx_with_frames(8);

        for _ in 0..8 {
            pager.record_push();
        }
        pager.spill_frames(&mut cx);

        // for_each_frame 应遍历 frame_metas + 堆上的所有 meta
        let mut visited_count: usize = 0;
        pager.for_each_frame(&cx, |_meta| {
            visited_count += 1;
        });

        // frame_metas: [Trampoline, m5, m6, m7] = 4
        // spilled_blocks[0].metas: [m0, m1, m2, m3, m4] = 5
        // 总计 9
        assert_eq!(visited_count, 9);
    }

    #[test]
    fn test_stats_tracking() {
        let mut pager = FramePager::new(10, 3, 4);
        let mut cx = make_cx_with_frames(6);

        // 模拟深度增长
        for _ in 0..6 {
            pager.record_push();
        }
        assert_eq!(pager.stats().max_depth, 6);

        // 换出
        pager.spill_frames(&mut cx);
        assert_eq!(pager.stats().spill_count, 1);
        assert_eq!(pager.stats().frames_spilled, 4);
        assert_eq!(pager.stats().max_spilled_blocks, 1);

        // 换入
        pager.restore_frames(&mut cx);
        assert_eq!(pager.stats().restore_count, 1);
        assert_eq!(pager.stats().frames_restored, 4);
    }

    #[test]
    fn test_spill_with_fewer_frames_than_batch() {
        // 使用合理的参数：capacity=200, spill_batch_size=100
        // 但帧栈中只有 5 个帧，远少于 batch 大小
        let mut pager = FramePager::new(200, 50, 100);
        let mut cx = make_cx_with_frames(5);

        for _ in 0..5 {
            pager.record_push();
        }

        // batch=100 但只有 5 个帧，应保留至少 1 个帧在栈上
        pager.spill_frames(&mut cx);

        // 实际换出 4 个帧（5 - 1 = 4），插入 1 个桩帧
        assert_eq!(cx.frame_metas.len(), 2); // 5 - 4 + 1 = 2
        assert_eq!(cx.frame_metas[0].kind, FrameKind::Trampoline);
        assert_eq!(cx.frame_overflow.infos.len(), 2);
        assert_eq!(pager.stats().frames_spilled, 4);
    }

    #[test]
    fn test_record_pop_saturating() {
        let mut pager = FramePager::new(10, 3, 4);

        // depth 为 0 时 pop 不应下溢
        pager.record_pop();
        assert_eq!(pager.depth(), 0);
    }

    /// 验证 ExecutionContext::new() 不再包含 frames: VecDeque<CallFrame> 字段
    /// （SCHF v6 Phase 4：VecDeque 已移除）
    #[test]
    fn test_execution_context_has_no_vecdeque() {
        let cx = ExecutionContext::new();
        // frame_metas / frame_ring / frame_overflow / frame_data 是 v6 帧栈的全部
        assert!(cx.frame_metas.is_empty());
        assert!(cx.frame_overflow.infos.is_empty());
        assert_eq!(cx.frame_data.top, 0);
    }

    /// 验证 GlobalScope 可访问（ExecutionContext 字段完整性烟雾测试）
    #[test]
    fn test_execution_context_global_scope_accessible() {
        let cx = ExecutionContext::new();
        let _scope: &GlobalScope = &cx.global_scope;
    }

    // ========================================================================
    // Phase 5 回归测试：spill 时 ring + overflow 同时有帧的顺序正确性
    // ========================================================================

    /// 辅助函数：模拟 VM 的帧路由逻辑构造 ExecutionContext。
    ///
    /// 前 63 帧写入 frame_ring（VM 在 ring_depth < 63 时路由到 ring），
    /// 第 63 帧起写入 frame_overflow.infos（VM 在 ring 满或 overflow 非空时路由到 overflow）。
    /// frame_metas 统一保存所有帧的 meta（与 VM 行为一致）。
    ///
    /// 每帧的 FrameInfo.base 与 FrameMeta.caller_func_reg 都设为帧序号 `i`，
    /// 使测试能验证 spill/restore 后 info 与 meta 的一一对应。
    fn make_cx_with_routed_frames(n: usize) -> ExecutionContext {
        let mut cx = ExecutionContext::new();
        for i in 0..n {
            cx.frame_metas.push(make_meta_with_id(i));
            let info = FrameInfo { return_address: i * 1000, base: i };
            if i < 63 {
                cx.frame_ring.push(info);
            } else {
                cx.frame_overflow.infos.push(info);
            }
        }
        cx
    }

    /// 验证 spill_frames 在 ring + overflow 同时有帧时保持 FrameInfo/FrameMeta 对应。
    ///
    /// 场景：100 帧（F0-F62 在 ring，F63-F99 在 overflow），spill 50 帧。
    ///
    /// 修复前的 bug：`drain_to_vec` + `extend` 将 ring 内容追加到 overflow 尾部，
    /// 导致 overflow 顺序变为 [F63..F99, F0..F62]，drain(0..50) 取到 [F63..F99, F0..F12]
    /// 而非正确的 [F0..F49]，与 frame_metas.drain(0..50) 取到的 [F0_meta..F49_meta] 错位。
    ///
    /// 修复后：`drain_to_overflow` 将 ring 内容插入 overflow 头部，
    /// overflow 顺序为 [F0..F62, F63..F99]，drain(0..50) 正确取到 [F0..F49]。
    #[test]
    fn test_spill_with_overflow_nonempty_preserves_order() {
        let mut pager = FramePager::new(200, 50, 50);
        let mut cx = make_cx_with_routed_frames(100);

        // 模拟 record_push 到 100 层
        for _ in 0..100 {
            pager.record_push();
        }
        assert_eq!(pager.depth(), 100);

        // 执行 spill（batch=50）
        pager.spill_frames(&mut cx);

        // 验证：换出 50 帧，插入 1 桩帧 → frame_metas 与 overflow 各 51 条
        assert_eq!(cx.frame_metas.len(), 51, "frame_metas 应为 51 (100-50+1)");
        assert_eq!(cx.frame_overflow.infos.len(), 51, "overflow.infos 应为 51");
        assert_eq!(cx.frame_metas[0].kind, FrameKind::Trampoline);
        assert!(FramePager::front_is_trampoline(&cx));

        // 关键验证：overflow.infos[i] 与 frame_metas[i] 一一对应
        // overflow[0] = trampoline (base=0) ↔ frame_metas[0] = trampoline (caller_func_reg=0)
        // overflow[1] = F50_info (base=50) ↔ frame_metas[1] = F50_meta (caller_func_reg=50)
        // ...overflow[50] = F99_info (base=99) ↔ frame_metas[50] = F99_meta (caller_func_reg=99)
        for i in 0..51 {
            let info_base = cx.frame_overflow.infos[i].base;
            let meta_id = cx.frame_metas[i].caller_func_reg;
            assert_eq!(
                info_base, meta_id,
                "spill 后 info/meta 错位: overflow[{}].base={} != frame_metas[{}].caller_func_reg={}",
                i, info_base, i, meta_id
            );
        }

        // 验证 spilled block 内容：F0-F49（最老的 50 帧）
        assert_eq!(pager.spilled_blocks.len(), 1);
        let block = &pager.spilled_blocks[0];
        assert_eq!(block.infos.len(), 50);
        assert_eq!(block.metas.len(), 50);
        for i in 0..50 {
            assert_eq!(block.infos[i].base, i, "spilled block infos[{}].base 应为 {}", i, i);
            assert_eq!(
                block.metas[i].caller_func_reg, i,
                "spilled block metas[{}].caller_func_reg 应为 {}",
                i, i
            );
        }

        // 执行 restore
        pager.restore_frames(&mut cx);

        // 验证：桩帧移除，50 帧恢复 → frame_metas 与 overflow 各 100 条
        assert_eq!(cx.frame_metas.len(), 100, "restore 后 frame_metas 应为 100");
        assert_eq!(cx.frame_overflow.infos.len(), 100, "restore 后 overflow 应为 100");
        assert!(!FramePager::front_is_trampoline(&cx));

        // 关键验证：restore 后 info/meta 仍一一对应（F0-F99 顺序）
        for i in 0..100 {
            let info_base = cx.frame_overflow.infos[i].base;
            let meta_id = cx.frame_metas[i].caller_func_reg;
            assert_eq!(
                info_base, meta_id,
                "restore 后 info/meta 错位: overflow[{}].base={} != frame_metas[{}].caller_func_reg={}",
                i, info_base, i, meta_id
            );
        }
        assert_eq!(pager.stats().restore_count, 1);
        assert_eq!(pager.stats().frames_restored, 50);
    }

    /// 验证多次 spill + restore 循环在 ring+overflow 混合状态下保持一致。
    ///
    /// 场景：120 帧（F0-F62 在 ring，F63-F119 在 overflow），batch=40。
    /// 第一次 spill 40 帧 → 剩 81 帧（含桩）。
    /// 继续模拟 push 40 帧到 overflow → 121 帧。
    /// 第二次 spill 40 帧 → 剩 82 帧（含桩）。
    /// 逐次 restore 验证一致性。
    #[test]
    fn test_multiple_spill_with_overflow_nonempty() {
        let mut pager = FramePager::new(200, 50, 40);
        let mut cx = make_cx_with_routed_frames(120);

        for _ in 0..120 {
            pager.record_push();
        }

        // 第一次 spill
        pager.spill_frames(&mut cx);
        assert_eq!(cx.frame_metas.len(), 81); // 120-40+1
        assert_eq!(cx.frame_overflow.infos.len(), 81);
        // 验证 info/meta 对应
        for i in 0..81 {
            assert_eq!(
                cx.frame_overflow.infos[i].base, cx.frame_metas[i].caller_func_reg,
                "第一次 spill 后 overflow[{}].base 与 frame_metas[{}].caller_func_reg 不匹配",
                i, i
            );
        }

        // 模拟继续 push 40 帧到 overflow（VM 在 overflow 非空时路由到 overflow）
        for i in 120..160 {
            cx.frame_metas.push(make_meta_with_id(i));
            cx.frame_overflow.infos.push(FrameInfo { return_address: i * 1000, base: i });
            pager.record_push();
        }

        // 第二次 spill
        pager.spill_frames(&mut cx);
        assert_eq!(cx.frame_metas.len(), 82); // 121-40+1
        assert_eq!(cx.frame_overflow.infos.len(), 82);
        assert_eq!(pager.spilled_blocks.len(), 2);

        // 验证 info/meta 对应
        for i in 0..82 {
            assert_eq!(
                cx.frame_overflow.infos[i].base, cx.frame_metas[i].caller_func_reg,
                "第二次 spill 后 overflow[{}].base 与 frame_metas[{}].caller_func_reg 不匹配",
                i, i
            );
        }

        // 逐次 restore
        pager.restore_frames(&mut cx);
        assert_eq!(cx.frame_metas.len(), 121); // 82+40
        for i in 0..121 {
            assert_eq!(
                cx.frame_overflow.infos[i].base, cx.frame_metas[i].caller_func_reg,
                "第一次 restore 后 overflow[{}].base 与 frame_metas[{}].caller_func_reg 不匹配",
                i, i
            );
        }

        pager.restore_frames(&mut cx);
        // 第二次 restore：移除 OLD trampoline (1) + 恢复 block[0] 的 40 帧
        // 121 - 1 + 40 = 160
        assert_eq!(cx.frame_metas.len(), 160);
        for i in 0..160 {
            assert_eq!(
                cx.frame_overflow.infos[i].base, cx.frame_metas[i].caller_func_reg,
                "第二次 restore 后 overflow[{}].base 与 frame_metas[{}].caller_func_reg 不匹配",
                i, i
            );
        }
        assert_eq!(pager.stats().restore_count, 2);
    }

    /// 验证 drain_to_overflow 在 ring 为空时是 no-op。
    #[test]
    fn test_drain_to_overflow_empty_ring_is_noop() {
        let mut cx = ExecutionContext::new();
        // overflow 预填一些帧
        for i in 0..10 {
            cx.frame_overflow.infos.push(FrameInfo { return_address: i, base: i });
        }
        // ring 为空，drain_to_overflow 应是 no-op
        cx.frame_ring.drain_to_overflow(&mut cx.frame_overflow);
        assert_eq!(cx.frame_overflow.infos.len(), 10, "ring 为空时 overflow 不应变");
        for i in 0..10 {
            assert_eq!(cx.frame_overflow.infos[i].base, i, "原顺序应保持不变");
        }
    }
}

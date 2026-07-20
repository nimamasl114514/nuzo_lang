//! SCHF v6 — Contiguous Frame Stack with Bump Pointer
//!
//! Phase 1：数据结构搭建（不改行为）。本模块定义 v6 帧栈的核心数据结构，
//! 与现有 `VecDeque<CallFrame>` 并存，不影响 `push_frame`/`pop_frame` 现有逻辑。
//! Phase 2 将切换调用路径到本模块。
//!
//! 设计目标（spec 第 2 章）：
//! - [`FrameInfo`]：16B 紧凑环形槽，缓存行友好
//! - [`FrameRing`]：固定 64 槽环形缓冲，push/pop 仅位运算（`& 63`）
//! - [`FrameData`]：单一连续值栈，帧 = `data[base..base+n_cip]`
//! - [`FrameMeta`]：冷路径字段分离，不污染热路径缓存行
//! - [`OverflowStack`]：ring 溢出降级（>64 层递归）

// Phase 2 已接入 push_frame/pop_frame 影子写入路径（FrameRing/FrameMeta/FrameData）。
// 未使用的方法/字段预留待 Phase 3（读取路径切换）启用，单独标记 allow(dead_code)。

use std::sync::Arc;

use nuzo_bytecode::Chunk;
use nuzo_values::{HeapObject, NIL, SourceLocation};

use super::{FrameKind, TcoRecord};

// ============================================================================
// FrameInfo — 16B 环形槽（spec 2.1）
// ============================================================================

/// 帧元信息，仅控制字段。数据（locals + spill）在 [`FrameData`] 中。
///
/// 16B 紧凑结构（`usize * 2`），放入 [`FrameRing`] 槽位以保持缓存行友好。
/// `n_cip` 不需要存储：pop 时用 `base` 回退 `FrameData::top` 即可。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameInfo {
    /// pop 时恢复的 IP 地址。
    pub return_address: usize,
    /// `FrameData` 中的起始偏移（帧的 locals 区起点）。
    pub base: usize,
}

// ============================================================================
// FrameRing — 固定 64 槽环形缓冲（spec 2.2）
// ============================================================================

/// 固定大小环形帧信息缓冲区。
///
/// 64 槽环形缓冲，push/pop 仅需位运算（`& 63`）。
/// 深度 > 64 时由调用方降级到 [`OverflowStack`]。
///
/// **空状态**：`head == 0`。调用方必须保证：
/// - push/pop 平衡
/// - 任意时刻深度（已 push - 已 pop）< 64
///
/// 否则 `head` 回绕到 0 会被误判为空。spec 设计由调用方在 push 前检查深度，
/// 达到 64 时改走 `OverflowStack`，故 ring 内部不做溢出检测。
pub(crate) struct FrameRing {
    slots: [FrameInfo; 64],
    head: u8,
}

impl FrameRing {
    const CAPACITY: usize = 64;
    const MASK: u8 = 63;

    /// 创建空 ring，所有槽位初始化为零值 `FrameInfo`。
    pub(super) fn new() -> Self {
        Self { slots: [FrameInfo { return_address: 0, base: 0 }; Self::CAPACITY], head: 0 }
    }

    /// 压入一个 `FrameInfo`（spec 2.2 push，2 条指令）。
    ///
    /// **不检查溢出**：调用方负责在深度将达到 64 时降级到 `OverflowStack`。
    #[inline]
    pub(crate) fn push(&mut self, info: FrameInfo) {
        self.slots[self.head as usize] = info;
        self.head = self.head.wrapping_add(1) & Self::MASK;
    }

    /// 弹出一个 `FrameInfo`（spec 2.2 pop）。
    ///
    /// 返回 `None` 表示栈空（`StackUnderflow`）。
    #[inline]
    pub(super) fn pop(&mut self) -> Option<FrameInfo> {
        if self.head == 0 {
            return None;
        }
        self.head = self.head.wrapping_sub(1) & Self::MASK;
        Some(self.slots[self.head as usize])
    }

    /// 返回栈顶 `FrameInfo` 的不可变引用（spec 4.4 当前帧访问）。
    ///
    /// `head == 0` 时返回 `None`。Phase 3 起所有 `frames.back()` 读取
    /// 改为先查 `OverflowStack`，再调用本方法。
    #[inline]
    pub(super) fn back(&self) -> Option<&FrameInfo> {
        if self.head == 0 {
            return None;
        }
        let idx = (self.head.wrapping_sub(1)) & Self::MASK;
        Some(&self.slots[idx as usize])
    }

    /// 返回当前 ring 深度（spec 4.4 frame_depth 组成部分）。
    ///
    /// `head` 即当前 ring 中已 push 但未 pop 的 FrameInfo 数。
    /// spill 后被 `clear()` 重置为 0，故 spill 后 ring 深度归零。
    #[inline]
    #[allow(dead_code)] // Phase 4: build_call_stack 改用 iter()，len() 保留供诊断/测试使用
    pub(super) fn len(&self) -> usize {
        self.head as usize
    }

    /// 按推送顺序（最老在前）迭代 ring 中的 FrameInfo（spec 4.4 build_call_stack）。
    ///
    /// 返回 `slots[0..head]` 的迭代器。spill 后 `head == 0`，返回空迭代器。
    /// 用于 `build_call_stack` 重建每帧的 `base` 字段（FrameMeta 不存 base）。
    #[inline]
    pub(super) fn iter(&self) -> std::slice::Iter<'_, FrameInfo> {
        self.slots[..self.head as usize].iter()
    }

    /// 重置为空状态（不清零 `slots`，仅重置 `head`）。
    #[inline]
    pub(super) fn clear(&mut self) {
        self.head = 0;
    }

    /// Phase 4 启用：将 ring 中所有 FrameInfo 按推送顺序（最老在前）转移到 Vec，并清空 ring。
    ///
    /// 用于 FramePager spill 时把 ring 内容转移到 overflow_stack（spec 5.2/5.3）。
    /// 调用后 `head == 0`，ring 回到空状态。
    #[allow(dead_code)] // Phase 5 改用 drain_to_overflow；保留供诊断/测试使用
    pub(crate) fn drain_to_vec(&mut self) -> Vec<FrameInfo> {
        let head = self.head as usize;
        let result: Vec<FrameInfo> = self.slots[..head].to_vec();
        self.head = 0;
        result
    }

    /// Phase 5：将 ring 中所有 FrameInfo 转移到 overflow 头部（spec 5.3 drain_to_overflow）。
    ///
    /// ring 中的 FrameInfo 比 OverflowStack 中已有的更老（更早 push），
    /// 因此必须 **插入到 overflow.infos 头部**（而非尾部 extend），以保持
    /// 全局时间顺序（最老在前）。这确保 `spill_frames` 的 `drain(0..batch)`
    /// 能取到真正最老的帧，与 `frame_metas.drain(0..batch)` 的 FrameMeta 对齐。
    ///
    /// # 工作流程
    ///
    /// 1. 取出 ring 中 `slots[..head]` 的 FrameInfo（按 push 顺序，最老在前）
    /// 2. 用 `splice(0..0, ..)` 插入到 overflow.infos 头部
    /// 3. 重置 ring `head = 0`，回到空状态
    ///
    /// # 参数
    ///
    /// - `overflow`: 目标溢出栈，ring 内容将插入到其 `infos` 头部
    ///
    /// # 注意
    ///
    /// - 若 ring 为空（`head == 0`），此方法是 no-op
    /// - 调用后 ring 回到空状态，可重新用于后续 push
    /// - FrameMeta 不需要转移：`frame_metas` 是统一 Vec，spill 时单独 drain
    pub(crate) fn drain_to_overflow(&mut self, overflow: &mut OverflowStack) {
        let head = self.head as usize;
        if head == 0 {
            return;
        }
        // splice(0..0, iter) 在 Vec 头部插入，保持 ring（更老）在 overflow（较新）之前
        overflow.infos.splice(0..0, self.slots[..head].iter().copied());
        self.head = 0;
    }
}

impl Default for FrameRing {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// FrameData — 连续值栈（spec 2.3）
// ============================================================================

/// 单一连续值栈。帧 = `data[base..base+n_cip]`。
///
/// 预分配 `capacity = max_stack_size`，运行期 bump pointer 推进 `top`。
#[derive(Default)]
pub(crate) struct FrameData {
    /// 预分配的值栈存储。
    pub data: Vec<nuzo_core::Value>,
    /// bump pointer，指向下一个空闲 slot。
    pub top: usize,
}

impl FrameData {
    /// 创建指定容量的空 `FrameData`。
    #[allow(dead_code)] // Phase 3 启用：FrameData 预分配路径
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { data: Vec::with_capacity(capacity), top: 0 }
    }

    /// 将 `[base, base+count)` 区间填充为 `NIL`（spec 2.3 locals 初始化）。
    ///
    /// 自动裁剪到 `data.len()`，避免越界。调用方负责先 `resize` 到足够长度。
    #[inline]
    pub(super) fn fill_nil(&mut self, base: usize, count: usize) {
        let end = base.saturating_add(count).min(self.data.len());
        if base < end {
            self.data[base..end].fill(NIL);
        }
    }

    /// 重置为空状态（保留已分配 `capacity`，仅重置 `top`）。
    #[inline]
    pub(super) fn clear(&mut self) {
        self.top = 0;
    }
}

// ============================================================================
// FrameMeta — 辅助帧元数据（spec 2.4）
// ============================================================================

/// 非热路径帧元数据。
///
/// 仅在冷路径访问（chunk 切换、arena 逃逸检测、错误诊断），
/// 不放入 [`FrameRing`] 以保持 [`FrameInfo`] 16B 紧凑。
#[derive(Debug, Clone)]
pub(crate) struct FrameMeta {
    pub closure: Option<Arc<HeapObject>>,
    pub caller_func_reg: usize,
    pub arena: usize,
    pub kind: FrameKind,
    // Phase 4 启用：VecDeque 移除后，这些字段成为唯一数据源
    pub caller_chunk: Option<Arc<Chunk>>,
    pub call_site: Option<SourceLocation>,
    pub tco_reused: bool,
    pub tco_history: Vec<TcoRecord>,
}

impl FrameMeta {
    /// 创建默认的 `Normal` 帧 meta（与 `CallFrame::new` 字段对齐）。
    pub(super) fn new() -> Self {
        Self {
            closure: None,
            caller_chunk: None,
            caller_func_reg: 0,
            arena: usize::MAX,
            call_site: None,
            kind: FrameKind::Normal,
            tco_reused: false,
            tco_history: Vec::new(),
        }
    }
}

impl Default for FrameMeta {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// OverflowStack — ring 溢出降级（spec 2.5）
// ============================================================================

/// ring 溢出降级存储。
///
/// 当 ring 深度 > 64 时，将 ring 中所有 `FrameInfo` + 对应 `FrameMeta` 转移到此处。
/// 极罕见（64+ 层递归），O(n) 可接受。
pub(crate) struct OverflowStack {
    pub infos: Vec<FrameInfo>,
    #[allow(dead_code)] // Phase 3 启用：ring 溢出时同步转移 FrameMeta
    pub metas: Vec<FrameMeta>,
}

impl OverflowStack {
    pub(super) fn new() -> Self {
        Self { infos: Vec::new(), metas: Vec::new() }
    }

    #[inline]
    pub(super) fn clear(&mut self) {
        self.infos.clear();
        self.metas.clear();
    }
}

impl Default for OverflowStack {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试（spec 2.2 push/pop 语义验证）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_ring_push_pop_basic() {
        let mut ring = FrameRing::new();
        assert!(ring.pop().is_none(), "新建 ring 必须为空");

        ring.push(FrameInfo { return_address: 42, base: 100 });
        let info = ring.pop().expect("push 后 pop 必须返回 Some");
        assert_eq!(info.return_address, 42);
        assert_eq!(info.base, 100);
        assert!(ring.pop().is_none(), "pop 后 ring 必须为空");
    }

    #[test]
    fn test_frame_ring_lifo_order() {
        let mut ring = FrameRing::new();
        ring.push(FrameInfo { return_address: 1, base: 10 });
        ring.push(FrameInfo { return_address: 2, base: 20 });
        ring.push(FrameInfo { return_address: 3, base: 30 });

        assert_eq!(ring.pop().unwrap().return_address, 3);
        assert_eq!(ring.pop().unwrap().return_address, 2);
        assert_eq!(ring.pop().unwrap().return_address, 1);
        assert!(ring.pop().is_none());
    }

    #[test]
    fn test_frame_ring_clear() {
        let mut ring = FrameRing::new();
        ring.push(FrameInfo { return_address: 1, base: 10 });
        ring.push(FrameInfo { return_address: 2, base: 20 });
        ring.clear();
        assert!(ring.pop().is_none(), "clear 后 ring 必须为空");
    }

    #[test]
    fn test_frame_ring_head_advances_via_mask() {
        // spec 约束：调用方保证 depth < 64，ring 内部不做溢出检测。
        // 本测试验证在合法深度（<=63）下 head 用位掩码正确推进，且不回绕到 0。
        let mut ring = FrameRing::new();
        // push 63 次（深度 63，head 从 0 推进到 63，未回绕）
        for i in 0..63 {
            ring.push(FrameInfo { return_address: i, base: i * 2 });
        }
        // head == 63，pop 应返回最后 push 的 FrameInfo
        let info = ring.pop().expect("深度 63 时 pop 必须返回 Some");
        assert_eq!(info.return_address, 62);
        // 全部 pop 后回到空状态
        for _ in 0..62 {
            ring.pop().expect("剩余深度 > 0 时 pop 必须返回 Some");
        }
        assert!(ring.pop().is_none(), "全部 pop 后 ring 必须为空");
    }

    #[test]
    fn test_frame_data_fill_nil_within_bounds() {
        let mut fd = FrameData::with_capacity(8);
        fd.data.resize(8, nuzo_core::Value::from_smi(1));
        fd.fill_nil(2, 3);
        // [0,2) 未被填充
        assert!(!fd.data[0].is_nil());
        // [2,5) 被填充为 NIL
        assert!(fd.data[2].is_nil());
        assert!(fd.data[4].is_nil());
        // [5,8) 未被填充
        assert!(!fd.data[5].is_nil());
    }

    #[test]
    fn test_frame_data_fill_nil_clamps_to_len() {
        let mut fd = FrameData::with_capacity(8);
        fd.data.resize(4, nuzo_core::Value::from_smi(1));
        // 请求 [2, 2+10) 但 len=4，应裁剪到 [2,4)
        fd.fill_nil(2, 10);
        assert!(fd.data[2].is_nil());
        assert!(fd.data[3].is_nil());
    }

    #[test]
    fn test_frame_data_clear_resets_top_only() {
        let mut fd = FrameData::with_capacity(16);
        fd.top = 100;
        fd.clear();
        assert_eq!(fd.top, 0);
        assert_eq!(fd.data.capacity(), 16, "clear 不应释放 capacity");
    }

    #[test]
    fn test_frame_meta_default_is_normal() {
        let m = FrameMeta::default();
        assert_eq!(m.kind, FrameKind::Normal);
        assert!(m.closure.is_none());
        assert!(m.caller_chunk.is_none());
        assert_eq!(m.arena, usize::MAX);
    }

    #[test]
    fn test_overflow_stack_clear() {
        let mut os = OverflowStack::new();
        os.infos.push(FrameInfo { return_address: 1, base: 2 });
        os.metas.push(FrameMeta::default());
        os.clear();
        assert!(os.infos.is_empty());
        assert!(os.metas.is_empty());
    }

    /// Phase 5：验证 drain_to_overflow 将 ring 内容插入 overflow 头部，保持时间顺序。
    ///
    /// ring 中的帧比 overflow 中已有的更老，必须插入头部（而非尾部）。
    #[test]
    fn test_drain_to_overflow_prepends_ring_to_overflow() {
        let mut ring = FrameRing::new();
        // ring: push F0, F1, F2（最老在前：slots[0]=F0, slots[1]=F1, slots[2]=F2）
        ring.push(FrameInfo { return_address: 100, base: 0 });
        ring.push(FrameInfo { return_address: 101, base: 1 });
        ring.push(FrameInfo { return_address: 102, base: 2 });

        // overflow 预填 F3, F4（比 ring 中的更新）
        let mut overflow = OverflowStack::new();
        overflow.infos.push(FrameInfo { return_address: 103, base: 3 });
        overflow.infos.push(FrameInfo { return_address: 104, base: 4 });

        // drain_to_overflow：ring 内容插入 overflow 头部
        ring.drain_to_overflow(&mut overflow);

        // ring 应清空
        assert_eq!(ring.head, 0, "ring 应清空");
        assert!(ring.pop().is_none(), "ring 清空后 pop 返回 None");

        // overflow 应为 [F0, F1, F2, F3, F4]（最老在前，全局时间顺序）
        assert_eq!(overflow.infos.len(), 5);
        assert_eq!(overflow.infos[0].base, 0, "overflow[0] 应为 F0 (ring 中最老的)");
        assert_eq!(overflow.infos[1].base, 1, "overflow[1] 应为 F1");
        assert_eq!(overflow.infos[2].base, 2, "overflow[2] 应为 F2 (ring 中最新的)");
        assert_eq!(overflow.infos[3].base, 3, "overflow[3] 应为 F3 (overflow 原最老的)");
        assert_eq!(overflow.infos[4].base, 4, "overflow[4] 应为 F4");
    }

    /// Phase 5：验证 drain_to_overflow 在 ring 为空时是 no-op。
    #[test]
    fn test_drain_to_overflow_empty_ring() {
        let mut ring = FrameRing::new();
        let mut overflow = OverflowStack::new();
        overflow.infos.push(FrameInfo { return_address: 1, base: 10 });

        ring.drain_to_overflow(&mut overflow);

        assert_eq!(overflow.infos.len(), 1, "ring 为空时 overflow 不应变");
        assert_eq!(overflow.infos[0].base, 10, "原内容应保持不变");
    }
}

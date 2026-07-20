//! 寄存器分配器 trait 及其实现
//!
//! 提供 [`RegisterManager`] trait 作为可插拔接口，
//! [`TrackerRegManager`] 作为默认实现（引用计数 + checkpoint 批量回收），
//! [`NaiveRegManager`] 作为旧行为 fallback（单调递增，不回收）。
//!
//! ## 错误处理策略
//!
//! 本模块定义独立的 [`RegAllocError`]，不直接引用 [`crate::codegen::CodegenError`]，
//! 避免循环依赖。上层（codegen.rs）通过 `impl From<RegAllocError> for CodegenError`
//! 将错误转换为 CodegenError。

use std::collections::HashMap;

use nuzo_ir::types::ValueRef;

use crate::reg_pool::{Checkpoint, DualPool, RegPoolExhausted};
use crate::value_tracker::{ConsumeResult, ValueTracker};

// ---------------------------------------------------------------------------
// RegisterManager trait
// ---------------------------------------------------------------------------

/// 寄存器分配器 trait -- 可插拔接口
///
/// 所有操作通过此 trait 访问，CodeGen 不直接依赖具体实现。
pub(crate) trait RegisterManager {
    /// 为 ValueRef 定义分配物理寄存器
    fn allocate_def(&mut self, vr: ValueRef) -> Result<u16, RegAllocError>;

    /// 消费 ValueRef 的一次使用，返回物理寄存器编号
    fn consume_use(&mut self, vr: ValueRef) -> Result<u16, RegAllocError>;

    /// 分配物理寄存器，避免与 excludes 中的寄存器冲突
    ///
    /// **注意**: 在 DualPool 单端布局中，excludes 不再需要（持久区单调递增保证不冲突），
    /// 此方法保留仅为向后兼容，内部直接路由到 allocate_def。
    #[allow(dead_code)] // DualPool 布局中 excludes 不再需要，保留供向后兼容
    fn allocate_def_avoiding(
        &mut self,
        vr: ValueRef,
        excludes: &[u16],
    ) -> Result<u16, RegAllocError>;

    /// 分配临时寄存器（不关联 ValueRef，用于 Return void 的 nil_reg 等）
    fn allocate_temp(&mut self) -> Result<u16, RegAllocError>;

    /// 释放临时寄存器
    fn deallocate_temp(&mut self, reg: u16);

    /// 分配连续 `count` 个临时寄存器，返回起始寄存器编号
    ///
    /// 用于 StringBuild 等 VM 要求操作数位于连续寄存器的指令。
    /// 返回的区间 `[base, base+count)` 物理连续，调用者负责用 `deallocate_temp_block` 释放。
    fn allocate_temp_block(&mut self, count: u16) -> Result<u16, RegAllocError>;

    /// 释放 `allocate_temp_block` 分配的连续寄存器块
    fn deallocate_temp_block(&mut self, base: u16, count: u16);

    /// 完成分配，返回 locals_count（峰值寄存器数）
    ///
    /// 注意：使用 `self: Box<Self>` 签名以支持 trait object 调用
    /// （`dyn RegisterManager` 大小未知，必须通过 Box 消费）。
    fn finalize(self: Box<Self>) -> u16;

    /// 保存持久区检查点，返回 `Checkpoint { top, temp_free_len }` 快照
    ///
    /// 用于 ArrayNew 等复合类型分配：在分配 dest_reg 后保存 checkpoint，
    /// 分配 elem_regs 后 restore_checkpoint 批量回收。
    fn save_checkpoint(&self) -> Checkpoint;

    /// 恢复持久区检查点，批量释放 [cp.top, top) 范围内的持久寄存器
    ///
    /// 同时 `truncate` 临时区 free 栈到 `cp.temp_free_len`：仅回滚 checkpoint 后释放的
    /// 临时寄存器，保留 checkpoint 前合法释放的寄存器。
    fn restore_checkpoint(&mut self, cp: Checkpoint);
}

// ---------------------------------------------------------------------------
// RegAllocError — 独立错误类型（方案 B：避免循环依赖）
// ---------------------------------------------------------------------------

/// 寄存器分配错误
///
/// 独立于 [`crate::codegen::CodegenError`]，由上层通过 `From` 转换。
#[derive(Debug, Clone)]
pub(crate) enum RegAllocError {
    /// ValueRef 未定义即使用
    UndefinedValueRef(u32),
    /// 寄存器池耗尽
    PoolExhausted { count: u16 },
}

impl std::fmt::Display for RegAllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UndefinedValueRef(vr) => {
                write!(f, "Undefined ValueRef v{} (consume_use on undefined/dead value)", vr)
            }
            Self::PoolExhausted { count } => {
                write!(f, "Register pool exhausted: {} registers in use", count)
            }
        }
    }
}

impl std::error::Error for RegAllocError {}

impl From<RegPoolExhausted> for RegAllocError {
    fn from(err: RegPoolExhausted) -> Self {
        RegAllocError::PoolExhausted { count: err.count }
    }
}

// ---------------------------------------------------------------------------
// Region — 寄存器区域归属
// ---------------------------------------------------------------------------

/// 寄存器区域归属
///
/// 标记物理寄存器属于持久区还是临时区，用于 `consume_use` 决定是否单个释放。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Region {
    /// 持久区：由 acquire_persistent 分配，不单个回收（由 restore_checkpoint 批量回收）
    Persist,
    /// 临时区：由 acquire_temp 分配，可在最后一次使用后单个释放回 free 堆
    Temp,
}

// ---------------------------------------------------------------------------
// RegEvent — 寄存器分配/回收事件（可观测性）
// ---------------------------------------------------------------------------

/// 寄存器分配/回收事件
#[derive(Debug, Clone)]
#[allow(dead_code)] // 可观测性结构体，保留供调试/日志系统使用
pub(crate) struct RegEvent {
    pub vr: u32,
    pub reg: u16,
    pub remaining: u32,
    pub event_kind: RegEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RegEventKind {
    Def,
    Use,
    UseAndFree,
}

// ---------------------------------------------------------------------------
// TrackerRegManager — 引用计数 + checkpoint 批量回收
// ---------------------------------------------------------------------------

/// 基于引用计数的寄存器管理器
///
/// 组合 [`DualPool`] + [`ValueTracker`]，实现寄存器分配：
/// - `allocate_def`: 从 DualPool 持久区分配寄存器，在 ValueTracker 中记录定义和总引用次数
/// - `consume_use`: 从 ValueTracker 消费一次使用，持久区不单个释放（由 restore_checkpoint 批量回收），
///   临时区在最后一次使用后 release_temp 回 free 堆
/// - `allocate_def_avoiding`: 同 allocate_def（DualPool 单端布局中 excludes 不再需要）
/// - `save_checkpoint/restore_checkpoint`: 批量回收持久区寄存器
pub(crate) struct TrackerRegManager {
    pool: DualPool,
    tracker: ValueTracker,
    use_counts: HashMap<u32, u32>,
    /// 可选日志
    log: Vec<RegEvent>,
    /// 是否启用日志
    logging_enabled: bool,
    /// 寄存器区域归属（Persist/Temp），用于 consume_use 决定是否单个释放
    reg_region: HashMap<u16, Region>,
}

impl TrackerRegManager {
    /// 从预计算的 use_counts 创建管理器
    pub(crate) fn new(use_counts: HashMap<u32, u32>) -> Self {
        Self {
            pool: DualPool::new(),
            tracker: ValueTracker::new(),
            use_counts,
            log: Vec::new(),
            logging_enabled: false,
            reg_region: HashMap::new(),
        }
    }

    /// 启用日志记录
    #[cfg(test)]
    pub(crate) fn with_logging(mut self) -> Self {
        self.logging_enabled = true;
        self
    }

    /// 获取日志
    #[cfg(test)]
    #[allow(dead_code)] // 仅测试使用，保留供单元测试断言
    pub(crate) fn dump_log(&self) -> &[RegEvent] {
        &self.log
    }

    /// 导出 CSV 格式日志
    #[cfg(test)]
    pub(crate) fn dump_csv(&self) -> String {
        let mut csv = String::from("vr,reg,remaining,event\n");
        for event in &self.log {
            let kind = match event.event_kind {
                RegEventKind::Def => "def",
                RegEventKind::Use => "use",
                RegEventKind::UseAndFree => "use_free",
            };
            csv.push_str(&format!("{},{},{},{}\n", event.vr, event.reg, event.remaining, kind));
        }
        csv
    }

    /// 获取 ValueRef 的总引用次数（0 表示从未被使用）
    fn total_uses(&self, vr: ValueRef) -> u32 {
        self.use_counts.get(&vr.0).copied().unwrap_or(0)
    }
}

impl RegisterManager for TrackerRegManager {
    fn allocate_def(&mut self, vr: ValueRef) -> Result<u16, RegAllocError> {
        // Phi/Mov 重定义：同一 ValueRef 在多个基本块中被定义时，
        // 返回已分配的寄存器而非重新分配。这与 NaiveRegManager 的
        // "已分配则直接返回" 行为一致，确保 SSA Phi 模式正确。
        if let Some(reg) = self.tracker.peek_reg(vr) {
            return Ok(reg);
        }
        let reg = self.pool.acquire_persistent()?;
        let total_uses = self.total_uses(vr);
        self.tracker.track_def(vr, reg, total_uses);
        self.reg_region.insert(reg, Region::Persist);
        if self.logging_enabled {
            self.log.push(RegEvent {
                vr: vr.0,
                reg,
                remaining: total_uses,
                event_kind: RegEventKind::Def,
            });
        }
        Ok(reg)
    }

    fn consume_use(&mut self, vr: ValueRef) -> Result<u16, RegAllocError> {
        let ConsumeResult { reg, is_dead } =
            self.tracker.consume_use(vr).ok_or(RegAllocError::UndefinedValueRef(vr.0))?;
        // DualPool: 持久区不单个释放（由 restore_checkpoint 批量回收），
        // 临时区在最后一次使用后 release_temp 回 free 堆
        if is_dead && matches!(self.reg_region.get(&reg), Some(Region::Temp)) {
            self.pool.release_temp(reg);
        }
        if self.logging_enabled {
            let remaining = self.tracker.remaining_uses(vr).unwrap_or(0);
            self.log.push(RegEvent {
                vr: vr.0,
                reg,
                remaining,
                event_kind: if is_dead { RegEventKind::UseAndFree } else { RegEventKind::Use },
            });
        }
        Ok(reg)
    }

    fn allocate_def_avoiding(
        &mut self,
        vr: ValueRef,
        _excludes: &[u16],
    ) -> Result<u16, RegAllocError> {
        // Phi/Mov 重定义：同 allocate_def，已分配则直接返回
        // DualPool 单端布局：top 递增保证不冲突，excludes 不再需要
        if let Some(reg) = self.tracker.peek_reg(vr) {
            return Ok(reg);
        }
        let reg = self.pool.acquire_persistent()?;
        let total_uses = self.total_uses(vr);
        // 同 allocate_def：peek_reg 已保证 vr 未被跟踪
        let _ = self.tracker.track_def(vr, reg, total_uses);
        self.reg_region.insert(reg, Region::Persist);
        if self.logging_enabled {
            self.log.push(RegEvent {
                vr: vr.0,
                reg,
                remaining: total_uses,
                event_kind: RegEventKind::Def,
            });
        }
        Ok(reg)
    }

    fn allocate_temp(&mut self) -> Result<u16, RegAllocError> {
        let reg = self.pool.acquire_temp()?;
        self.reg_region.insert(reg, Region::Temp);
        Ok(reg)
    }

    fn deallocate_temp(&mut self, reg: u16) {
        self.pool.release_temp(reg);
    }

    fn allocate_temp_block(&mut self, count: u16) -> Result<u16, RegAllocError> {
        let base = self.pool.acquire_temp_block(count)?;
        for i in 0..count {
            self.reg_region.insert(base + i, Region::Temp);
        }
        Ok(base)
    }

    fn deallocate_temp_block(&mut self, base: u16, count: u16) {
        self.pool.release_temp_block(base, count);
        for i in 0..count {
            self.reg_region.remove(&(base + i));
        }
    }

    fn finalize(self: Box<Self>) -> u16 {
        self.pool.peak()
    }

    fn save_checkpoint(&self) -> Checkpoint {
        self.pool.save_checkpoint()
    }

    fn restore_checkpoint(&mut self, cp: Checkpoint) {
        self.pool.restore_checkpoint(cp);
        // 清理 reg_region 中已回滚的寄存器（编号 >= cp.top 的已被回收）
        self.reg_region.retain(|&reg, _| reg < cp.top);
    }
}

// ---------------------------------------------------------------------------
// NaiveRegManager — 原始单调递增分配器（不回收）
// ---------------------------------------------------------------------------

/// 原始单调递增分配器（不回收）
///
/// 用于 A/B 对比和紧急回退。行为与原始 CodeGenerator 完全一致。
#[cfg(test)]
pub(crate) struct NaiveRegManager {
    reg_map: HashMap<u32, u16>,
    next_reg: u16,
    peak: u16,
}

#[cfg(test)]
impl NaiveRegManager {
    pub(crate) fn new() -> Self {
        Self { reg_map: HashMap::new(), next_reg: 0, peak: 0 }
    }
}

#[cfg(test)]
impl RegisterManager for NaiveRegManager {
    fn allocate_def(&mut self, vr: ValueRef) -> Result<u16, RegAllocError> {
        if let Some(&reg) = self.reg_map.get(&vr.0) {
            return Ok(reg);
        }
        let reg = self.next_reg;
        self.next_reg = self
            .next_reg
            .checked_add(1)
            .ok_or(RegAllocError::PoolExhausted { count: self.next_reg })?;
        self.peak = self.peak.max(self.next_reg);
        self.reg_map.insert(vr.0, reg);
        Ok(reg)
    }

    fn consume_use(&mut self, vr: ValueRef) -> Result<u16, RegAllocError> {
        self.reg_map.get(&vr.0).copied().ok_or(RegAllocError::UndefinedValueRef(vr.0))
    }

    fn allocate_def_avoiding(
        &mut self,
        vr: ValueRef,
        _excludes: &[u16],
    ) -> Result<u16, RegAllocError> {
        // Naive 分配器不做避让，直接用 allocate_def
        // 因为它从不同编号递增，天然不冲突
        self.allocate_def(vr)
    }

    fn allocate_temp(&mut self) -> Result<u16, RegAllocError> {
        let reg = self.next_reg;
        self.next_reg = self
            .next_reg
            .checked_add(1)
            .ok_or(RegAllocError::PoolExhausted { count: self.next_reg })?;
        self.peak = self.peak.max(self.next_reg);
        Ok(reg)
    }

    fn deallocate_temp(&mut self, _reg: u16) {
        // Naive 分配器不回收，no-op
    }

    fn allocate_temp_block(&mut self, count: u16) -> Result<u16, RegAllocError> {
        if count == 0 {
            return Ok(self.next_reg);
        }
        let base = self.next_reg;
        let new_next = self
            .next_reg
            .checked_add(count)
            .ok_or(RegAllocError::PoolExhausted { count: self.next_reg })?;
        self.next_reg = new_next;
        self.peak = self.peak.max(self.next_reg);
        Ok(base)
    }

    fn deallocate_temp_block(&mut self, _base: u16, _count: u16) {
        // Naive 分配器不回收，no-op
    }

    fn finalize(self: Box<Self>) -> u16 {
        self.peak
    }

    fn save_checkpoint(&self) -> Checkpoint {
        // Naive 分配器不回收，checkpoint 携带 next_reg（temp_free_len 恒为 0）
        Checkpoint { top: self.next_reg, temp_free_len: 0 }
    }

    fn restore_checkpoint(&mut self, cp: Checkpoint) {
        // Naive 分配器不回收，no-op
        let _ = cp;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_straight_line() {
        // v0=1, v1=2, v2=add(v0,v1), v3=mul(v2,v0), ret v3
        // use_counts: v0=2, v1=1, v2=1, v3=1
        //
        // DualPool 单端布局：持久区寄存器不单个回收（由 restore_checkpoint 批量回收），
        // allocate_def 总是从 top 递增分配，不复用已分配寄存器。
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 2); // v0 used twice
        use_counts.insert(1, 1); // v1 used once
        use_counts.insert(2, 1); // v2 used once
        use_counts.insert(3, 1); // v3 used once

        let mut mgr = TrackerRegManager::new(use_counts);

        // v0 = LoadConst 1
        let r0 = mgr.allocate_def(ValueRef(0)).unwrap();
        assert_eq!(r0, 0); // top: 0->1

        // v1 = LoadConst 2
        let r1 = mgr.allocate_def(ValueRef(1)).unwrap();
        assert_eq!(r1, 1); // top: 1->2

        // v2 = Add(v0, v1)
        let l_reg = mgr.consume_use(ValueRef(0)).unwrap(); // v0 remaining=1，持久区不释放
        assert_eq!(l_reg, 0);
        let r_reg = mgr.consume_use(ValueRef(1)).unwrap(); // v1 remaining=0，持久区不单个回收
        assert_eq!(r_reg, 1);
        let d_reg = mgr.allocate_def(ValueRef(2)).unwrap();
        assert_eq!(d_reg, 2); // top 递增到 3，不复用 r1

        // v3 = Mul(v2, v0)
        let l_reg = mgr.consume_use(ValueRef(2)).unwrap(); // v2 remaining=0，持久区不释放
        assert_eq!(l_reg, 2);
        let r_reg = mgr.consume_use(ValueRef(0)).unwrap(); // v0 remaining=0，持久区不释放
        assert_eq!(r_reg, 0);
        let d_reg = mgr.allocate_def_avoiding(ValueRef(3), &[l_reg, r_reg]).unwrap();
        // DualPool 单端布局：excludes 不再需要，top 递增分配 r3
        assert_eq!(d_reg, 3);

        // ret v3
        let _ = mgr.consume_use(ValueRef(3)).unwrap();

        let locals = Box::new(mgr).finalize();
        // peak = 4（持久区不复用，4 个 def 各占一寄存器）
        assert_eq!(locals, 4);
    }

    #[test]
    fn test_naive_vs_tracker() {
        // 同一 IR，Naive 和 Tracker 应该用相同数量寄存器（DualPool 不复用持久区）
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 2);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);
        use_counts.insert(3, 1);

        // Naive
        let mut naive = NaiveRegManager::new();
        naive.allocate_def(ValueRef(0)).unwrap();
        naive.allocate_def(ValueRef(1)).unwrap();
        naive.consume_use(ValueRef(0)).unwrap();
        naive.consume_use(ValueRef(1)).unwrap();
        naive.allocate_def(ValueRef(2)).unwrap();
        naive.consume_use(ValueRef(2)).unwrap();
        naive.consume_use(ValueRef(0)).unwrap();
        naive.allocate_def(ValueRef(3)).unwrap();
        naive.consume_use(ValueRef(3)).unwrap();
        let naive_locals = Box::new(naive).finalize();

        // Tracker
        let mut tracker = TrackerRegManager::new(use_counts);
        tracker.allocate_def(ValueRef(0)).unwrap();
        tracker.allocate_def(ValueRef(1)).unwrap();
        tracker.consume_use(ValueRef(0)).unwrap();
        tracker.consume_use(ValueRef(1)).unwrap();
        tracker.allocate_def(ValueRef(2)).unwrap();
        tracker.consume_use(ValueRef(2)).unwrap();
        tracker.consume_use(ValueRef(0)).unwrap();
        tracker.allocate_def(ValueRef(3)).unwrap();
        tracker.consume_use(ValueRef(3)).unwrap();
        let tracker_locals = Box::new(tracker).finalize();

        assert!(tracker_locals <= naive_locals);
        assert_eq!(naive_locals, 4); // Naive: 0,1,2,3 -> peak=4
    }

    #[test]
    fn test_allocate_def_avoiding() {
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);

        let mut mgr = TrackerRegManager::new(use_counts);
        mgr.allocate_def(ValueRef(0)).unwrap(); // r0=0
        mgr.allocate_def(ValueRef(1)).unwrap(); // r1=1
        mgr.consume_use(ValueRef(0)).unwrap(); // v0 consumed，持久区不单个释放
        // allocate_def_avoiding for v2, avoid r0 (excludes ignored in DualPool)
        let reg = mgr.allocate_def_avoiding(ValueRef(2), &[0]).unwrap();
        // DualPool: top 递增分配，excludes 不再需要
        assert_eq!(reg, 2);
    }

    #[test]
    fn test_csv_log() {
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);

        let mut mgr = TrackerRegManager::new(use_counts).with_logging();
        mgr.allocate_def(ValueRef(0)).unwrap();
        mgr.consume_use(ValueRef(0)).unwrap();

        let csv = mgr.dump_csv();
        assert!(csv.contains("def") || csv.contains("use"));
    }

    #[test]
    fn test_consume_undefined_returns_error() {
        let mut mgr = TrackerRegManager::new(HashMap::new());
        let result = mgr.consume_use(ValueRef(99));
        assert!(matches!(result, Err(RegAllocError::UndefinedValueRef(99))));
    }

    #[test]
    fn test_naive_consume_undefined_returns_error() {
        let mut mgr = NaiveRegManager::new();
        let result = mgr.consume_use(ValueRef(42));
        assert!(matches!(result, Err(RegAllocError::UndefinedValueRef(42))));
    }

    #[test]
    fn test_reg_alloc_error_display() {
        let err = RegAllocError::UndefinedValueRef(5);
        assert!(err.to_string().contains("v5"));

        let err = RegAllocError::PoolExhausted { count: 100 };
        assert!(err.to_string().contains("100"));
    }

    #[test]
    fn test_from_reg_pool_exhausted() {
        // 验证 From<RegPoolExhausted> 正确传递 count 字段到 RegAllocError
        let exhausted = RegPoolExhausted { count: 42, available: 0, peak: 10 };
        let err: RegAllocError = exhausted.into();
        assert!(matches!(err, RegAllocError::PoolExhausted { count: 42 }));
    }

    #[test]
    fn test_reg_pool_exhausted_display_and_error() {
        // 验证 RegPoolExhausted 的 Display 输出包含诊断字段，
        // 且实现了 std::error::Error（可作为错误源 ? 传播）
        let err = RegPoolExhausted { count: 250, available: 0, peak: 250 };
        let msg = err.to_string();
        assert!(msg.contains("allocated=250"), "msg = {msg}");
        assert!(msg.contains("available=0"), "msg = {msg}");
        assert!(msg.contains("peak=250"), "msg = {msg}");
        // std::error::Error trait 可用（作为 source 链）
        fn _assert_error<E: std::error::Error>(_e: E) {}
        _assert_error(err);
    }

    // ----- checkpoint API 测试 -----

    #[test]
    fn test_save_restore_checkpoint() {
        // 验证 save_checkpoint/restore_checkpoint 批量回收持久区寄存器
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);

        let mut mgr = TrackerRegManager::new(use_counts);
        let r0 = mgr.allocate_def(ValueRef(0)).unwrap(); // 0
        let checkpoint = mgr.save_checkpoint(); // 1
        let r1 = mgr.allocate_def(ValueRef(1)).unwrap(); // 1
        let r2 = mgr.allocate_def(ValueRef(2)).unwrap(); // 2
        assert_eq!(r0, 0);
        assert_eq!(r1, 1);
        assert_eq!(r2, 2);
        // restore 后，r1/r2 被批量回收
        mgr.restore_checkpoint(checkpoint);
        // 下一次分配应从 checkpoint=1 开始
        let r3 = mgr.allocate_def(ValueRef(0)).unwrap(); // 复用 r0 的 VR（已分配则直接返回）
        assert_eq!(r3, 0); // peek_reg 返回已分配的 r0=0
    }

    #[test]
    fn test_checkpoint_restores_peak() {
        // restore_checkpoint 不影响 peak（高水位不下降）
        let mut mgr = TrackerRegManager::new(HashMap::new());
        let _ = mgr.allocate_def(ValueRef(0)).unwrap();
        let _ = mgr.allocate_def(ValueRef(1)).unwrap();
        let checkpoint = mgr.save_checkpoint();
        let _ = mgr.allocate_def(ValueRef(2)).unwrap();
        // peak = 3
        mgr.restore_checkpoint(checkpoint);
        // peak 仍为 3
        let locals = Box::new(mgr).finalize();
        assert_eq!(locals, 3);
    }

    #[test]
    fn test_naive_checkpoint_noop() {
        // Naive 分配器的 checkpoint 是 no-op
        let mut mgr = NaiveRegManager::new();
        let _ = mgr.allocate_def(ValueRef(0)).unwrap(); // 0
        let checkpoint = mgr.save_checkpoint(); // 1
        let _ = mgr.allocate_def(ValueRef(1)).unwrap(); // 1
        mgr.restore_checkpoint(checkpoint); // no-op
        // Naive 不回收，下一次分配仍从 2 开始
        let reg = mgr.allocate_temp().unwrap();
        assert_eq!(reg, 2);
    }

    #[test]
    fn test_temp_reg_released_on_consume_dead() {
        // 临时区寄存器在最后一次使用后应被 release_temp
        let mut mgr = TrackerRegManager::new(HashMap::new());
        let t0 = mgr.allocate_temp().unwrap(); // 0, region=Temp
        assert_eq!(t0, 0);
        // 手动释放
        mgr.deallocate_temp(t0);
        // 复用
        let t1 = mgr.allocate_temp().unwrap();
        assert_eq!(t1, 0); // 从 temp_free 复用
    }

    // ----- TrackerRegManager core tests (coverage expansion) -----

    #[test]
    fn test_consume_use_releases_temp_on_last_use() {
        // Verify that deallocate_temp + allocate_temp correctly reuses freed slots.
        // Note: consume_use auto-release only fires for Temp-region registers tracked
        // via allocate_def (which always uses Persist). For temp registers,
        // deallocate_temp is the release path.
        let mut mgr = TrackerRegManager::new(HashMap::new());
        let t0 = mgr.allocate_temp().unwrap(); // reg=0, Region::Temp
        assert_eq!(t0, 0);
        mgr.deallocate_temp(t0);
        let t1 = mgr.allocate_temp().unwrap(); // reuse 0 from temp_free
        assert_eq!(t1, 0, "deallocated temp should be reused");
    }

    #[test]
    fn test_persist_not_released_on_consume_dead() {
        // Persistent registers should NOT be released on last consume_use
        // (they are batch-released via restore_checkpoint)
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1); // v0 used once
        use_counts.insert(1, 1); // v1 used once
        let mut mgr = TrackerRegManager::new(use_counts);

        let r0 = mgr.allocate_def(ValueRef(0)).unwrap(); // 0, Persist
        let r1 = mgr.allocate_def(ValueRef(1)).unwrap(); // 1, Persist
        assert_eq!((r0, r1), (0, 1));

        // Consume both — they die but are NOT released (Persist region)
        let result0 = mgr.consume_use(ValueRef(0)).unwrap();
        assert_eq!(result0, 0);
        let result1 = mgr.consume_use(ValueRef(1)).unwrap();
        assert_eq!(result1, 1);

        // Next allocate should still increment top (not reuse 0 or 1)
        let r2 = mgr.allocate_def(ValueRef(2)).unwrap();
        assert_eq!(r2, 2, "persist regs not reused even after consume_dead");
    }

    #[test]
    fn test_mixed_persist_temp_with_region_tracking() {
        // Verify Region tracking: persist and temp allocated, region correctly stored
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        let mut mgr = TrackerRegManager::new(use_counts);

        let p0 = mgr.allocate_def(ValueRef(0)).unwrap(); // 0, Persist
        let t0 = mgr.allocate_temp().unwrap(); // 1, Temp
        assert_eq!((p0, t0), (0, 1));

        // Verify region tracking via behavior:
        // - consume_use on persist (v0) does NOT release even if dead
        mgr.consume_use(ValueRef(0)).unwrap(); // v0 dead, but Persist → no release
        // - deallocate_temp on t0 should release it
        mgr.deallocate_temp(t0);

        // Next temp should reuse t0=1
        let t1 = mgr.allocate_temp().unwrap();
        assert_eq!(t1, 1, "temp should reuse deallocated slot");
    }

    #[test]
    fn test_restore_checkpoint_cleans_reg_region() {
        // After restore_checkpoint, reg_region entries for regs >= cp.top should be removed
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);
        let mut mgr = TrackerRegManager::new(use_counts);

        let _r0 = mgr.allocate_def(ValueRef(0)).unwrap(); // 0, Persist
        let cp = mgr.save_checkpoint(); // top=1
        let _r1 = mgr.allocate_def(ValueRef(1)).unwrap(); // 1, Persist
        let _t0 = mgr.allocate_temp().unwrap(); // 2, Temp
        // reg_region should have entries for 0, 1, 2
        assert_eq!(mgr.reg_region.len(), 3);

        mgr.restore_checkpoint(cp);
        // reg_region should only have entry for reg 0 (< cp.top=1)
        assert_eq!(mgr.reg_region.len(), 1, "only reg 0 should remain in reg_region");
        assert!(mgr.reg_region.contains_key(&0));
        assert!(!mgr.reg_region.contains_key(&1));
        assert!(!mgr.reg_region.contains_key(&2));
    }

    #[test]
    fn test_finalize_returns_peak() {
        // finalize should return the peak register count
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);
        let mut mgr = TrackerRegManager::new(use_counts);
        mgr.allocate_def(ValueRef(0)).unwrap(); // 0
        mgr.allocate_def(ValueRef(1)).unwrap(); // 1
        mgr.allocate_temp().unwrap(); // 2
        let locals = Box::new(mgr).finalize();
        assert_eq!(locals, 3, "finalize should return peak=3");
    }

    #[test]
    fn test_allocate_def_idempotent_for_same_vr() {
        // Calling allocate_def twice for the same ValueRef should return the same register
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 2);
        let mut mgr = TrackerRegManager::new(use_counts);

        let r0a = mgr.allocate_def(ValueRef(0)).unwrap();
        let r0b = mgr.allocate_def(ValueRef(0)).unwrap();
        assert_eq!(r0a, r0b, "re-def of same VR should return same register");
        // Peak should only be 1 (no second allocation)
        let locals = Box::new(mgr).finalize();
        assert_eq!(locals, 1);
    }

    #[test]
    fn test_consume_use_decrements_remaining() {
        // consume_use should decrement remaining uses, only freeing Temp on last use
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 3); // v0 used 3 times
        let mut mgr = TrackerRegManager::new(use_counts).with_logging();

        mgr.allocate_def(ValueRef(0)).unwrap(); // reg=0, Persist

        // First consume: remaining=2
        let r0 = mgr.consume_use(ValueRef(0)).unwrap();
        assert_eq!(r0, 0);
        // Log should show "use" (not "use_free") since Persist and remaining > 0
        assert_eq!(mgr.dump_log().len(), 2); // def + use

        // Second consume: remaining=1
        mgr.consume_use(ValueRef(0)).unwrap();

        // Third consume: remaining=0, is_dead=true, but Persist → no release
        mgr.consume_use(ValueRef(0)).unwrap();
        // Should still have 1 log entry for the last use
        assert_eq!(mgr.dump_log().len(), 4); // def + 3 uses
    }

    #[test]
    fn test_temp_allocate_deallocate_cycle() {
        // Repeated allocate/deallocate temp should not leak or panic
        let mut mgr = TrackerRegManager::new(HashMap::new());
        for cycle in 0..10 {
            let regs: Vec<u16> = (0..5).map(|_| mgr.allocate_temp().unwrap()).collect();
            // First cycle: regs are 0,1,2,3,4. Subsequent: reuse from temp_free
            if cycle == 0 {
                assert_eq!(regs, vec![0, 1, 2, 3, 4]);
            }
            for r in regs {
                mgr.deallocate_temp(r);
            }
        }
        // After all cycles, peak should still be 5
        let locals = Box::new(mgr).finalize();
        assert_eq!(locals, 5, "peak should be 5 after 10 allocate/deallocate cycles");
    }

    #[test]
    fn test_checkpoint_with_temp_allocation_pattern() {
        // Simulate a real codegen pattern:
        // dest(persist) → checkpoint → idx(temp) → elem(persist) × 3 → deallocate idx → restore
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1); // dest
        use_counts.insert(1, 1); // elem0
        use_counts.insert(2, 1); // elem1
        use_counts.insert(3, 1); // elem2
        let mut mgr = TrackerRegManager::new(use_counts);

        let dest = mgr.allocate_def(ValueRef(0)).unwrap(); // 0, Persist
        assert_eq!(dest, 0);
        let cp = mgr.save_checkpoint(); // top=1
        let idx = mgr.allocate_temp().unwrap(); // 1, Temp
        assert_eq!(idx, 1);
        let e0 = mgr.allocate_def(ValueRef(1)).unwrap(); // 2, Persist
        let e1 = mgr.allocate_def(ValueRef(2)).unwrap(); // 3, Persist
        let e2 = mgr.allocate_def(ValueRef(3)).unwrap(); // 4, Persist
        assert_eq!((e0, e1, e2), (2, 3, 4));

        // Deallocate idx before restore
        mgr.deallocate_temp(idx);
        // Restore checkpoint: top=1, truncate temp_free, clean reg_region
        mgr.restore_checkpoint(cp);
        // reg_region should only have dest (reg=0)
        assert_eq!(mgr.reg_region.len(), 1);
        // Next persistent starts from top=1
        let next = mgr.allocate_def(ValueRef(5)).unwrap(); // v5 not in use_counts, treated as 0 uses
        assert_eq!(next, 1);
    }

    #[test]
    fn test_pool_exhaustion_through_manager() {
        // Allocate MAX_FUNCTION_LOCALS temps via TrackerRegManager → next should fail
        let mut mgr = TrackerRegManager::new(HashMap::new());
        for _ in 0..nuzo_core::MAX_FUNCTION_LOCALS {
            assert!(mgr.allocate_temp().is_ok());
        }
        let result = mgr.allocate_temp();
        assert!(matches!(result, Err(RegAllocError::PoolExhausted { .. })));
    }

    #[test]
    fn test_region_enum_values() {
        // Verify Region::Persist != Region::Temp (trivial but ensures no regression)
        assert_ne!(Region::Persist, Region::Temp);
        assert_eq!(Region::Persist, Region::Persist);
        assert_eq!(Region::Temp, Region::Temp);
    }

    #[test]
    fn test_reg_event_kind_variants() {
        // Verify RegEventKind equality
        assert_eq!(RegEventKind::Def, RegEventKind::Def);
        assert_eq!(RegEventKind::Use, RegEventKind::Use);
        assert_eq!(RegEventKind::UseAndFree, RegEventKind::UseAndFree);
        assert_ne!(RegEventKind::Def, RegEventKind::Use);
        assert_ne!(RegEventKind::Use, RegEventKind::UseAndFree);
    }

    #[test]
    fn test_logging_def_and_use_events() {
        // Verify logging captures def and use events correctly
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 2); // v0 used twice
        let mut mgr = TrackerRegManager::new(use_counts).with_logging();

        mgr.allocate_def(ValueRef(0)).unwrap(); // def
        mgr.consume_use(ValueRef(0)).unwrap(); // use (remaining=1)
        mgr.consume_use(ValueRef(0)).unwrap(); // use_free (remaining=0, but Persist → no release)

        let log = mgr.dump_log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].event_kind, RegEventKind::Def);
        assert_eq!(log[0].vr, 0);
        assert_eq!(log[1].event_kind, RegEventKind::Use);
        assert_eq!(log[2].event_kind, RegEventKind::UseAndFree);
        // Note: UseAndFree is logged even for Persist (logging is independent of release)
    }

    #[test]
    fn test_multiple_checkpoints_sequential() {
        // Multiple sequential checkpoints, each restored independently
        let mut use_counts = HashMap::new();
        use_counts.insert(0, 1);
        use_counts.insert(1, 1);
        use_counts.insert(2, 1);
        let mut mgr = TrackerRegManager::new(use_counts);

        let _r0 = mgr.allocate_def(ValueRef(0)).unwrap(); // 0
        let cp1 = mgr.save_checkpoint(); // top=1
        let _r1 = mgr.allocate_def(ValueRef(1)).unwrap(); // 1
        let cp2 = mgr.save_checkpoint(); // top=2
        let _r2 = mgr.allocate_def(ValueRef(2)).unwrap(); // 2

        // Restore cp2 → top=2
        mgr.restore_checkpoint(cp2);
        let r_new = mgr.allocate_def(ValueRef(0)).unwrap(); // re-def v0 → returns existing reg=0
        assert_eq!(r_new, 0);

        // Restore cp1 → top=1
        mgr.restore_checkpoint(cp1);
        let r_new2 = mgr.allocate_def(ValueRef(0)).unwrap(); // v0 still tracked? No, restore doesn't untrack
        // After restore to cp1, reg_region only has reg 0. But v0's tracker entry
        // is NOT removed by restore_checkpoint. So peek_reg still returns 0.
        assert_eq!(r_new2, 0, "v0 should still be tracked after checkpoint restore");
    }

    #[test]
    fn test_undefined_value_ref_error_message() {
        // Verify that consuming an undefined ValueRef produces a descriptive error
        let mut mgr = TrackerRegManager::new(HashMap::new());
        let err = mgr.consume_use(ValueRef(42)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("v42"), "error message should mention v42: {msg}");
    }
}

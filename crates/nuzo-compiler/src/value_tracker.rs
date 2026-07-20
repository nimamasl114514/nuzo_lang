//! ValueRef 状态跟踪器 — 跟踪每个虚拟寄存器的生命周期状态
//!
//! 核心状态机：Dead ↔ Alive
//! - track_def: Dead → Alive{reg, remaining_uses}
//! - consume_use: Alive → (递减 remaining_uses) → 若=0 则 Dead + 返回 is_dead=true

use std::collections::HashMap;

use nuzo_ir::types::ValueRef;

/// ValueRef 的生命周期状态
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // 枚举变体由 consume_use 间接使用，保留供状态机完整性
enum SlotState {
    /// 未定义或已死亡，不占用物理寄存器
    Dead,
    /// 活跃：拥有物理寄存器，remaining_uses 为剩余引用次数
    Alive { reg: u16, remaining_uses: u32 },
}

/// consume_use 的返回结果
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConsumeResult {
    /// 物理寄存器编号
    pub reg: u16,
    /// 该值是否已死亡（remaining_uses 递减到 0）
    pub is_dead: bool,
}

/// ValueRef 状态跟踪器
///
/// 跟踪每个虚拟寄存器（ValueRef）的生命周期状态。
/// 配合 UsageCounter 使用：track_def 时传入 total_uses，
/// 每次 consume_use 递减，归零时标记为 Dead。
pub(crate) struct ValueTracker {
    slots: HashMap<u32, SlotState>,
}

impl ValueTracker {
    pub(crate) fn new() -> Self {
        Self { slots: HashMap::new() }
    }

    /// 记录 ValueRef 的定义，分配物理寄存器和总引用次数
    ///
    /// 返回旧寄存器（若之前处于 Alive 状态），供调用方释放回寄存器池。
    ///
    /// # SSA-like 语义说明
    /// IR 允许同一 ValueRef 在互斥的基本块（如 if/else 分支）中定义，
    /// 类似 PHI 节点。因此 `Alive` 是合法的前置状态，表示在另一分支中
    /// 已定义。调用方应释放返回的旧寄存器以避免泄漏。
    ///
    /// # P2.9 文档说明（当前调用模式）
    ///
    /// 审核报告指出"`Some(SlotState::Alive {...}) => Some(old_reg)` 分支
    /// 实际不可达（死代码）"。经验证，当前所有调用方（reg_manager.rs 中的
    /// `allocate_def`）在调用 `track_def` 前都
    /// 通过 `peek_reg` 检查了 vr 是否已存在：
    ///
    /// ```text
    /// if let Some(reg) = self.tracker.peek_reg(vr) {
    ///     return Ok(reg);  // 已分配，直接返回，不调 track_def
    /// }
    /// let reg = self.pool.acquire_persistent()?;
    /// self.tracker.track_def(vr, reg, total_uses);  // 此时 vr 必然未跟踪
    /// ```
    ///
    /// 因此 `track_def` 内部的 `Some(SlotState::Alive {...}) => Some(old_reg)`
    /// 分支在当前调用模式下确实不可达。**保留此分支的原因**：
    /// 1. 防御性编程：若未来调用方忘记 peek_reg 检查，此分支能正确处理
    /// 2. API 完整性：track_def 作为公共 API，应正确处理所有合法前置状态
    /// 3. 测试覆盖：单元测试 `test_track_def_overrides_existing` 验证此分支
    ///
    /// 不删除此分支，避免未来调用模式变化时引入寄存器泄漏 bug。
    pub(crate) fn track_def(&mut self, vr: ValueRef, reg: u16, total_uses: u32) -> Option<u16> {
        let old = self.slots.insert(vr.0, SlotState::Alive { reg, remaining_uses: total_uses });
        match old {
            // 互斥分支重定义：返回旧寄存器供上层释放
            Some(SlotState::Alive { reg: old_reg, .. }) => Some(old_reg),
            None | Some(SlotState::Dead) => None,
        }
    }

    /// 消费 ValueRef 的一次使用
    ///
    /// 返回物理寄存器编号和是否已死亡的标志。
    /// 若 ValueRef 未定义或已死亡，返回 None。
    pub(crate) fn consume_use(&mut self, vr: ValueRef) -> Option<ConsumeResult> {
        match self.slots.get_mut(&vr.0) {
            Some(SlotState::Alive { remaining_uses, reg }) => {
                *remaining_uses -= 1;
                let is_dead = *remaining_uses == 0;
                let reg = *reg;
                if is_dead {
                    self.slots.remove(&vr.0);
                }
                Some(ConsumeResult { reg, is_dead })
            }
            _ => None,
        }
    }

    /// 查询 ValueRef 是否处于 Alive 状态
    #[allow(dead_code)] // 调试查询 API，保留供寄存器分配诊断使用
    pub(crate) fn is_alive(&self, vr: ValueRef) -> bool {
        matches!(self.slots.get(&vr.0), Some(SlotState::Alive { .. }))
    }

    /// 查询 ValueRef 当前分配的物理寄存器（不消费，不修改状态）
    ///
    /// 用于 Phi/Mov 重定义场景：同一 ValueRef 在多个基本块中被定义，
    /// 第二次 `allocate_def` 应返回已分配的寄存器而非重新分配。
    pub(crate) fn peek_reg(&self, vr: ValueRef) -> Option<u16> {
        match self.slots.get(&vr.0) {
            Some(SlotState::Alive { reg, .. }) => Some(*reg),
            _ => None,
        }
    }

    /// 查询 ValueRef 当前剩余引用次数
    #[allow(dead_code)] // 调试查询 API，保留供寄存器分配诊断使用
    pub(crate) fn remaining_uses(&self, vr: ValueRef) -> Option<u32> {
        match self.slots.get(&vr.0) {
            Some(SlotState::Alive { remaining_uses, .. }) => Some(*remaining_uses),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_def_then_use() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(0);
        let _old = tracker.track_def(vr, 0, 2);
        assert!(_old.is_none());
        assert!(tracker.is_alive(vr));

        let result = tracker.consume_use(vr).unwrap();
        assert_eq!(result.reg, 0);
        assert!(!result.is_dead);
        assert_eq!(tracker.remaining_uses(vr), Some(1));

        let result = tracker.consume_use(vr).unwrap();
        assert!(result.is_dead);
        assert!(!tracker.is_alive(vr));
    }

    #[test]
    fn test_single_use_immediate_free() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(1);
        let _old = tracker.track_def(vr, 5, 1);
        assert!(_old.is_none());

        let result = tracker.consume_use(vr).unwrap();
        assert_eq!(result.reg, 5);
        assert!(result.is_dead);
        assert!(!tracker.is_alive(vr));
    }

    #[test]
    fn test_consume_undef_returns_none() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(99);
        assert!(tracker.consume_use(vr).is_none());
    }

    #[test]
    fn test_consume_after_death_returns_none() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(0);
        let _old = tracker.track_def(vr, 0, 1);
        assert!(_old.is_none());
        let _ = tracker.consume_use(vr);
        assert!(tracker.consume_use(vr).is_none());
    }

    #[test]
    fn test_zero_uses_immediate_death() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(0);
        // 0 uses = 定义后即死亡（不会被任何指令引用）
        let _old = tracker.track_def(vr, 3, 0);
        assert!(_old.is_none());
        // track_def 不自动处理这种情况，需要外部保证
        // 暂时让外部逻辑处理：0 uses 的 vreg 不会调用 consume_use
        assert!(tracker.is_alive(vr)); // Alive { remaining_uses: 0 }
        // consume_use(0-1=underflow) 不会发生，因为外部不会对 0-uses 的 vreg 调用 consume_use
    }

    /// P2.9 回归测试：验证 track_def 在 Alive 状态下重定义时返回旧寄存器
    ///
    /// 此场景对应 SSA-like 互斥分支重定义：同一 ValueRef 在 if/else 两个分支中
    /// 都被定义。当前生产代码通过 peek_reg 检查避免此场景，但 track_def API
    /// 应正确处理以保证防御性编程。
    #[test]
    fn test_track_def_overrides_existing() {
        let mut tracker = ValueTracker::new();
        let vr = ValueRef(0);

        // 第一次定义：vr → reg 5, 2 uses
        let old = tracker.track_def(vr, 5, 2);
        assert!(old.is_none(), "first track_def should return None");
        assert_eq!(tracker.peek_reg(vr), Some(5));

        // 第二次定义（模拟 if/else 互斥分支重定义）：vr → reg 9, 1 use
        // 应返回旧寄存器 5 供调用方释放
        let old = tracker.track_def(vr, 9, 1);
        assert_eq!(old, Some(5), "track_def on Alive state should return old reg");

        // 验证 vr 现在指向新寄存器 9
        assert_eq!(tracker.peek_reg(vr), Some(9));

        // consume_use 应基于新定义（1 use）
        let result = tracker.consume_use(vr).unwrap();
        assert_eq!(result.reg, 9);
        assert!(result.is_dead, "after single use of new def, vr should be dead");
    }
}

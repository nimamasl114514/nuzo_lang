//! # 自适应槽位批处理（Adaptive Slot Batching, ASB）
//!
//! 本模块为 Signal 提供槽位热度统计与批量发射缓冲原语，
//! 用于在多槽场景下降低 per-slot 调度开销并指导路径选择。
//!
//! ## 设计要点
//!
//! ### 1. 槽位热度分级（SlotTier）
//! 通过指数衰减分数将槽位划分为 Hot / Warm / Cold 三级，
//! 为未来"热槽优先处理"等优化提供数据基础。
//!
//! ### 2. 指数衰减统计（SlotStats）
//! 采用经典 EWMA（指数加权移动平均）更新策略：
//! ```text
//! decayed_score = old_score * 0.95 + new_call * 0.05
//! ```
//! - 衰减系数 0.95：约 14 次调用后历史权重降至 50%
//! - 增量 0.05：保证单次调用对分数的影响可控
//!
//! ### 3. 批量发射缓冲（EmitBatch）
//! 用于 >64 槽路径的内部批量执行：在单次 emit 内部收集待执行项，
//! 达到 BATCH_MAX_SIZE 或 BATCH_THRESHOLD 时统一 flush。
//!
//! ## 性能特征
//!
//! | 操作 | 时间复杂度 | 备注 |
//! |------|-----------|------|
//! | SlotStats::new | O(1) | 栈分配 |
//! | record_call | O(1) | 两次浮点乘加 + 一次分支 |
//! | decay | O(1) | 单次浮点乘法 |
//! | EmitBatch::push | O(1) amortized | VecDeque 末尾插入 |
//! | EmitBatch::drain | O(n) | 一次性取出全部待执行项 |

use std::collections::VecDeque;
use web_time::{Duration, Instant};

/// 槽位热度分级
///
/// 基于 [`SlotStats::decayed_score`] 阈值划分，用于未来热槽优先调度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SlotTier {
    /// 高频调用（decayed_score > 0.5）
    Hot,
    /// 中频调用（0.1 < decayed_score ≤ 0.5）
    #[default]
    Warm,
    /// 低频调用（decayed_score ≤ 0.1）
    Cold,
}

/// 单个槽位的衰减统计
///
/// # 字段语义
/// - `decayed_score`: 指数衰减分数，反映"近期调用密度"
/// - `tier`: 当前热度分级，由 `decayed_score` 派生
/// - `call_count`: 窗口期累计调用次数（仅单调递增，衰减时不重置）
///
/// # 线程安全
/// 本结构不是 `Sync` 的，需通过外部锁（如 `RwLock`）保护并发访问。
#[derive(Debug, Clone)]
pub struct SlotStats {
    /// 指数衰减分数（EWMA），范围 [0.0, 1.0]
    pub decayed_score: f64,
    /// 当前热度分级
    pub tier: SlotTier,
    /// 窗口期累计调用次数
    pub call_count: u64,
}

/// 衰减系数：每次调用后历史分数保留比例
const DECAY_FACTOR: f64 = 0.95;
/// 单次调用增量：新调用对分数的贡献
const CALL_INCREMENT: f64 = 0.05;
/// Hot 分级阈值
const HOT_THRESHOLD: f64 = 0.5;
/// Warm 分级阈值（高于此值且不达 Hot 即为 Warm）
const WARM_THRESHOLD: f64 = 0.1;

impl SlotStats {
    /// 创建一个 Cold 初始状态的统计条目
    pub fn new() -> Self {
        Self { decayed_score: 0.0, tier: SlotTier::Cold, call_count: 0 }
    }

    /// 记录一次调用，更新衰减分数与分级
    ///
    /// # 算法
    /// `decayed_score = decayed_score * 0.95 + 0.05`
    ///
    /// 长期未调用时分数会逐渐趋近 0（Cold），
    /// 高频调用时分数会逐渐趋近 1.0（Hot）。
    pub fn record_call(&mut self) {
        self.decayed_score = self.decayed_score * DECAY_FACTOR + CALL_INCREMENT;
        self.call_count += 1;
        self.tier = if self.decayed_score > HOT_THRESHOLD {
            SlotTier::Hot
        } else if self.decayed_score > WARM_THRESHOLD {
            SlotTier::Warm
        } else {
            SlotTier::Cold
        };
    }

    /// 执行一次衰减（不记录调用）
    ///
    /// 用于定期全局衰减：每 N 次 emit 触发一次，
    /// 让长期未调用的槽位分数逐渐降低。
    pub fn decay(&mut self) {
        self.decayed_score *= DECAY_FACTOR;
        // 同步刷新 tier，避免分级与分数脱节
        self.tier = if self.decayed_score > HOT_THRESHOLD {
            SlotTier::Hot
        } else if self.decayed_score > WARM_THRESHOLD {
            SlotTier::Warm
        } else {
            SlotTier::Cold
        };
    }
}

impl Default for SlotStats {
    fn default() -> Self {
        Self::new()
    }
}

/// 触发批量 flush 的时间阈值
///
/// 当距上次 flush 超过此阈值时，即使 batch 未满也会触发执行。
/// 设为 50µs 以平衡延迟与吞吐。
pub const BATCH_THRESHOLD: Duration = Duration::from_micros(50);

/// 单批次最大容量
///
/// 超过此容量时立即 flush，避免内存占用过高。
/// 128 是 L1 缓存友好的上限（每项 ~32B 时约占 4KB）。
pub const BATCH_MAX_SIZE: usize = 128;

/// 批量发射缓冲
///
/// # 设计意图
/// 在 >64 槽路径下，emit 内部使用此结构批量收集待执行项，
/// 达到阈值后统一执行，减少 per-slot 分支与锁竞争。
///
/// # 不变性
/// - `pending.len() <= BATCH_MAX_SIZE`（push 后超限会立即触发调用方 flush）
/// - `last_flush` 单调递增
///
/// # 线程安全
/// 本结构不是 `Sync` 的，需通过外部锁保护。
#[derive(Debug)]
pub struct EmitBatch<T> {
    /// 待执行项队列（slot_index, value）
    pub pending: VecDeque<(usize, T)>,
    /// 上次 flush 时间点
    pub last_flush: Instant,
}

impl<T> EmitBatch<T> {
    /// 创建一个空批次，`last_flush` 初始化为当前时刻
    pub fn new() -> Self {
        Self { pending: VecDeque::with_capacity(BATCH_MAX_SIZE), last_flush: Instant::now() }
    }

    /// 追加一项到待执行队列
    pub fn push(&mut self, slot_index: usize, value: T) {
        self.pending.push_back((slot_index, value));
    }

    /// 当前待执行项数量
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 是否应该立即 flush
    ///
    /// 满足以下任一条件即返回 true：
    /// - 待执行项数量达到 [`BATCH_MAX_SIZE`]
    /// - 距上次 flush 超过 [`BATCH_THRESHOLD`]
    pub fn should_flush(&self) -> bool {
        self.pending.len() >= BATCH_MAX_SIZE || self.last_flush.elapsed() >= BATCH_THRESHOLD
    }

    /// 取出全部待执行项（按入队顺序）
    ///
    /// 同时刷新 `last_flush` 时间戳。
    pub fn drain(&mut self) -> Vec<(usize, T)> {
        self.last_flush = Instant::now();
        self.pending.drain(..).collect()
    }
}

impl<T> Default for EmitBatch<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_stats_new_starts_cold() {
        let s = SlotStats::new();
        assert_eq!(s.tier, SlotTier::Cold);
        assert_eq!(s.decayed_score, 0.0);
        assert_eq!(s.call_count, 0);
    }

    #[test]
    fn slot_stats_record_call_increments_count() {
        let mut s = SlotStats::new();
        for _ in 0..10 {
            s.record_call();
        }
        assert_eq!(s.call_count, 10);
        // 10 次调用后分数应大于 0.1（Warm 阈值）
        assert!(s.decayed_score > WARM_THRESHOLD);
    }

    #[test]
    fn slot_stats_hot_threshold_transition() {
        let mut s = SlotStats::new();
        // 持续调用直到进入 Hot
        // 稳态分数 ≈ CALL_INCREMENT / (1 - DECAY_FACTOR) = 0.05 / 0.05 = 1.0
        for _ in 0..100 {
            s.record_call();
        }
        assert_eq!(s.tier, SlotTier::Hot);
        assert!(s.decayed_score > HOT_THRESHOLD);
    }

    #[test]
    fn slot_stats_decay_reduces_score() {
        let mut s = SlotStats::new();
        for _ in 0..50 {
            s.record_call();
        }
        let score_before = s.decayed_score;
        s.decay();
        assert!(s.decayed_score < score_before);
    }

    #[test]
    fn slot_stats_decay_eventually_cools_down() {
        let mut s = SlotStats::new();
        for _ in 0..100 {
            s.record_call();
        }
        assert_eq!(s.tier, SlotTier::Hot);
        // 多次衰减后应降至 Cold
        for _ in 0..200 {
            s.decay();
        }
        assert_eq!(s.tier, SlotTier::Cold);
        assert!(s.decayed_score < WARM_THRESHOLD);
    }

    #[test]
    fn emit_batch_new_is_empty() {
        let b: EmitBatch<i32> = EmitBatch::new();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn emit_batch_push_and_drain() {
        let mut b = EmitBatch::new();
        b.push(0, 10);
        b.push(1, 20);
        b.push(2, 30);
        assert_eq!(b.len(), 3);
        assert!(!b.is_empty());

        let drained = b.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0], (0, 10));
        assert_eq!(drained[1], (1, 20));
        assert_eq!(drained[2], (2, 30));
        assert!(b.is_empty());
    }

    #[test]
    fn emit_batch_should_flush_on_full() {
        let mut b = EmitBatch::new();
        for i in 0..BATCH_MAX_SIZE {
            b.push(i, i as i32);
        }
        assert!(b.should_flush());
    }

    #[test]
    fn emit_batch_should_not_flush_when_partial_and_recent() {
        let b: EmitBatch<i32> = EmitBatch::new();
        // 刚创建，未超时，未满
        assert!(!b.should_flush());
    }

    #[test]
    fn emit_batch_drain_resets_last_flush() {
        let mut b = EmitBatch::new();
        b.push(0, 1);
        // 模拟时间流逝（通过 drain 重置时间戳）
        let first_flush = b.last_flush;
        b.drain();
        // drain 后 last_flush 应更新（不早于原值）
        assert!(b.last_flush >= first_flush);
    }

    #[test]
    fn emit_batch_drain_order_is_fifo() {
        let mut b = EmitBatch::new();
        for i in 0..10 {
            b.push(i, i * 10);
        }
        let drained = b.drain();
        // 验证 FIFO 顺序
        for (idx, (slot, val)) in drained.iter().enumerate() {
            assert_eq!(*slot, idx);
            assert_eq!(*val, idx * 10);
        }
    }
}

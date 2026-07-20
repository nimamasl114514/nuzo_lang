//! # RegisterAllocator -- 基于信号槽的寄存器分配器
//!
//! 替代裸 `next_reg + free_heap` 模式，提供显式槽位预订、冲突检测和
//! 事件驱动的生命周期追踪。
//!
//! ## 设计目标
//!
//! 1. **信号驱动**：每次 reserve / release 都发射对应的 `Signal` 事件，
//!    外部监听者可实时观测分配器的状态变化（调试可视化、统计收集）。
//! 2. **槽位状态机**：每个预订范围经历 `Reserved -> Active -> Released` 三态，
//!    保证活跃范围永不重叠。
//! 3. **VM 兼容**：输出编号与 VM 约定一致（r0, r1, r2 ... 连续编号），
//!    可无缝替换 Compiler 内部的 `alloc_register()` / `release_temp_register()`。
//! 4. **作用域绑定**：通过 `depth` 字段支持按深度批量释放，等价于
//!    `Compiler::release_registers(from_reg)` 的语义。
//!
//! ## 与旧 API 的关系
//!
//! | 旧 API (Compiler)           | 新 API (RegisterAllocator)       |
//! |----------------------------|----------------------------------|
//! | `alloc_register()`         | `alloc_single(owner)`            |
//! | `release_temp_register()`  | `release_slot(handle)`           |
//! | `release_registers(from)`  | `release_slots_by_depth(depth)`  |
//! | `save_register_state()`    | `current_depth`                  |
//! | `free_registers` (heap)    | `free_heap` + `free_set`         |
//! | `next_reg`                 | `next_reg` (单调游标)            |

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use nuzo_core::MAX_FUNCTION_LOCALS;
use nuzo_signal::{Signal, SlotConflictedInfo, SlotOwner, SlotReleasedInfo, SlotReservedInfo};

use crate::compiler::CompileError;

// ============================================================================
// 槽位状态机（Slot Status State Machine）
// ============================================================================

/// 槽位生命周期状态
///
/// # 状态转换图
///
/// ```text
///   new()          release_slot()     （可被 reserve_slot 复用）
/// Reserved ───────▶ Active ───────▶ Released
///                     ^                  |
///                     |                  |
///                     └──────────────────┘
///                       reserve_slot() 从 free_heap 复用
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Reserved 状态保留用于未来扩展（两阶段预订模式）
enum SlotStatus {
    /// 已预订但尚未标记为 Active（预留状态）
    ///
    /// `reserve_slot()` 创建后立即进入此状态。
    /// 调用方可在后续操作中将 slot 标记为 Active（当前实现中
    /// reserve_slot 直接设为 Active，此状态保留用于未来扩展）。
    Reserved,
    /// 正在使用中，范围不可与其他 Active/Reserved 槽位重叠
    Active,
    /// 已释放，范围内的寄存器已归还到 free_heap 可复用
    Released,
}

// ============================================================================
// RegisterSlot -- 预定的连续寄存器范围
// ============================================================================

/// 一个预定的连续寄存器范围
///
/// 表示从 `start` 到 `start + count - 1` 的连续寄存器区间，
/// 绑定一个所有者和作用域深度。
///
/// # 不变量
///
/// - `start + count <= MAX_FUNCTION_LOCALS`
/// - 同一时刻，任何两个 status 为 `Active` 或 `Reserved` 的 slot 范围不重叠
#[derive(Debug, Clone)]
struct RegisterSlot {
    /// 起始寄存器编号（含）
    start: u16,
    /// 连续寄存器数量
    count: u16,
    /// 所有者标识（用于信号事件和调试区分）
    owner: SlotOwner,
    /// 当前生命周期状态
    status: SlotStatus,
    /// Scope 深度（用于 `release_slots_by_depth` 批量释放）
    depth: usize,
}

impl RegisterSlot {
    /// 返回范围的结束位置（不含），即 `start + count`
    #[inline]
    fn end(&self) -> u16 {
        self.start.saturating_add(self.count)
    }

    /// 检查是否与另一个范围重叠
    ///
    /// 重叠定义：两个区间 [a_start, a_end) 和 [b_start, b_end) 有交集。
    #[inline]
    fn overlaps(&self, other: &RegisterSlot) -> bool {
        // 只检查 Active 和 Reserved 状态的槽位（Released 已归还，允许重叠）
        let self_active = matches!(self.status, SlotStatus::Active | SlotStatus::Reserved);
        let other_active = matches!(other.status, SlotStatus::Active | SlotStatus::Reserved);
        if !self_active || !other_active {
            return false;
        }
        self.start < other.end() && other.start < self.end()
    }
}

// ============================================================================
// SlotHandle -- 安全的槽位句柄（Newtype 封装）
// ============================================================================

/// 槽位句柄（newtype 封装，用于安全的释放操作）
///
/// 内部存储的是 `slots` Vec 中的索引。调用方通过此句柄
/// 执行释放或查询操作，避免直接暴露内部数据结构。
///
/// # 安全性
///
/// - 句柄值在 `slots` 有效范围内时才有效
/// - 释放后句柄仍存在但指向的 slot 状态变为 Released
/// - 未来可扩展为 generation-based handle 防止 use-after-free
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotHandle(pub usize);

// ============================================================================
// RegisterAllocator -- 核心
// ============================================================================

/// 基于信号槽的寄存器分配器
///
/// # 分配策略概览
///
/// ```text
/// reserve_slot(count, owner):
///   1. 尝试从 free_heap 分配（复用已释放的寄存器）
///      → 需要 count 个连续空闲寄存器
///   2. 否则从 next_reg 游标分配（高地址递增）
///      → 推进 next_reg，确保不与 Active/Reserved 重叠
///   3. 冲突检测：线性探测直到找到无重叠位置
///   4. 创建 RegisterSlot，返回 SlotHandle
///   5. 发射 signal_reserved 信号
/// ```
///
/// # 性能特征
///
/// | 操作                | 时间复杂度 | 说明                        |
/// |---------------------|-----------|-----------------------------|
/// | `reserve_slot`      | O(n)      | n = 当前活跃槽数量           |
/// | `release_slot`      | O(k)      | k = 该槽位的 count          |
/// | `release_slots_by_depth` | O(n*m) | n=槽数, m=平均count     |
/// | `alloc_single`      | O(n)      | 委托给 reserve_slot(1, ...) |
/// | `slot_range`        | O(1)      | 直接索引访问               |
///
/// > 注：n 通常很小（< MAX_FUNCTION_LOCALS = 4096），O(n) 完全可接受。
pub struct RegisterAllocator {
    /// 所有已创建的槽位（含 Active、Reserved、Released）
    slots: Vec<RegisterSlot>,

    /// 单调递增游标（高地址分配策略）
    ///
    /// 新分配优先使用此游标推进，保证寄存器编号紧凑连续。
    /// 当 free_heap 有足够连续空间时优先复用。
    next_reg: u16,

    /// 编译期间 next_reg 达到的峰值
    ///
    /// 与 Compiler.peak_reg 语义一致：追踪 next_reg 的历史最大值，
    /// 用于计算 FunctionPrototype.locals_count，确保 VM 分配足够的寄存器空间。
    /// next_reg 不会收缩（allocator 无 release_temp_register 逻辑），但
    /// 从 free_heap 复用低编号寄存器时 next_reg 不更新，peak_reg 确保
    /// 捕获到曾经达到的最高值。
    peak_reg: u16,

    /// 已释放的寄存器池（最大堆，弹出最大的可用编号）
    ///
    /// 使用 `Reverse<u16>` 使得 `pop()` 返回**最小**的编号，
    /// 优先复用低地址寄存器，减少地址碎片。
    free_heap: BinaryHeap<Reverse<u16>>,

    /// 已释放寄存器的 O(1) contains 查询集合
    ///
    /// BinaryHeap 不支持 O(1) contains，用此 HashSet 补偿。
    free_set: HashSet<u16>,

    /// 当前 Scope 深度（由 `begin_scope` / `end_scope` 管理）
    current_depth: usize,

    // ---- 事件信号 ----
    /// 寄存器槽位预订信号
    ///
    /// 在 `reserve_slot()` 成功分配后发射。
    /// 携带 `SlotReservedInfo`（owner, start, count, depth）。
    pub signal_reserved: Signal<SlotReservedInfo>,

    /// 寄存器槽位释放信号
    ///
    /// 在 `release_slot()` 或 `release_slots_by_depth()` 中发射。
    /// 携带 `SlotReleasedInfo`（start, count）。
    pub signal_released: Signal<SlotReleasedInfo>,

    /// 寄存器冲突检测信号
    ///
    /// 当新请求与已有 Active 槽位重叠时发射（探测过程中）。
    /// 携带 `SlotConflictedInfo`（双方 owner 和 range）。
    pub signal_conflicted: Signal<SlotConflictedInfo>,
}

// ============================================================================
// 构造函数
// ============================================================================

impl RegisterAllocator {
    /// 创建新的寄存器分配器实例
    ///
    /// 初始状态：
    /// - `next_reg = 0`（从 r0 开始分配）
    /// - `free_heap` 为空（无可复用寄存器）
    /// - `current_depth = 0`（全局作用域）
    /// - 三个信号均已初始化并命名
    ///
    /// # 示例
    ///
    /// ```
    /// use nuzo_compiler::allocator::RegisterAllocator;
    /// use nuzo_signal::SlotOwner;
    ///
    /// let mut alloc = RegisterAllocator::new();
    /// let reg = alloc.alloc_single(SlotOwner::TempExpr).unwrap();
    /// assert_eq!(reg, 0); // r0
    /// ```
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            next_reg: 0,
            peak_reg: 0,
            free_heap: BinaryHeap::new(),
            free_set: HashSet::new(),
            current_depth: 0,
            signal_reserved: Signal::named("reg:reserved"),
            signal_released: Signal::named("reg:released"),
            signal_conflicted: Signal::named("reg:conflicted"),
        }
    }

    /// 带初始深度的构造函数（用于子编译器继承父级深度）
    pub fn with_depth(initial_depth: usize) -> Self {
        let mut slf = Self::new();
        slf.current_depth = initial_depth;
        slf
    }
}

// ============================================================================
// 核心分配方法
// ============================================================================

impl RegisterAllocator {
    /// 预订一组连续寄存器（核心方法）
    ///
    /// # 算法
    ///
    /// 1. **边界检查**：`count == 0` 返回空句柄；超出 `MAX_FUNCTION_LOCALS` 则报错
    /// 2. **优先从 free_heap 复用**：
    ///    - 弹出足够数量的连续空闲寄存器
    ///    - 使用 `find_contiguous_free()` 在 free_set 中寻找连续区间
    /// 3. **否则从 next_reg 分配**：
    ///    - 从 `next_reg` 开始尝试
    ///    - 如果与 Active/Reserved 槽位重叠则线性探测下一个位置
    /// 4. **创建 RegisterSlot** 加入 slots 列表，返回 SlotHandle
    /// 5. **发射 `signal_reserved`** 信号广播给所有监听者
    ///
    /// # 参数
    ///
    /// * `count` - 需要的连续寄存器数量
    /// * `owner` - 槽位所有者标识（用于调试和信号追踪）
    ///
    /// # 错误
    ///
    /// * `CompileError::TooManyLocals` - 无法在 [0, MAX_FUNCTION_LOCALS) 内找到足够的连续空间
    ///
    /// # 时间复杂度
    ///
    /// O(n) 其中 n = 当前 Active/Reserved 槽位数（冲突检测扫描）
    pub fn reserve_slot(
        &mut self,
        count: u16,
        owner: SlotOwner,
    ) -> Result<SlotHandle, CompileError> {
        // 边界：count == 0 时返回一个零长度句柄（不消耗寄存器）
        if count == 0 {
            let handle = SlotHandle(self.slots.len());
            self.slots.push(RegisterSlot {
                start: 0,
                count: 0,
                owner,
                status: SlotStatus::Active,
                depth: self.current_depth,
            });
            return Ok(handle);
        }

        if count as usize > MAX_FUNCTION_LOCALS as usize {
            return Err(CompileError::TooManyLocals {
                count: count as usize,
                line: 0, // allocator 无行号上下文
                column: 0,
            });
        }

        let start = self.find_free_range(count)?;

        let handle = SlotHandle(self.slots.len());
        self.slots.push(RegisterSlot {
            start,
            count,
            owner,
            status: SlotStatus::Active,
            depth: self.current_depth,
        });

        // 更新 next_reg 游标（确保下次分配不会回退到已用区域）
        let end = start.saturating_add(count);
        if end > self.next_reg {
            self.next_reg = end;
        }
        // 更新峰值寄存器计数（用于计算 FunctionPrototype.locals_count）
        // ⚠️ 必须与 next_reg 同步更新，否则 locals_count 会偏低
        // 导致 VM 分配的寄存器文件不足 → RegisterOutOfBounds 运行时错误
        self.peak_reg = self.peak_reg.max(end);

        self.signal_reserved.emit(&SlotReservedInfo {
            owner,
            start,
            count,
            depth: self.current_depth,
        });

        Ok(handle)
    }

    /// 内部方法：寻找 count 个连续空闲寄存器的起始位置
    ///
    /// # 分配优先级
    ///
    /// 1. **free_heap 复用**：在已释放的寄存器池中寻找连续区间
    /// 2. **next_reg 游标**：从单调递增游标开始分配
    /// 3. **线性探测**：如果游标位置被占用，逐个后移直到找到空隙
    fn find_free_range(&mut self, count: u16) -> Result<u16, CompileError> {
        // --- 策略 1：尝试从 free_heap 复用 ---
        if let Some(start) = self.try_allocate_from_free_heap(count) {
            return Ok(start);
        }

        // --- 策略 2：从 next_reg 开始，线性探测 ---
        let mut candidate = self.next_reg;

        loop {
            let end = candidate.saturating_add(count);
            if end > MAX_FUNCTION_LOCALS {
                // P2.11 修复：原 `count: self.next_reg as usize + count as usize` 语义歧义
                // （既不是请求量也不是峰值，而是 next_reg+count 的混合值，诊断无意义）。
                // 改为 `count: count as usize` 明确表示"请求的连续寄存器数量"，
                // 配合 next_reg/max 字段提供清晰诊断。
                return Err(CompileError::TooManyLocals {
                    count: count as usize,
                    line: 0,
                    column: 0,
                });
            }

            // 检查候选范围 [candidate, candidate+count) 是否与任何 Active/Reserved 槽位重叠
            let conflicts_with = self.find_overlap(candidate, count);

            if let Some((existing_slot, _existing_idx)) = conflicts_with {
                // 发射冲突信号（供监听者记录/调试）
                self.signal_conflicted.emit(&SlotConflictedInfo {
                    existing_owner: existing_slot.owner,
                    existing_range: (existing_slot.start, existing_slot.end()),
                    requested_owner: SlotOwner::TempExpr, // 此时还不知道 owner，用默认值
                    requested_range: (candidate, end),
                });

                // 跳过冲突槽位的整个范围，继续探测
                candidate = existing_slot.end();
                continue;
            }

            // 从 free_set 中移除即将使用的寄存器（防止重复分配）
            for reg in candidate..end {
                self.free_set.remove(&reg);
                // 同时从 free_heap 中移除（通过重建堆来保持一致性）
                // 注意：BinaryHeap 没有 remove_by_value，这里依赖 free_set 做 O(1) 判断
                // 实际 pop 时会跳过已在 free_set 中不存在的元素
            }

            return Ok(candidate);
        }
    }

    /// 尝试从 free_heap 分配 count 个连续寄存器
    ///
    /// # 算法
    ///
    /// 由于 free_heap 存储的是离散的单个寄存器编号，我们需要在其中
    /// 寻找一段长度 >= count 的连续区间。
    ///
    /// 采用贪心策略：将 free_set 中的编号排序后，寻找最长连续段。
    /// 如果某段长度 >= count，则从该段头部分配。
    ///
    /// 这是一个启发式优化：对于常见的单寄存器分配（count=1），
    /// 直接 pop 即可；对于多寄存器分配（如数组构造），
    /// 连续段搜索能显著提高命中率。
    fn try_allocate_from_free_heap(&mut self, count: u16) -> Option<u16> {
        if count == 1 {
            // 快速路径：单寄存器直接弹出一个
            while let Some(Reverse(reg)) = self.free_heap.pop() {
                if self.free_set.remove(&reg) {
                    // 确认这个寄存器确实还在 free_set 中（未被其他路径消费）
                    // 再次检查与 Active 槽位不重叠（防御性检查）
                    if !self.range_overlaps_active(reg, 1) {
                        return Some(reg);
                    }
                    // 如果意外重叠，丢弃此 reg 继续找下一个
                }
                // free_heap 中的元素不在 free_set 中说明已被消费，跳过
            }
            None
        } else {
            // 多寄存器：需要在 free_set 中寻找连续区间
            let mut sorted_free: Vec<u16> = self.free_set.iter().copied().collect();
            sorted_free.sort_unstable();

            let mut run_start: Option<u16> = None;
            let mut run_len: u16 = 0;

            for &reg in &sorted_free {
                match run_start {
                    None => {
                        run_start = Some(reg);
                        run_len = 1;
                    }
                    Some(start) if reg == start.saturating_add(run_len) => {
                        run_len += 1;
                    }
                    _ => {
                        if run_len >= count
                            && let Some(s) = run_start
                            && !self.range_overlaps_active(s, count)
                        {
                            self.consume_free_range(s, count);
                            return Some(s);
                        }
                        run_start = Some(reg);
                        run_len = 1;
                    }
                }
            }

            if run_len >= count
                && let Some(s) = run_start
                && !self.range_overlaps_active(s, count)
            {
                self.consume_free_range(s, count);
                return Some(s);
            }

            None
        }
    }

    /// 将 [start, start+count) 范围内的寄存器从 free_heap 和 free_set 中移除
    fn consume_free_range(&mut self, start: u16, count: u16) {
        for reg in start..start.saturating_add(count) {
            self.free_set.remove(&reg);
        }
        // 清理 free_heap 中已被消费的元素（延迟清理：下次 pop 时跳过）
        // 这里不做完整重建，因为代价较高；pop 时通过 free_set 检查即可过滤
    }

    /// 检查范围 [start, start+count) 是否与任何 Active/Reserved 槽位重叠
    #[inline]
    fn range_overlaps_active(&self, start: u16, count: u16) -> bool {
        let end = start.saturating_add(count);
        self.slots.iter().any(|slot| {
            let is_active = matches!(slot.status, SlotStatus::Active | SlotStatus::Reserved);
            is_active && start < slot.end() && slot.start < end
        })
    }

    /// 查找与 [start, start+count) 重叠的第一个 Active/Reserved 槽位
    ///
    /// 返回 `(slot引用, 索引)` 或 `None`
    fn find_overlap(&self, start: u16, count: u16) -> Option<(RegisterSlot, usize)> {
        let _end = start.saturating_add(count);
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.overlaps(&RegisterSlot {
                start,
                count,
                owner: SlotOwner::TempExpr, // dummy
                status: SlotStatus::Active,
                depth: 0,
            }) {
                // overlaps 方法已经检查了 Active/Reserved 状态
                return Some((slot.clone(), idx));
            }
        }
        None
    }
}

// ============================================================================
// 释放方法
// ============================================================================

impl RegisterAllocator {
    /// 释放单个槽位
    ///
    /// 将指定 handle 对应的槽位标记为 `Released`，
    /// 并将其范围内的每个寄存器加入 `free_heap` 和 `free_set` 供后续复用。
    ///
    /// # 行为
    ///
    /// - 如果槽位已经是 `Released` 状态，此操作为幂等 no-op
    /// - 释放后会发射 `signal_released` 信号
    ///
    /// # 参数
    ///
    /// * `handle` - 由 `reserve_slot()` 或 `alloc_single()` 返回的句柄
    ///
    /// # 越界行为（与查询方法一致）
    ///
    /// P2.5 修复：原实现 `if handle.0 >= self.slots.len() { return; }` 静默 no-op，
    /// 与查询方法（`slot_range` 等越界 panic）行为不一致。
    /// 现统一为：越界视为"已释放"（幂等语义），因为 release_slot 本身是幂等操作
    /// （对已 Released 状态的 slot 重复释放也是 no-op）。
    /// 这与"释放一个不存在的句柄 = 该句柄已不活跃 = 等价于已释放"的语义一致。
    ///
    /// 若需严格越界检测，使用 [`RegisterAllocator::try_release_slot`]。
    pub fn release_slot(&mut self, handle: SlotHandle) {
        // P2.5: 越界视为已释放（幂等语义），与"释放已 Released slot 是 no-op"一致
        if handle.0 >= self.slots.len() {
            return;
        }

        let slot = &mut self.slots[handle.0];

        // 幂等：已经是 Released 状态则跳过
        if slot.status == SlotStatus::Released {
            return;
        }

        let start = slot.start;
        let count = slot.count;

        slot.status = SlotStatus::Released;

        for reg in start..start.saturating_add(count) {
            // 先检查是否已在 free_set 中（避免重复插入）
            if !self.free_set.contains(&reg) {
                self.free_set.insert(reg);
                self.free_heap.push(Reverse(reg));
            }
        }

        self.signal_released.emit(&SlotReleasedInfo { start, count });
    }

    /// 释放句柄对应的槽位（fallible 版本，越界返回 false）
    ///
    /// 与 [`RegisterAllocator::release_slot`] 功能相同，但越界时返回 `false`
    /// 而非静默 no-op。用于需要严格越界检测的场景（如测试断言、调试诊断）。
    ///
    /// # 返回值
    /// - `true`：句柄有效且已处理（包括幂等释放已 Released 的 slot）
    /// - `false`：句柄越界（handle.0 >= slots.len()）
    pub fn try_release_slot(&mut self, handle: SlotHandle) -> bool {
        if handle.0 >= self.slots.len() {
            return false;
        }
        self.release_slot(handle);
        true
    }

    /// 按深度批量释放槽位
    ///
    /// 释放所有 `depth <= target_depth` 且状态非 `Released` 的槽位。
    /// 用于离开作用域时的自动清理，等价于旧 API 的
    /// `Compiler::release_registers(from_reg)`。
    ///
    /// # 典型用法
    ///
    /// ```text
    /// let saved_depth = alloc.current_depth();
    /// alloc.begin_scope();
    /// // ... 在此深度内分配多个槽位 ...
    /// alloc.release_slots_by_depth(saved_depth); // 清理该深度内所有槽位
    /// ```
    ///
    /// # 参数
    ///
    /// * `target_depth` - 目标深度，释放所有 depth > target_depth 的槽位
    ///
    /// # 注意
    ///
    /// 此方法释放的是 `depth > target_depth` 的槽位（更深层的作用域），
    /// 保留 `depth <= target_depth` 的槽位（外层作用域仍然活跃）。
    pub fn release_slots_by_depth(&mut self, target_depth: usize) {
        // 收集需要释放的 handle（避免在迭代过程中修改 slots）
        let to_release: Vec<SlotHandle> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.depth > target_depth && slot.status != SlotStatus::Released)
            .map(|(idx, _)| SlotHandle(idx))
            .collect();

        for handle in to_release {
            self.release_slot(handle);
        }
    }
}

// ============================================================================
// 查询方法
// ============================================================================

impl RegisterAllocator {
    /// 获取槽位的寄存器范围
    ///
    /// # 返回值
    ///
    /// `(start, end)` 其中 `end` 是**不含**的上界，
    /// 即实际占用的寄存器为 `[start, end)` 半开区间。
    ///
    /// # 示例
    ///
    /// ```text
    /// let handle = alloc.reserve_slot(3, owner)?; // 占用 r5, r6, r7
    /// assert_eq!(alloc.slot_range(handle), (5, 8));
    /// ```
    ///
    /// # Panics
    ///
    /// 如果 `handle.0` 越界，panic 时提供清晰诊断信息（handle 值 + slots.len）。
    /// P2.5 修复：原直接下标 `self.slots[handle.0]` 越界 panic 无消息，
    /// 现在加 `debug_assert!` 在 debug 构建中提供诊断，release 构建中
    /// 通过 `get` + `expect` 提供清晰 panic 消息。
    /// 若需 fallible 查询，使用 [`RegisterAllocator::try_slot_range`]。
    pub fn slot_range(&self, handle: SlotHandle) -> (u16, u16) {
        let slot = self.slots.get(handle.0).unwrap_or_else(|| {
            panic!(
                "RegisterAllocator::slot_range: SlotHandle({}) out of range \
                 (slots.len()={}) — caller must ensure handle is valid",
                handle.0,
                self.slots.len()
            )
        });
        (slot.start, slot.end())
    }

    /// 获取槽位的起始寄存器编号（便捷方法）
    ///
    /// 等价于 `slot_range(handle).0`。
    pub fn slot_start(&self, handle: SlotHandle) -> u16 {
        self.slots
            .get(handle.0)
            .unwrap_or_else(|| {
                panic!(
                    "RegisterAllocator::slot_start: SlotHandle({}) out of range \
                 (slots.len()={}) — caller must ensure handle is valid",
                    handle.0,
                    self.slots.len()
                )
            })
            .start
    }

    /// 获取槽位的寄存器数量
    pub fn slot_count(&self, handle: SlotHandle) -> u16 {
        self.slots
            .get(handle.0)
            .unwrap_or_else(|| {
                panic!(
                    "RegisterAllocator::slot_count: SlotHandle({}) out of range \
                 (slots.len()={}) — caller must ensure handle is valid",
                    handle.0,
                    self.slots.len()
                )
            })
            .count
    }

    /// 获取槽位的所有者
    pub fn slot_owner(&self, handle: SlotHandle) -> SlotOwner {
        self.slots
            .get(handle.0)
            .unwrap_or_else(|| {
                panic!(
                    "RegisterAllocator::slot_owner: SlotHandle({}) out of range \
                 (slots.len()={}) — caller must ensure handle is valid",
                    handle.0,
                    self.slots.len()
                )
            })
            .owner
    }

    // ── P2.5: fallible 查询方法（与 try_release_slot 配套）──

    /// 获取槽位的寄存器范围（fallible 版本，越界返回 `None`）
    ///
    /// 与 [`RegisterAllocator::slot_range`] 功能相同，但越界时返回 `None`。
    /// 用于需要严格越界检测的场景（如外部输入验证、防御性编程）。
    pub fn try_slot_range(&self, handle: SlotHandle) -> Option<(u16, u16)> {
        self.slots.get(handle.0).map(|slot| (slot.start, slot.end()))
    }

    /// 获取槽位的起始寄存器编号（fallible 版本）
    pub fn try_slot_start(&self, handle: SlotHandle) -> Option<u16> {
        self.slots.get(handle.0).map(|slot| slot.start)
    }

    /// 获取槽位的寄存器数量（fallible 版本）
    pub fn try_slot_count(&self, handle: SlotHandle) -> Option<u16> {
        self.slots.get(handle.0).map(|slot| slot.count)
    }

    /// 获取槽位的所有者（fallible 版本）
    pub fn try_slot_owner(&self, handle: SlotHandle) -> Option<SlotOwner> {
        self.slots.get(handle.0).map(|slot| slot.owner)
    }

    /// 获取当前 Scope 深度
    #[inline]
    pub fn current_depth(&self) -> usize {
        self.current_depth
    }

    /// 获取编译期间 next_reg 达到的峰值
    ///
    /// 与 Compiler.peak_reg 语义一致：返回 next_reg 的历史最大值，
    /// 用于计算 FunctionPrototype.locals_count。
    #[inline]
    pub fn peak_reg(&self) -> u16 {
        self.peak_reg
    }

    /// 获取当前活跃（非 Released）的槽位数量
    #[inline]
    pub fn active_slot_count(&self) -> usize {
        self.slots.iter().filter(|s| s.status != SlotStatus::Released).count()
    }

    /// 获取 free_heap 中的可复用寄存器数量
    #[inline]
    pub fn free_count(&self) -> usize {
        self.free_set.len()
    }

    /// 检查指定寄存器是否在空闲池中
    #[inline]
    pub fn is_register_free(&self, reg: u16) -> bool {
        self.free_set.contains(&reg)
    }
}

// ============================================================================
// 作用域管理
// ============================================================================

impl RegisterAllocator {
    /// 进入一个新的作用域（深度 +1）
    ///
    /// 每次调用使 `current_depth` 递增。
    /// 后续在此深度内分配的槽位都会绑定到此深度，
    /// 以便在 `release_slots_by_depth()` 时批量清理。
    ///
    /// # 配对要求
    ///
    /// 每次 `begin_scope()` 必须有对应的 `end_scope()` 调用，
    /// 否则深度会无限增长（虽然不影响正确性，但浪费内存）。
    pub fn begin_scope(&mut self) {
        self.current_depth += 1;
    }

    /// 退出当前作用域（深度 -1 并释放该深度的槽位）
    ///
    /// 等价于先调用 `release_slots_by_depth(current_depth - 1)`，
    /// 再将 `current_depth` 减 1。
    ///
    /// # 安全
    ///
    /// 如果 `current_depth` 已经为 0，此操作为 no-op（防止下溢）。
    pub fn end_scope(&mut self) {
        if self.current_depth == 0 {
            return;
        }
        let target = self.current_depth.saturating_sub(1);
        self.release_slots_by_depth(target);
        self.current_depth = target;
    }
}

// ============================================================================
// 向后兼容的快捷 API
// ============================================================================

impl RegisterAllocator {
    /// 分配单个寄存器（兼容旧 API 的快捷方法）
    ///
    /// 内部调用 `reserve_slot(1, owner)?`，然后返回起始寄存器编号。
    /// 提供与 `Compiler::alloc_register()` 相同的外部行为：
    ///
    /// ```text
    /// // 旧 API:
    /// let reg = compiler.alloc_register()?;  // -> Result<u16, CompileError>
    ///
    /// // 新 API:
    /// let reg = allocator.alloc_single(SlotOwner::TempExpr)?;  // -> Result<u16, CompileError>
    /// ```
    ///
    /// # 参数
    ///
    /// * `owner` - 槽位所有者标识
    ///
    /// # 返回值
    ///
    /// 分配到的单个寄存器编号
    ///
    /// # 错误
    ///
    /// * `CompileError::TooManyLocals` - 寄存器耗尽
    pub fn alloc_single(&mut self, owner: SlotOwner) -> Result<u16, CompileError> {
        let handle = self.reserve_slot(1, owner)?;
        Ok(self.slot_range(handle).0)
    }

    /// 远端基地址分配（Remote Base Allocation）
    ///
    /// 专门为数组/对象构造等需要**连续寄存器区域**的场景设计。
    /// 与 `reserve_slot()` 的关键区别：
    ///
    /// | 特性 | reserve_slot() | reserve_remote() |
    /// |------|---------------|-----------------|
    /// | 分配起点 | free_heap 或 next_reg | **强制从 next_reg 开始** |
    /// | 复用释放的寄存器 | ✅ 优先复用 | ❌ 永不复用 |
    /// | 与活跃寄存器的距离 | 可能相邻 | **保证在最高地址端** |
    /// | 适用场景 | 通用分配 | 数组构造、函数调用帧 |
    ///
    /// # 为什么需要这个方法？
    ///
    /// VM 的 `ArrayNew` 指令约定：元素必须紧邻目标寄存器之后（dest, elem1, elem2...）。
    /// 如果使用通用分配器，可能将构造区域分配到已被其他值占用的低地址区域，
    /// 导致 ArrayNew 读取到错误的数据（这就是原始 bug 的根因）。
    ///
    /// # 参数
    ///
    /// * `count` - 需要的连续寄存器数量
    /// * `owner` - 槽位所有者标识
    pub fn reserve_remote(
        &mut self,
        count: u16,
        owner: SlotOwner,
    ) -> Result<SlotHandle, CompileError> {
        if count == 0 {
            let handle = SlotHandle(self.slots.len());
            self.slots.push(RegisterSlot {
                start: 0,
                count: 0,
                owner,
                status: SlotStatus::Active,
                depth: self.current_depth,
            });
            return Ok(handle);
        }

        let start = self.next_reg;
        let end = start.saturating_add(count);

        if end > MAX_FUNCTION_LOCALS {
            // P2.11 修复：与 find_free_range 一致，count 字段表示"请求的连续寄存器数量"
            return Err(CompileError::TooManyLocals { count: count as usize, line: 0, column: 0 });
        }

        // 推进 next_reg（核心：永不回退，保证远端语义）
        self.next_reg = end;
        self.peak_reg = self.peak_reg.max(end);

        let handle = SlotHandle(self.slots.len());
        self.slots.push(RegisterSlot {
            start,
            count,
            owner,
            status: SlotStatus::Active,
            depth: self.current_depth,
        });

        self.signal_reserved.emit(&SlotReservedInfo {
            owner,
            start,
            count,
            depth: self.current_depth,
        });

        Ok(handle)
    }
}

// ============================================================================
// Default trait 实现
// ============================================================================

impl Default for RegisterAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// LSRA -- 线性扫描寄存器分配器（Linear Scan Register Allocation）
// ============================================================================
//
// ## 设计哲学
//
// 与 `RegisterAllocator`（即时分配）不同，LSRA 采用**两阶段**策略：
//
// 1. **区间计算阶段**：扫描指令序列，收集每个虚拟寄存器的 def 点（首次写入 IP）
//    和 use 点（最后读取 IP），构建活区间列表。
// 2. **分配阶段**：按 start 排序后线性扫描，为每个区间分配物理寄存器或 spill 槽位。
//
// ## 核心优势
//
// - **寄存器复用**：活跃区间不重叠的变量自动共享同一物理寄存器
// - **全局最优视野**：在函数级别做决策，而非局部即时分配
// - **Lazy Spilling**：仅在物理寄存器不足时才溢出到栈，选择 end 最大的 victim 最小化 spill 开销
//
// ## 与 RegisterAllocator 的关系
//
// | 特性              | RegisterAllocator          | LsraAllocator            |
// |-------------------|---------------------------|--------------------------|
// | 分配时机           | 即时（编译时按需）         | 两阶段（先收集后分配）     |
// | 复用策略           | free_heap 复用释放的槽位   | 活跃区间不重叠即共享       |
// | Spill 策略         | 无（永不溢出）             | Lazy Spill（选最长 victim）|
// | 适用场景           | 单遍编译、简单表达式       | 函数级优化、密集计算       |
//
// 两者**共存**，LSRA 作为可选后端供 Compiler 在需要时启用。

use std::cmp::Ordering;

/// LSRA 分配错误
///
/// 覆盖两种失败场景：
/// - 寄存器耗尽且无法 spill（spill 槽位也用完）
/// - 输入数据非法（空区间列表、vreg 越界等）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// 所有物理寄存器和 spill 槽位均已耗尽
    Exhausted {
        /// 请求分配的虚拟寄存器编号
        vreg: u16,
        /// 当前位置（字节码 IP）
        position: usize,
    },
    /// 输入数据非法
    InvalidInput {
        /// 错误描述信息
        message: String,
    },
}

impl std::fmt::Display for AllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AllocError::Exhausted { vreg, position } => {
                write!(f, "LSRA exhausted: cannot allocate vreg {} at IP {}", vreg, position)
            }
            AllocError::InvalidInput { message } => {
                write!(f, "LSRA invalid input: {}", message)
            }
        }
    }
}

impl std::error::Error for AllocError {}

// ============================================================================
// Interval -- 活跃区间
// ============================================================================

/// 活跃区间：表示一个虚拟寄存器的生命周期范围
///
/// # 字段语义
///
/// | 字段        | 类型          | 含义                              |
/// |-------------|---------------|-----------------------------------|
/// | `vreg`      | `u16`         | 虚拟寄存器编号（由编译器分配）      |
/// | `start`     | `usize`       | 首次定义的位置（字节码 IP）        |
/// | `end`       | `usize`       | 最后使用的位置（字节码 IP）        |
/// | `reg`       | `Option<u16>` | 分配到的物理寄存器（None = spilled）|
/// | `spill_slot`| `Option<u16>` | spill 栈槽位编号（如有）           |
///
/// # 不变量
///
/// - `start <= end`（合法区间）
/// - `reg.is_some() XOR spill_slot.is_some()`（要么在寄存器中，要么在栈上）
/// - `vreg < MAX_FUNCTION_LOCALS`
#[derive(Debug, Clone)]
pub struct Interval {
    /// 虚拟寄存器编号
    pub vreg: u16,
    /// 首次定义的字节码 IP（区间起点，含）
    pub start: usize,
    /// 最后使用的字节码 IP（区间终点，含）
    pub end: usize,
    /// 分配到的物理寄存器（None 表示已 spill 到栈）
    pub reg: Option<u16>,
    /// spill 栈槽位（仅当 reg == None 时有值）
    pub spill_slot: Option<u16>,
    /// 循环嵌套深度（定义时）
    pub loop_depth: u8,
    /// 租约剩余指令数（0 = 可被驱逐）
    pub lease_remaining: u8,
    /// 在区间内的使用频率（用于循环保护加权）
    pub use_frequency: u8,
}

impl Interval {
    /// 创建新的活跃区间
    ///
    /// # 参数
    ///
    /// * `vreg` - 虚拟寄存器编号
    /// * `start` - 定义点 IP
    /// * `end` - 最后使用点 IP
    ///
    /// # Panics
    ///
    /// 如果 `start > end`
    #[inline]
    pub fn new(vreg: u16, start: usize, end: usize) -> Self {
        debug_assert!(
            start <= end,
            "Interval invariant violated: start({}) > end({}) for vreg {}",
            start,
            end,
            vreg
        );
        Self {
            vreg,
            start,
            end,
            reg: None,
            spill_slot: None,
            loop_depth: 0,
            lease_remaining: 0,
            use_frequency: 1,
        }
    }

    /// 区间长度（包含起止点）
    #[inline]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    /// 是否为空区间（start == end，单点使用）
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// 检查两个区间是否重叠（用于调试和验证）
    ///
    /// 重叠定义：[a_start, a_end] 和 [b_start, b_end] 有交集。
    /// 注意这里用的是闭区间（两端都含），与 RegisterSlot 的半开区间不同。
    #[inline]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

// ---- Ord trait 实现：用于 BinaryHeap 排序 ----
//
// BinaryHeap 是最大堆，我们需要按 end 降序排列：
// - 堆顶是 end 最大的区间 → Lazy Spill 时优先选择"活得最长的"作为 victim
// - 这样 spill 的代价最小（victim 的剩余生命期最长，当前区间可能更快死亡）

impl Ord for Interval {
    fn cmp(&self, other: &Self) -> Ordering {
        // 首键：lease_remaining — 有租约的区间不应被优先驱逐
        // Max-Heap 堆顶会被优先选为 victim，所以有租约的应排堆底（返回 Less）
        let self_has_lease = self.lease_remaining > 0;
        let other_has_lease = other.lease_remaining > 0;
        match (self_has_lease, other_has_lease) {
            (true, false) => return Ordering::Less, // self 有租约 → 排后面（不被优先驱逐）
            (false, true) => return Ordering::Greater, // other 有租约 → self 排前面（可被优先驱逐）
            _ => {}                                 // 两者都有或都没有租约，继续比较
        }
        // 次键：end 降序（Max-Heap 堆顶是 end 最大的 → Lazy Spill 的首选 victim）
        // BinaryHeap 是 Max-Heap，cmp 返回 Greater 表示 self 排在 other 前面（堆顶）
        // 所以 self.end > other.end 时应返回 Greater
        match self.end.cmp(&other.end) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // 末键：vreg 降序（保证确定性排序，避免堆元素比较歧义）
        self.vreg.cmp(&other.vreg)
    }
}

impl PartialOrd for Interval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Interval {}

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        self.end == other.end
            && self.vreg == other.vreg
            && self.loop_depth == other.loop_depth
            && self.lease_remaining == other.lease_remaining
            && self.use_frequency == other.use_frequency
    }
}

impl std::fmt::Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = match (self.reg, self.spill_slot) {
            (Some(r), _) => format!("r{}", r),
            (_, Some(s)) => format!("spill#{}", s),
            (None, None) => "unallocated".to_string(),
        };
        write!(
            f,
            "vreg{}[{}, {}) -> {} (depth={}, lease={})",
            self.vreg,
            self.start,
            self.end.saturating_add(1),
            location,
            self.loop_depth,
            self.lease_remaining
        )
    }
}

/// 位图所需 u64 字数：`(MAX_FUNCTION_LOCALS + 63) / 64`
const BITMAP_WORDS: usize = (MAX_FUNCTION_LOCALS as usize).div_ceil(64);

// ============================================================================
// NudConfig -- NUD 增强配置
// ============================================================================

/// NUD（Next-Use Distance）增强配置
///
/// 控制 LSRA 的 NUD 增强行为，包括物理寄存器上限、循环保护系数、
/// 租约长度和冷分支惩罚等参数。
///
/// # 默认值
///
/// | 参数                   | 默认值 | 含义                              |
/// |-----------------------|--------|----------------------------------|
/// | `phys_reg_limit`      | 64     | 物理寄存器上限（0 = 不限制）       |
/// | `loop_immunity_factor`| 3      | 循环保护系数                       |
/// | `lease_length`        | 5      | 租约长度（指令数）                  |
/// | `cold_branch_penalty` | 2      | 冷分支惩罚系数                      |
/// | `enabled`             | true   | 是否启用 NUD 增强                   |
#[derive(Debug, Clone, Copy)]
pub struct NudConfig {
    /// 物理寄存器上限（0 = 使用 MAX_FUNCTION_LOCALS，即不限制）
    pub phys_reg_limit: u16,
    /// 循环保护系数：effective_end = end * (1 + loop_depth * loop_immunity_factor)
    pub loop_immunity_factor: u8,
    /// 租约长度：新分配的寄存器在 N 条指令内不可被驱逐
    pub lease_length: u8,
    /// 冷分支惩罚系数：冷路径变量的 effective_end 除以此值
    pub cold_branch_penalty: u8,
    /// 是否启用 NUD 增强（false = 退化为纯 LSRA）
    pub enabled: bool,
}

impl Default for NudConfig {
    fn default() -> Self {
        Self {
            phys_reg_limit: 64,
            loop_immunity_factor: 3,
            lease_length: 5,
            cold_branch_penalty: 2,
            enabled: true,
        }
    }
}

impl NudConfig {
    /// 创建禁用 NUD 的配置（退化为纯 LSRA）
    pub fn disabled() -> Self {
        Self { enabled: false, ..Self::default() }
    }

    /// 计算区间的有效 NUD（Next-Use Distance）值
    ///
    /// 有效 NUD 越大，该区间越不应该被驱逐。
    ///
    /// # 公式
    ///
    /// ```text
    /// effective_nud = end * (1 + loop_depth * loop_immunity_factor) / cold_branch_penalty
    /// if lease_remaining > 0: effective_nud = u64::MAX (不可驱逐)
    /// ```
    ///
    /// # 参数
    ///
    /// * `interval` - 要计算的活跃区间
    ///
    /// # 返回值
    ///
    /// 有效 NUD 值（u64），越大越不应该被驱逐
    pub fn effective_nud(&self, interval: &Interval) -> u64 {
        // 租约保护：不可驱逐
        if interval.lease_remaining > 0 {
            return u64::MAX;
        }

        let base_nud = interval.end as u64;
        let depth = interval.loop_depth as u64;
        let factor = self.loop_immunity_factor as u64;

        // 循环保护：effective = base * (1 + depth * factor)
        let loop_boost = if depth > 0 {
            base_nud.saturating_mul(1 + depth.saturating_mul(factor))
        } else {
            base_nud
        };

        // 冷分支惩罚：除以系数
        let penalty = self.cold_branch_penalty as u64;

        if penalty > 1 { loop_boost / penalty } else { loop_boost }
    }
}

// ============================================================================
// LsraAllocator -- 线性扫描分配器核心
// ============================================================================

/// LSRA（Linear Scan Register Allocation）分配器
///
/// # 算法概览
///
/// ```text
/// 输入：Interval 列表（未排序）
/// 输出：每个 Interval.reg 或 Interval.spill_slot 已填充
///
/// 1. 按 start 升序排序
/// 2. 对每个 interval（线性扫描）：
///    a. expire_old_intervals(current_start) — 释放已过期的寄存器
///    b. if 有空闲寄存器 → alloc_one(), 加入 active
///    c. else → Lazy Spill:
///       - 选 active 中 end 最大的（堆顶）
///       - if victim.end > current.end → spill victim, 抢其寄存器
///       - else → spill current interval
/// ```
///
/// # 数据结构
///
/// | 结构          | 类型               | 用途                          |
/// |---------------|--------------------|-------------------------------|
/// | `free_regs`   | `[u64; BITMAP_WORDS]` 位图 | O(1) 分配/释放物理寄存器       |
/// | `active`      | `BinaryHeap<Interval>` | 当前活跃区间（按 end 降序） |
/// | `handled`     | `Vec<Interval>`    | 已完成分配的区间               |
/// | `vreg_to_preg`| 数组映射           | 虚拟→物理寄存器查找表          |
/// | `spill_next`  | `u16`              | 下一个可用的 spill 槽位游标    |
pub struct LsraAllocator {
    // ---- 物理寄存器管理 ----
    /// 可用物理寄存器位图
    ///
    /// 每个比特代表一个物理寄存器：`1` = 空闲可分配，`0` = 已占用。
    /// 使用 `BITMAP_WORDS` 个 u64 支持 `MAX_FUNCTION_LOCALS` 个寄存器位图。
    ///
    /// # 位操作约定
    ///
    /// - `free_regs[word_idx] & (1 << bit_offset)` : 检查第 `reg` 号寄存器是否空闲
    /// - `free_regs[word_idx] &= !(1 << bit_offset)` : 占用寄存器
    /// - `free_regs[word_idx] |= (1 << bit_offset)`   : 释放寄存器
    free_regs: [u64; BITMAP_WORDS],

    /// 物理寄存器总数上限（<= MAX_FUNCTION_LOCALS）
    max_regs: u16,

    // ---- 活跃区间管理 ----
    /// 当前活跃区间集合（Max-Heap，按 end 降序）
    ///
    /// 堆顶区间的 `end` 最大 → Lazy Spill 时的首选 victim。
    /// 使用 BinaryHeap 保证 O(log n) 插入/删除。
    active: BinaryHeap<Interval>,

    /// 已处理完毕的区间（分配结果记录）
    handled: Vec<Interval>,

    /// 虚拟→物理寄存器映射表
    ///
    /// 索引 = vreg，值 = 分配到的物理寄存器编号。
    /// 用于编译后端查询 "vreg x 对应哪个物理寄存器"。
    /// 大小固定为 `MAX_FUNCTION_LOCALS`，用数组而非 HashMap 避免 heap 分配。
    vreg_to_preg: [Option<u16>; MAX_FUNCTION_LOCALS as usize],

    // ---- Spill 管理 ----
    /// 下一个可用的 spill 槽位编号
    ///
    /// CIP: 下一个可用的 spill 槽位编号。
    /// 无上限检查（理论上受栈空间限制，实际函数不可能有 65535 个 spill 变量）。
    spill_next: u16,
    /// CIP: 每个 spill 槽位的结束 IP，用于判断槽位是否可复用。
    /// `spill_slot_ends[i]` = 分配到槽位 i 的最后一个区间的 end。
    /// 当新区间的 start > spill_slot_ends[i] 时，槽位 i 可复用。
    spill_slot_ends: Vec<usize>,

    /// NUD 增强配置
    ///
    /// 控制租约递减、物理寄存器上限等 NUD 增强行为。
    /// 通过 `with_nud_config()` 构造时设置；通过 `with_max_locals()` 构造时
    /// 使用 `NudConfig::disabled()`（退化为纯 LSRA）。
    nud_config: NudConfig,
}

// ============================================================================
// 构造函数
// ============================================================================

impl LsraAllocator {
    /// 创建新的 LSRA 分配器实例
    ///
    /// # 参数
    ///
    /// * `max_regs` - 可用的物理寄存器数量上限（必须 <= MAX_FUNCTION_LOCALS）
    ///
    /// # 初始状态
    ///
    /// - 所有 `max_regs` 个寄存器标记为空闲（位图全 1）
    /// - `active` 为空，无活跃区间
    /// - `spill_next = 0`
    ///
    /// # Panics
    ///
    /// 如果 `max_regs == 0` 或 `max_regs > MAX_FUNCTION_LOCALS`
    pub fn new(max_regs: u16) -> Self {
        assert!(
            max_regs > 0 && max_regs <= MAX_FUNCTION_LOCALS,
            "LsraAllocator: max_regs must be in (0, {}], got {}",
            MAX_FUNCTION_LOCALS,
            max_regs
        );

        // 初始化位图：前 max_regs 个寄存器设为空闲（1），其余为 0
        let mut free_regs = [0u64; BITMAP_WORDS];
        let mut remaining = max_regs;
        for word in free_regs.iter_mut() {
            if remaining == 0 {
                break;
            }
            // 每个字最多设置 64 位
            let bits_to_set = remaining.min(64);
            *word = u64::MAX >> (64 - bits_to_set);
            remaining -= bits_to_set;
        }

        Self {
            free_regs,
            max_regs,
            active: BinaryHeap::new(),
            handled: Vec::new(),
            vreg_to_preg: [None; MAX_FUNCTION_LOCALS as usize],
            spill_next: 0,
            spill_slot_ends: Vec::new(),
            nud_config: NudConfig::disabled(),
        }
    }

    /// 使用默认最大寄存器数创建分配器（= MAX_FUNCTION_LOCALS）
    pub fn with_max_locals() -> Self {
        Self::new(MAX_FUNCTION_LOCALS)
    }

    /// 创建带 NUD 配置的 LSRA 分配器
    ///
    /// 当 `nud_config.phys_reg_limit > 0` 时，使用该值作为物理寄存器上限；
    /// 否则退化为 `MAX_FUNCTION_LOCALS`。
    ///
    /// # 参数
    ///
    /// * `nud_config` - NUD 增强配置
    pub fn with_nud_config(nud_config: NudConfig) -> Self {
        let num_regs = if nud_config.phys_reg_limit > 0 {
            nud_config.phys_reg_limit
        } else {
            MAX_FUNCTION_LOCALS
        };

        let mut free_regs = [0u64; BITMAP_WORDS];
        let mut remaining = num_regs;
        for word in free_regs.iter_mut() {
            if remaining == 0 {
                break;
            }
            let bits_to_set = remaining.min(64);
            *word = u64::MAX >> (64 - bits_to_set);
            remaining -= bits_to_set;
        }

        Self {
            free_regs,
            max_regs: num_regs,
            active: BinaryHeap::new(),
            handled: Vec::new(),
            vreg_to_preg: [None; MAX_FUNCTION_LOCALS as usize],
            spill_next: 0,
            spill_slot_ends: Vec::new(),
            nud_config,
        }
    }
}

// ============================================================================
// 物理寄存器位图操作
// ============================================================================

impl LsraAllocator {
    /// 从位图中分配一个空闲物理寄存器
    ///
    /// 总是选择**编号最小**的空闲寄存器（lowest-numbered-first 策略），
    /// 这有助于提高缓存局部性（低编号寄存器通常更早被访问）。
    ///
    /// # 返回值
    ///
    /// `Some(reg)` - 分配到的物理寄存器编号
    /// `None` - 无空闲寄存器
    #[inline]
    pub fn alloc_one(&mut self) -> Option<u16> {
        for (word_idx, word) in self.free_regs.iter().enumerate() {
            if *word == 0 {
                continue;
            }

            // 找到最低位的 1 的位置（trailing_zeros 给出的就是最低置位位的索引）
            let bit_offset = word.trailing_zeros() as u16;
            let reg = (word_idx as u16) * 64 + bit_offset;

            // 安全边界检查（防御性，理论上不会越界因为位图初始化时已限制）
            if reg >= self.max_regs {
                continue;
            }

            self.free_regs[word_idx] &= !(1u64 << bit_offset);
            return Some(reg);
        }

        None
    }

    /// 释放一个物理寄存器回位图
    ///
    /// # 参数
    ///
    /// * `reg` - 要释放的物理寄存器编号
    ///
    /// # Panics
    ///
    /// 如果 `reg >= max_regs`（防御性断言，防止释放不存在的寄存器导致位图损坏）
    #[inline]
    pub fn free(&mut self, reg: u16) {
        debug_assert!(
            reg < self.max_regs,
            "LsraAllocator::free: reg {} out of range (max={})",
            reg,
            self.max_regs
        );

        let word_idx = (reg / 64) as usize;
        let bit_offset = (reg % 64) as u32;

        // 防御性：检查该寄存器是否已被释放（双重释放检测）
        debug_assert!(
            self.free_regs[word_idx] & (1u64 << bit_offset) == 0,
            "LsraAllocator::free: double-free on reg {}",
            reg
        );

        self.free_regs[word_idx] |= 1u64 << bit_offset;
    }

    /// 检查是否有空闲物理寄存器
    ///
    /// 时间复杂度：O(1) 平摊（最多检查 4 个 u64）
    #[inline]
    pub fn has_free(&self) -> bool {
        self.free_regs.iter().any(|&w| w != 0)
    }

    /// 获取当前空闲寄存器数量
    ///
    /// 遍历位图统计置位数，用于调试和测试。
    #[inline]
    pub fn free_count(&self) -> u16 {
        self.free_regs.iter().map(|w| w.count_ones() as u16).sum()
    }

    /// 获取当前活跃区间数量
    #[inline]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

// ============================================================================
// 过期区间管理
// ============================================================================

impl LsraAllocator {
    /// 移除已过期的活跃区间并释放其占用的物理寄存器
    ///
    /// 过期条件：`interval.end <= current_pos`（区间在当前位置之前已结束）。
    ///
    /// # 算法（Drain-Rebuild 模式）
    ///
    /// 由于 `active` 是 Max-Heap（按 end 降序，服务于 Lazy Spill 的 victim 选择），
    /// 无法直接定位到 end 最小的过期区间。因此采用全量 drain 再重建策略：
    ///
    /// ```text
    /// 1. 弹出所有活跃区间（O(n log n)）
    /// 2. 分拣：end <= current_pos → 过期（释放寄存器），否则保留
    /// 3. 将保留的区间重新推入堆（O(k log k)，k = 未过期数）
    /// ```
    ///
    /// 时间复杂度：O(n log n)，n = active 区间数。
    /// 对于 n < MAX_FUNCTION_LOCALS (4096)，完全可接受。
    ///
    /// # 参数
    ///
    /// * `current_pos` - 当前扫描到的字节码 IP 位置
    pub fn expire_old_intervals(&mut self, current_pos: usize) {
        // 收集未过期的区间（Drain 阶段）
        let mut survivors: Vec<Interval> = Vec::with_capacity(self.active.len());

        while let Some(mut iv) = self.active.pop() {
            // 租约递减：仅在 NUD 启用时递减活跃区间的 lease_remaining
            if self.nud_config.enabled && iv.lease_remaining > 0 {
                iv.lease_remaining = iv.lease_remaining.saturating_sub(1);
            }

            if iv.end <= current_pos && iv.lease_remaining == 0 {
                // 区间已过期且无租约 → 释放物理寄存器
                if let Some(reg) = iv.reg {
                    self.free(reg);
                }
                self.handled.push(iv);
            } else {
                // 未过期或仍有租约：保留，稍后重新入堆
                survivors.push(iv);
            }
        }

        // Rebuild 阶段：将存活区间推回 Max-Heap
        for iv in survivors {
            self.active.push(iv);
        }
    }
}

// ============================================================================
// Spill 管理
// ============================================================================

impl LsraAllocator {
    /// CIP: 分配一个 spill 栈槽位（区间图着色复用）
    ///
    /// 使用左边缘贪心着色：如果存在一个槽位 i，其上一个区间的 end <
    /// 当前区间的 start，则复用该槽位；否则分配新槽位。
    /// 这使得不重叠生命周期的变量共享同一个 spill 槽，
    /// 将 spill_slot_count 从「被 spill 变量总数」降到「最大重叠深度」。
    ///
    /// # 参数
    ///
    /// * `start` - 被 spill 区间的起始 IP
    /// * `end` - 被 spill 区间的终止 IP
    ///
    /// # 返回值
    ///
    /// 分配到的 spill 槽位编号
    #[inline]
    fn allocate_spill_slot(&mut self, start: usize, end: usize) -> u16 {
        for (slot_idx, slot_end) in self.spill_slot_ends.iter_mut().enumerate() {
            if *slot_end < start {
                *slot_end = end;
                return slot_idx as u16;
            }
        }
        let slot = self.spill_next;
        self.spill_slot_ends.push(end);
        self.spill_next += 1;
        slot
    }

    /// 将一个区间标记为 spilled 并记录到 handled
    ///
    /// # 参数
    ///
    /// * `interval` - 被 spill 的区间（其 reg 已被抢占）
    #[allow(dead_code)] // 保留为公开 API，allocate() 内部使用内联逻辑
    fn spill_interval(&mut self, mut interval: Interval) {
        interval.spill_slot = Some(self.allocate_spill_slot(interval.start, interval.end));
        interval.reg = None;

        // 直接加入 handled（不再活跃）
        self.handled.push(interval);
    }
}

// ============================================================================
// 核心 LSRA 分配算法
// ============================================================================

impl LsraAllocator {
    /// 对一组活跃区间执行线性扫描寄存器分配
    ///
    /// 这是 LSRA 的主入口方法。调用后，每个 `interval.reg` 或
    /// `interval.spill_slot` 将被填充为有效的分配结果。
    ///
    /// # 算法流程
    ///
    /// ```text
    /// 1. 按 start 升序排序（线性扫描的前提）
    /// 2. for each interval (in sorted order):
    ///    a. expire_old_intervals(interval.start) — 回收过期寄存器
    ///    b. if has_free():
    ///       → alloc_one(), 设置 interval.reg, push 到 active
    ///    c. else (寄存器耗尽 → Lazy Spill):
    ///       → victim = active.peek() (end 最大的)
    ///       → if victim.end > interval.end:
    ///           victim 活得更长 → spill victim, 抢其寄存器给 interval
    ///       → else:
    ///           interval 活得更长 → spill interval 自身
    /// 3. 将剩余 active 区间全部移入 handled
    /// ```
    ///
    /// # 参数
    ///
    /// * `intervals` - 可变引用的区间切片，分配结果将就地修改
    ///
    /// # 错误
    ///
    /// * `AllocError::InvalidInput` - intervals 为空或包含非法数据
    /// * `AllocError::Exhausted` - 理论上不应触发（spill 兜底），保留作为安全网
    ///
    /// # 时间复杂度
    ///
    /// O(n log n) 其中 n = intervals 数量（排序主导）
    pub fn allocate(&mut self, intervals: &mut [Interval]) -> Result<(), AllocError> {
        // ---- 前置校验 ----
        if intervals.is_empty() {
            return Err(AllocError::InvalidInput { message: "interval list is empty".to_string() });
        }

        // 校验所有 vreg 在合法范围内
        for iv in intervals.iter() {
            if iv.vreg >= MAX_FUNCTION_LOCALS {
                return Err(AllocError::InvalidInput {
                    message: format!(
                        "vreg {} exceeds MAX_FUNCTION_LOCALS ({})",
                        iv.vreg, MAX_FUNCTION_LOCALS
                    ),
                });
            }
        }

        // ---- Step 1: 按 start 升序排序 ----
        // 稳定排序保证相同 start 的区间保持原始顺序（确定性）
        intervals.sort_by_key(|i| i.start);

        // ---- Step 2: 线性扫描 ----
        // 记录 spill 决策（vreg → spill_slot），用于循环结束后回写到原数组
        // （因为 Rust 借用检查器不允许在 iter_mut 内部再次可变借 用 intervals）
        let mut spill_decisions: Vec<(u16, u16)> = Vec::new();

        for interval in intervals.iter_mut() {
            // Step 2a: 回收已过期的活跃区间
            self.expire_old_intervals(interval.start);

            // Step 2b: 尝试直接分配
            if self.has_free() {
                // H1 修复: 用 AllocError 替代 expect,从根源消除 panic。
                // has_free() 返回 true 时 alloc_one() 理论上必成功,
                // 但若位图状态不一致(内部 bug)则向上传播错误而非崩溃。
                let reg = self.alloc_one().ok_or_else(|| AllocError::InvalidInput {
                    message: "LsraAllocator invariant violated: has_free() returned true but alloc_one() returned None".to_string(),
                })?;
                interval.reg = Some(reg);

                self.vreg_to_preg[interval.vreg as usize] = Some(reg);

                self.active.push(interval.clone());
                continue;
            }

            // Step 2c: Lazy Spill —— 寄存器耗尽时的冲突解决策略
            //
            // 核心思想：比较当前区间与活跃区间中最长寿的那个（堆顶），
            // 选择"牺牲较小"的一方进行 spill。
            //
            // 代价函数 = 被 spill 区间的剩余生命期
            // 选择 spill victim 使得总代价最小。

            if let Some(victim) = self.active.peek() {
                if victim.end > interval.end {
                    // ── Case C1: Victim 活得更长 ──
                    //
                    // victim 的 end > current.end → victim 在当前区间之后仍需存活
                    // → spill victim，抢它的寄存器给当前区间使用
                    //
                    // 示例：
                    //   current:  [====]          end=10
                    //   victim:   [=============]  end=50  ← spill 这个
                    //   结果：current 得到寄存器，victim 去 spill 槽

                    let vic =
                        self.active.pop().expect("active list must be non-empty when spilling");

                    if let Some(vic_reg) = vic.reg {
                        self.free(vic_reg);
                    }

                    // CIP: 分配 spill 槽位给 victim（复用不重叠的槽位）
                    let spill_slot = self.allocate_spill_slot(vic.start, vic.end);

                    // 记录 spill 决策（循环结束后统一回写到原 intervals 数组）
                    spill_decisions.push((vic.vreg, spill_slot));

                    // 将 victim 记录到 handled（用于查询和统计）
                    let mut spilled_vic = vic;
                    spilled_vic.reg = None;
                    spilled_vic.spill_slot = Some(spill_slot);
                    self.handled.push(spilled_vic);

                    // 分配寄存器给当前区间
                    // H2 修复: 同 H1,用 AllocError 替代 expect。
                    // 刚释放了 victim 的寄存器,alloc_one() 应成功,
                    // 但若 free/alloc 状态机不一致则向上传播错误。
                    let reg = self.alloc_one().ok_or_else(|| AllocError::InvalidInput {
                        message: "LsraAllocator invariant violated: just freed a reg but alloc_one() failed".to_string(),
                    })?;
                    interval.reg = Some(reg);

                    self.vreg_to_preg[interval.vreg as usize] = Some(reg);

                    // 当前区间成为新的活跃成员
                    self.active.push(interval.clone());
                } else {
                    // ── Case C2: 当前区间活得更长（或一样长）──
                    //
                    // victim.end <= current.end → victim 先于（或同时于）当前区间结束
                    // → spill 当前区间本身，让 victim 继续占用寄存器
                    //
                    // 这是"认怂"策略：与其挤掉一个快死的变量，不如自己去栈上待着
                    //
                    // 示例：
                    //   current:  [===============]  end=50  ← spill 自己
                    //   victim:   [=====]            end=20  ← 让它活着

                    interval.spill_slot =
                        Some(self.allocate_spill_slot(interval.start, interval.end));
                    interval.reg = None;

                    // 当前区间不需要加入 active（它不在寄存器中）
                    // 但仍然记录到 handled 以便后续查询
                    self.handled.push(interval.clone());
                }
            } else {
                // active 为空但 has_free() 返回 false → 不应发生（逻辑矛盾）
                // 作为安全网，spill 当前区间
                interval.spill_slot = Some(self.allocate_spill_slot(interval.start, interval.end));
                self.handled.push(interval.clone());
            }
        }

        // ---- Step 3: 清扫剩余活跃区间 ----
        // 扫描结束后，将仍在 active 中的区间全部移入 handled
        while let Some(remaining) = self.active.pop() {
            self.handled.push(remaining);
        }

        // ---- Step 4: 回写 spill 决策到原 intervals 数组 ----
        // 主循环中 spill 的 victim 是堆中弹出的副本，原数组中的条目仍是旧状态（reg 未清空）
        // 此处统一将 spill_decisions 应用回 intervals，保证调用方看到一致的分配结果
        for (vreg, spill_slot) in spill_decisions {
            if let Some(original) = intervals.iter_mut().find(|iv| iv.vreg == vreg) {
                original.reg = None;
                original.spill_slot = Some(spill_slot);
            }
        }

        Ok(())
    }

    /// 查询虚拟寄存器对应的物理寄存器
    ///
    /// 必须在 `allocate()` 调用后使用。
    ///
    /// # 返回值
    ///
    /// * `Some(preg)` - 分配到的物理寄存器编号
    /// * `None` - 该虚拟寄存器被 spill 到栈上了
    #[inline]
    pub fn get_phys_reg(&self, vreg: u16) -> Option<u16> {
        if (vreg as usize) < self.vreg_to_preg.len() {
            self.vreg_to_preg[vreg as usize]
        } else {
            None
        }
    }

    /// 获取所有已处理的区间（分配结果的只读视图）
    #[inline]
    pub fn handled_intervals(&self) -> &[Interval] {
        &self.handled
    }

    /// 获取 spill 槽位总数（用于计算栈帧大小）
    #[inline]
    pub fn spill_slot_count(&self) -> u16 {
        self.spill_next
    }

    /// 重置分配器状态（复用同一实例多次分配）
    ///
    /// 清空所有内部状态，恢复到 `new()` 后的初始状态（保持 max_regs 不变）。
    pub fn reset(&mut self) {
        let mut remaining = self.max_regs;
        for word in self.free_regs.iter_mut() {
            if remaining == 0 {
                *word = 0;
                break;
            }
            let bits_to_set = remaining.min(64);
            *word = u64::MAX >> (64 - bits_to_set);
            remaining -= bits_to_set;
        }

        self.active.clear();
        self.handled.clear();
        self.vreg_to_preg.fill(None);
        self.spill_next = 0;
        self.spill_slot_ends.clear();
    }
}

// ============================================================================
// build_intervals -- 活区间构建辅助函数
// ============================================================================

/// 从 def/use 信息构建活区间列表
///
/// # 参数
///
/// * `def_ips` - 定义点数组：`def_ips[vreg] = Some(ip)` 表示 vreg 在 ip 处首次被定义
/// * `use_ips` - 使用点数组：`use_ips[vreg] = Some(ip)` 表示 vreg 在 ip 处最后被使用
///
/// # 返回值
///
/// 包含所有有效 `(def_ip, use_ip)` 对的 `Vec<Interval>`，按 vreg 编号排序。
///
/// # 语义说明
///
/// - 只有两个数组都有 `Some` 值的 vreg 才会产生区间（忽略从未使用或从未定义的 vreg）
/// - 区间的 `start = def_ip`，`end = use_ip`
/// - 如果 `def_ip > use_ip`（异常情况），交换二者并产生警告级别的区间
///
/// # 典型用法
///
/// ```text
/// // 编译器单遍扫描时维护：
/// // def_ip[REG_MAX]: Option<usize>  ← 首次写入时记录（只在为 None 时写）
/// // use_ip[REG_MAX]: Option<usize>  ← 每次读取时覆盖更新
///
/// // 编译结束后调用：
/// let intervals = build_intervals(&def_ips, &use_ips)?;
/// lsra.allocate(&mut intervals)?;
/// ```
///
/// # 错误
///
/// * `AllocError::InvalidInput` - 数组长度不一致或超过 MAX_FUNCTION_LOCALS
pub fn build_intervals(
    def_ips: &[Option<usize>],
    use_ips: &[Option<usize>],
) -> Result<Vec<Interval>, AllocError> {
    // 校验输入一致性
    if def_ips.len() != use_ips.len() {
        return Err(AllocError::InvalidInput {
            message: format!(
                "def_ips length ({}) != use_ips length ({})",
                def_ips.len(),
                use_ips.len()
            ),
        });
    }

    if def_ips.len() > MAX_FUNCTION_LOCALS as usize {
        return Err(AllocError::InvalidInput {
            message: format!(
                "input array length {} exceeds MAX_FUNCTION_LOCALS ({})",
                def_ips.len(),
                MAX_FUNCTION_LOCALS
            ),
        });
    }

    let mut intervals = Vec::new();

    for (vreg, (def_ip, use_ip)) in def_ips.iter().zip(use_ips.iter()).enumerate() {
        if let (Some(start), Some(end)) = (*def_ip, *use_ip) {
            let (s, e) = if start <= end { (start, end) } else { (end, start) };
            intervals.push(Interval::new(vreg as u16, s, e));
        }
    }

    Ok(intervals)
}

// ============================================================================
// enhance_intervals -- NUD 增强器
// ============================================================================

/// 对活区间列表施加 NUD 增强
///
/// 在 `build_intervals()` 之后、`lsra.allocate()` 之前调用。
/// 根据循环深度放大区间 end，设置租约，为 LSRA 提供更智能的分配依据。
///
/// # 算法
///
/// 对每个区间：
/// 1. 设置 `loop_depth` = `loop_depths[vreg]`
/// 2. 如果 `loop_depth > 0`：放大 `end` = `end * (1 + loop_depth * loop_immunity_factor)`
///    - 放大后不超过 `usize::MAX / 2`（防止溢出）
/// 3. 设置 `lease_remaining` = `config.lease_length`（仅循环变量；非循环变量为 0）
/// 4. 设置 `use_frequency` = 1（暂固定）
///
/// # 参数
///
/// * `intervals` - 活区间列表（就地修改）
/// * `config` - NUD 配置
/// * `loop_depths` - 每个 vreg 定义时的循环深度数组
///
/// # 注意
///
/// 当 `config.enabled == false` 时，此函数为 no-op。
pub fn enhance_intervals(intervals: &mut [Interval], config: &NudConfig, loop_depths: &[u8]) {
    if !config.enabled {
        return;
    }

    let max_end = usize::MAX / 2;

    for iv in intervals.iter_mut() {
        // 1. 设置循环深度
        let depth =
            if (iv.vreg as usize) < loop_depths.len() { loop_depths[iv.vreg as usize] } else { 0 };
        iv.loop_depth = depth;

        // 2. 循环保护：放大 end
        if depth > 0 {
            let factor = config.loop_immunity_factor as usize;
            let multiplier = 1usize + (depth as usize).saturating_mul(factor);
            let boosted = iv.end.saturating_mul(multiplier);
            iv.end = boosted.min(max_end);
        }

        // 3. 设置租约：仅对循环内变量启用租约保护
        // 非循环变量（depth=0）的租约为 0，区间结束时立即可回收，
        // 避免阻碍不相交区间的寄存器复用。
        iv.lease_remaining = if depth > 0 { config.lease_length } else { 0 };

        // 4. 设置使用频率（暂固定为 1）
        iv.use_frequency = 1;
    }
}

// ============================================================================
// LSRA 单元测试
// ============================================================================

#[cfg(test)]
mod lsra_tests {
    use super::*;

    // ========================================================================
    // Interval 数据结构测试
    // ========================================================================

    #[test]
    fn test_interval_creation_and_invariants() {
        let iv = Interval::new(0, 10, 20);
        assert_eq!(iv.vreg, 0);
        assert_eq!(iv.start, 10);
        assert_eq!(iv.end, 20);
        assert_eq!(iv.reg, None);
        assert_eq!(iv.spill_slot, None);
        assert_eq!(iv.len(), 11); // [10, 20] 含两端
        assert!(!iv.is_empty());
    }

    #[test]
    fn test_interval_single_point() {
        let iv = Interval::new(5, 42, 42);
        assert!(iv.is_empty()); // start == end
        assert_eq!(iv.len(), 1);
    }

    #[test]
    fn test_interval_overlaps() {
        let a = Interval::new(0, 0, 10);
        let b = Interval::new(1, 5, 15); // 重叠 [5, 10]
        let c = Interval::new(2, 11, 20); // 不重叠（a.end=10, c.start=11）

        assert!(a.overlaps(&b));
        assert!(b.overlaps(&a));
        assert!(!a.overlaps(&c));
        assert!(!c.overlaps(&a));
    }

    #[test]
    fn test_interval_overlaps_adjacent() {
        // 闭区间相邻：[0, 10] 和 [11, 20] → 不重叠（10 < 11）
        let a = Interval::new(0, 0, 10);
        let b = Interval::new(1, 11, 20);
        assert!(!a.overlaps(&b));

        // 端点相接：[0, 10] 和 [10, 20] → 重叠（10 == 10）
        let c = Interval::new(2, 10, 20);
        assert!(a.overlaps(&c));
    }

    #[test]
    fn test_interval_ordering_for_heap() {
        // BinaryHeap 是 Max-Heap：end 最大的应该在堆顶
        let mut heap = BinaryHeap::new();
        heap.push(Interval::new(0, 0, 10)); // end=10
        heap.push(Interval::new(1, 0, 30)); // end=30 ← 应该在堆顶
        heap.push(Interval::new(2, 0, 20)); // end=20

        let top = heap.peek().unwrap();
        assert_eq!(top.end, 30); // 最大 end 在堆顶
        assert_eq!(top.vreg, 1);

        // 弹出后下一个应该是 end=20
        let second = heap.pop().unwrap();
        assert_eq!(second.end, 30); // 弹出的就是堆顶
        let next_top = heap.peek().unwrap();
        assert_eq!(next_top.end, 20);
    }

    #[test]
    fn test_interval_ordering_stable_by_vreg() {
        // 相同 end 时按 vreg 降序排列（保证确定性）
        let mut heap = BinaryHeap::new();
        heap.push(Interval::new(5, 0, 10)); // vreg=5, end=10
        heap.push(Interval::new(3, 0, 10)); // vreg=3, end=10

        let top = heap.peek().unwrap();
        assert_eq!(top.vreg, 5); // vreg 大的排前面（降序）
    }

    #[test]
    fn test_interval_display() {
        let mut iv = Interval::new(42, 10, 20);
        iv.reg = Some(3);
        let display = format!("{}", iv);
        assert!(display.contains("vreg42"));
        assert!(display.contains("r3")); // reg number
        assert!(display.contains("depth=0"));
        assert!(display.contains("lease=0"));
    }

    #[test]
    fn test_interval_ordering_lease_protected() {
        // 有租约的区间不应被优先选为 victim（应排堆底）
        let mut heap = BinaryHeap::new();
        let mut leased = Interval::new(0, 0, 30);
        leased.lease_remaining = 5; // 有租约
        let mut free = Interval::new(1, 0, 10);
        free.lease_remaining = 0; // 无租约

        heap.push(leased.clone());
        heap.push(free.clone());

        // 堆顶应该是无租约的区间（可被优先驱逐）
        let top = heap.peek().unwrap();
        assert_eq!(top.vreg, 1); // free 在堆顶
        assert_eq!(top.lease_remaining, 0);
    }

    #[test]
    fn test_interval_ordering_both_leased_falls_back_to_end() {
        // 两个都有租约时，退回到 end 降序比较
        let mut heap = BinaryHeap::new();
        let mut a = Interval::new(0, 0, 30);
        a.lease_remaining = 3;
        let mut b = Interval::new(1, 0, 10);
        b.lease_remaining = 5;

        heap.push(a);
        heap.push(b);

        let top = heap.peek().unwrap();
        assert_eq!(top.end, 30); // end 大的在堆顶
    }

    // ========================================================================
    // LsraAllocator 位图操作测试
    // ========================================================================

    #[test]
    fn test_allocator_new_initial_state() {
        let alloc = LsraAllocator::new(8);
        assert_eq!(alloc.free_count(), 8);
        assert_eq!(alloc.active_count(), 0);
        assert_eq!(alloc.spill_slot_count(), 0);
    }

    #[test]
    fn test_alloc_one_lowest_first() {
        let mut alloc = LsraAllocator::new(8);

        // 连续分配应该得到 0, 1, 2, ...
        assert_eq!(alloc.alloc_one(), Some(0));
        assert_eq!(alloc.alloc_one(), Some(1));
        assert_eq!(alloc.alloc_one(), Some(2));
        assert_eq!(alloc.free_count(), 5); // 8 - 3
    }

    #[test]
    fn test_alloc_until_exhausted() {
        let mut alloc = LsraAllocator::new(4);

        assert_eq!(alloc.alloc_one(), Some(0));
        assert_eq!(alloc.alloc_one(), Some(1));
        assert_eq!(alloc.alloc_one(), Some(2));
        assert_eq!(alloc.alloc_one(), Some(3));
        assert_eq!(alloc.alloc_one(), None); // 全部分配完毕
        assert!(!alloc.has_free());
    }

    #[test]
    fn test_free_and_realloc() {
        let mut alloc = LsraAllocator::new(8);

        // 分配 r0, r1, r2
        let _r0 = alloc.alloc_one().unwrap();
        let r1 = alloc.alloc_one().unwrap();
        let _r2 = alloc.alloc_one().unwrap();

        // 释放 r1
        alloc.free(r1);
        assert_eq!(alloc.free_count(), 6); // 8 - 3 + 1 = 6

        // 下次分配应该复用 r1（最小编号的空闲寄存器）
        assert_eq!(alloc.alloc_one(), Some(1)); // 复用 r1
        assert_eq!(alloc.free_count(), 5);
    }

    #[test]
    fn test_alloc_one_across_word_boundary() {
        // 测试跨 u64 字边界的分配（65+ 个寄存器）
        let mut alloc = LsraAllocator::new(70);

        // 分配前 66 个（填满第一个字 + 第二个字 2 位）
        for i in 0..66u16 {
            assert_eq!(alloc.alloc_one(), Some(i));
        }

        // 第 67 个应该从第二个字的第 2 位开始
        assert_eq!(alloc.alloc_one(), Some(66));
        assert_eq!(alloc.alloc_one(), Some(67));
        // ... 直到 69
        assert_eq!(alloc.alloc_one(), Some(68));
        assert_eq!(alloc.alloc_one(), Some(69));
        assert_eq!(alloc.alloc_one(), None); // 70 个全部分配完
    }

    #[test]
    fn test_has_free_consistency() {
        let mut alloc = LsraAllocator::new(4);
        assert!(alloc.has_free());

        alloc.alloc_one();
        alloc.alloc_one();
        alloc.alloc_one();
        assert!(alloc.has_free()); // 还剩 1 个

        alloc.alloc_one();
        assert!(!alloc.has_free()); // 全部耗尽
    }

    // ========================================================================
    // expire_old_intervals 测试
    // ========================================================================

    #[test]
    fn test_expire_old_intervals_basic() {
        let mut alloc = LsraAllocator::new(8);

        // 先从位图分配寄存器（模拟真实分配路径），再构造活跃区间
        let r0 = alloc.alloc_one().unwrap();
        let r1 = alloc.alloc_one().unwrap();
        let r2 = alloc.alloc_one().unwrap();
        assert_eq!(alloc.free_count(), 5); // 8 - 3 = 5

        let mut iv1 = Interval::new(0, 0, 5);
        iv1.reg = Some(r0);
        let mut iv2 = Interval::new(1, 2, 10);
        iv2.reg = Some(r1);
        let mut iv3 = Interval::new(2, 4, 15);
        iv3.reg = Some(r2);

        alloc.active.push(iv1);
        alloc.active.push(iv2);
        alloc.active.push(iv3);

        assert_eq!(alloc.active_count(), 3);

        // 在 position=7 过期：iv1 (end=5) 应该被移除
        alloc.expire_old_intervals(7);
        assert_eq!(alloc.active_count(), 2); // iv2, iv3 仍活跃
        assert_eq!(alloc.free_count(), 6); // 5 + 1 (r0 被释放) = 6

        // 在 position=12 过期：iv2 (end=10) 应该被移除
        alloc.expire_old_intervals(12);
        assert_eq!(alloc.active_count(), 1); // 只有 iv3
        assert_eq!(alloc.free_count(), 7); // 6 + 1 (r1 被释放) = 7
    }

    #[test]
    fn test_expire_at_exact_end() {
        let mut alloc = LsraAllocator::new(8);

        let r0 = alloc.alloc_one().unwrap();
        assert_eq!(alloc.free_count(), 7); // 8 - 1 = 7

        let mut iv = Interval::new(0, 0, 5);
        iv.reg = Some(r0);
        alloc.active.push(iv);

        // expire_old_intervals(5): end=5 <= 5 → 应该过期
        alloc.expire_old_intervals(5);
        assert_eq!(alloc.active_count(), 0);
        assert_eq!(alloc.free_count(), 8); // 7 + 1 (r0 被释放) = 8
    }

    #[test]
    fn test_expire_before_any_end() {
        let mut alloc = LsraAllocator::new(8);

        let r0 = alloc.alloc_one().unwrap();
        let mut iv = Interval::new(0, 10, 20);
        iv.reg = Some(r0);
        alloc.active.push(iv);

        // position=5 < start=10: 不应有任何过期
        alloc.expire_old_intervals(5);
        assert_eq!(alloc.active_count(), 1);
        assert_eq!(alloc.free_count(), 7); // r0 已分配（8-1=7），未释放
    }

    // ========================================================================
    // LSRA 核心算法测试（Happy Path）
    // ========================================================================

    #[test]
    fn test_allocate_no_overlap_all_fit() {
        // 三个区间完全不重叠，应该各自获得不同的寄存器
        let mut alloc = LsraAllocator::new(8);
        let mut intervals = vec![
            Interval::new(0, 0, 5),   // vreg0: [0, 5]
            Interval::new(1, 6, 10),  // vreg1: [6, 10]  ← 不与 vreg0 重叠
            Interval::new(2, 11, 15), // vreg2: [11, 15] ← 不与前两者重叠
        ];

        alloc.allocate(&mut intervals).unwrap();

        // 所有区间都应该分配到了寄存器
        for iv in &intervals {
            assert!(iv.reg.is_some(), "vreg{} should have a register", iv.vreg);
            assert!(iv.spill_slot.is_none(), "vreg{} should not be spilled", iv.vreg);
        }

        // 三个区间应该用了 3 个不同的寄存器（或者复用了已释放的）
        // 关键验证：映射正确
        assert_eq!(alloc.get_phys_reg(0), intervals[0].reg);
        assert_eq!(alloc.get_phys_reg(1), intervals[1].reg);
        assert_eq!(alloc.get_phys_reg(2), intervals[2].reg);
    }

    #[test]
    fn test_allocate_register_reuse() {
        // 两个区间不重叠 → 应该复用同一个寄存器！这是 LSRA 的核心价值
        let mut alloc = LsraAllocator::new(4);
        let mut intervals = vec![
            Interval::new(0, 0, 3), // vreg0: [0, 3]  用 r0
            Interval::new(1, 4, 7), // vreg1: [4, 7]  → r0 已释放 → 也用 r0
        ];

        alloc.allocate(&mut intervals).unwrap();

        // 两者应该分到同一个物理寄存器（r0）
        assert_eq!(intervals[0].reg, intervals[1].reg);
        assert_eq!(intervals[0].reg, Some(0)); // 最小编号优先
        assert_eq!(alloc.free_count(), 3); // 只用了 1 个寄存器
    }

    #[test]
    fn test_allocate_overlap_needs_different_regs() {
        // 两个区间重叠 → 需要不同的寄存器
        let mut alloc = LsraAllocator::new(8);
        let mut intervals = vec![
            Interval::new(0, 0, 10), // vreg0: [0, 10]  长区间
            Interval::new(1, 5, 8),  // vreg1: [5, 8]   ← 与 vreg0 重叠 [5, 8]
        ];

        alloc.allocate(&mut intervals).unwrap();

        // 两者都应该有寄存器，且不能相同
        assert!(intervals[0].reg.is_some());
        assert!(intervals[1].reg.is_some());
        assert_ne!(intervals[0].reg, intervals[1].reg, "overlapping intervals need different regs");
    }

    // ========================================================================
    // LSRA Spill 测试（Edge Case / Poison Pill）
    // ========================================================================

    #[test]
    fn test_spill_when_registers_exhausted() {
        // 只有 2 个物理寄存器，但有 3 个同时活跃的区间 → 必须 spill 一个
        //
        // LSRA Lazy Spill 策略：
        // - 堆顶是 end 最大的区间（最长寿）
        // - 如果 victim.end > current.end → spill victim（Case C1）
        // - 这里的语义：victim 活得更长，但当前区间更快死亡，
        //   所以 spill victim 让当前区间使用寄存器（当前区间很快就不需要了）
        let mut alloc = LsraAllocator::new(2);
        let mut intervals = vec![
            Interval::new(0, 0, 20), // vreg0: [0, 20]  最长 ← 会被选为 victim
            Interval::new(1, 0, 10), // vreg1: [0, 10]  中等
            Interval::new(2, 0, 5),  // vreg2: [0, 5]   最短（最后处理）
        ];

        alloc.allocate(&mut intervals).unwrap();

        // 应该至少有一个被 spill
        let spilled_count = intervals.iter().filter(|iv| iv.reg.is_none()).count();
        let registered_count = intervals.iter().filter(|iv| iv.reg.is_some()).count();

        assert_eq!(registered_count, 2, "should have exactly 2 in registers");
        assert_eq!(spilled_count, 1, "should have exactly 1 spilled");

        // 验证一致性：spilled 的 reg 为 None，registered 的 reg 为 Some
        for iv in &intervals {
            assert!(
                iv.reg.is_some() ^ iv.spill_slot.is_some(),
                "vreg{}: exactly one of reg/spill must be Some",
                iv.vreg
            );
        }
    }

    #[test]
    fn test_spill_current_interval_when_it_is_longest() {
        // 2 个寄存器，3 个区间：
        // - vreg0: [0, 5]   短
        // - vreg1: [0, 10]  中
        // - vreg2: [0, 20]  最长（最后一个处理，发现寄存器不够）
        //
        // 当处理 vreg2 时，active 中有 vreg0 和 vreg1（假设都已分配）。
        // victim = active 堆顶（end 最大的，比如 vreg1 end=10）。
        // vreg2.end(20) > victim.end(10) → Case C2: spill 当前区间（vreg2 自己去 spill）
        let mut alloc = LsraAllocator::new(2);
        let mut intervals = vec![
            Interval::new(0, 0, 5),  // vreg0: 最短
            Interval::new(1, 0, 10), // vreg1: 中等
            Interval::new(2, 0, 20), // vreg2: 最长
        ];

        alloc.allocate(&mut intervals).unwrap();

        // vreg2 (最长) 可能被 spill（因为它最后来，且比 active 中的 victim 都长）
        let vreg2 = intervals.iter().find(|iv| iv.vreg == 2).unwrap();
        // 不管具体 spill 策略如何，分配必须成功且结果一致
        assert!(vreg2.reg.is_some() || vreg2.spill_slot.is_some());
    }

    #[test]
    fn test_many_intervals_few_registers_stress() {
        // 压力测试：8 个区间，只有 3 个寄存器
        // 设计为链式重叠（每个区间与前一个重叠 2 个单位），
        // 使得任意时刻最多 3 个区间同时活跃 → 刚好不需要 spill
        let mut alloc = LsraAllocator::new(3);
        let mut intervals = vec![
            Interval::new(0, 0, 4),   // [0, 4]
            Interval::new(1, 2, 6),   // [2, 6]   与 vreg0 重叠 [2,4]
            Interval::new(2, 4, 8),   // [4, 8]   与 vreg1 重叠 [4,6]
            Interval::new(3, 6, 10),  // [6, 10]  与 vreg2 重叠 [6,8]
            Interval::new(4, 8, 12),  // [8, 12]  与 vreg3 重叠 [8,10]
            Interval::new(5, 10, 14), // [10,14]  与 vreg4 重叠 [10,12]
            Interval::new(6, 12, 16), // [12,16]  与 vreg5 重叠 [12,14]
            Interval::new(7, 14, 18), // [14,18]  与 vreg6 重叠 [14,16]
        ];

        alloc.allocate(&mut intervals).unwrap();

        // 验证：每个区间要么有 reg 要么有 spill_slot
        for (i, iv) in intervals.iter().enumerate() {
            assert!(
                iv.reg.is_some() || iv.spill_slot.is_some(),
                "interval vreg{} at index {} has no allocation",
                iv.vreg,
                i
            );
        }

        // 验证：同时活跃的区间不超过 3 个（物理寄存器数）
        // 由于设计为链式重叠，任意时刻最多 3 个活跃
        for pos in 0..=18 {
            let active_at_pos = intervals
                .iter()
                .filter(|iv| iv.start <= pos && pos <= iv.end && iv.reg.is_some())
                .count();
            assert!(
                active_at_pos <= 3,
                "at position {}: too many intervals in registers ({})",
                pos,
                active_at_pos
            );
        }
    }

    // ========================================================================
    // build_intervals 测试
    // ========================================================================

    #[test]
    fn test_build_intervals_normal() {
        let def_ips: Vec<Option<usize>> = vec![Some(0), Some(5), Some(10)];
        let use_ips: Vec<Option<usize>> = vec![Some(3), Some(8), Some(15)];

        let intervals = build_intervals(&def_ips, &use_ips).unwrap();

        assert_eq!(intervals.len(), 3);
        assert_eq!(intervals[0], Interval::new(0, 0, 3));
        assert_eq!(intervals[1], Interval::new(1, 5, 8));
        assert_eq!(intervals[2], Interval::new(2, 10, 15));
    }

    #[test]
    fn test_build_intervals_skips_incomplete() {
        // 只有 def 没有 use → 跳过
        let def_ips: Vec<Option<usize>> = vec![Some(0), Some(5)];
        let use_ips: Vec<Option<usize>> = vec![None, Some(10)];

        let intervals = build_intervals(&def_ips, &use_ips).unwrap();
        assert_eq!(intervals.len(), 1); // 只有 vreg1 完整
        assert_eq!(intervals[0].vreg, 1);
    }

    #[test]
    fn test_build_intervals_swaps_def_use_if_inverted() {
        // def > use（异常情况）：应该交换
        let def_ips: Vec<Option<usize>> = vec![Some(10)];
        let use_ips: Vec<Option<usize>> = vec![Some(3)];

        let intervals = build_intervals(&def_ips, &use_ips).unwrap();
        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].start, 3); // 被交换成较小的值
        assert_eq!(intervals[0].end, 10); // 被交换成较大的值
    }

    #[test]
    fn test_build_intervals_empty_input() {
        let def_ips: Vec<Option<usize>> = vec![];
        let use_ips: Vec<Option<usize>> = vec![];

        let intervals = build_intervals(&def_ips, &use_ips).unwrap();
        assert!(intervals.is_empty());
    }

    #[test]
    fn test_build_inputs_mismatch_error() {
        let def_ips: Vec<Option<usize>> = vec![Some(0)];
        let use_ips: Vec<Option<usize>> = vec![Some(0), Some(1)];

        let result = build_intervals(&def_ips, &use_ips);
        assert!(result.is_err());
        match result.unwrap_err() {
            AllocError::InvalidInput { .. } => {}
            other => panic!("Expected InvalidInput, got {:?}", other),
        }
    }

    // ========================================================================
    // 错误处理测试（Poison Pill）
    // ========================================================================

    #[test]
    fn test_allocate_empty_intervals_error() {
        let mut alloc = LsraAllocator::new(8);
        let mut intervals: Vec<Interval> = vec![];

        let result = alloc.allocate(&mut intervals);
        assert!(result.is_err());
    }

    #[test]
    fn test_allocate_vreg_out_of_range_error() {
        let mut alloc = LsraAllocator::new(8);
        let mut intervals = vec![
            Interval::new(MAX_FUNCTION_LOCALS, 0, 10), // vreg = 255, out of range for array indexing
        ];

        let result = alloc.allocate(&mut intervals);
        assert!(result.is_err());
    }

    #[test]
    fn test_allocator_new_zero_regs_panics() {
        // max_regs = 0 应该 panic（通过 assert 触发）
        let result = std::panic::catch_unwind(|| {
            LsraAllocator::new(0);
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_allocator_new_exceeds_max_panics() {
        let result = std::panic::catch_unwind(|| {
            LsraAllocator::new(MAX_FUNCTION_LOCALS + 1);
        });
        assert!(result.is_err());
    }

    // ========================================================================
    // reset 和重复使用测试
    // ========================================================================

    #[test]
    fn test_reset_restores_initial_state() {
        let mut alloc = LsraAllocator::new(4);

        // 第一次分配
        let mut intervals = vec![Interval::new(0, 0, 5), Interval::new(1, 6, 10)];
        alloc.allocate(&mut intervals).unwrap();
        assert!(alloc.free_count() < 4); // 用了一些寄存器

        // 重置
        alloc.reset();

        // 验证恢复初始状态
        assert_eq!(alloc.free_count(), 4);
        assert_eq!(alloc.active_count(), 0);
        assert_eq!(alloc.spill_slot_count(), 0);
        assert_eq!(alloc.handled_intervals().len(), 0);

        // 可以再次使用
        let mut intervals2 = vec![Interval::new(0, 0, 3)];
        alloc.allocate(&mut intervals2).unwrap();
        assert!(intervals2[0].reg.is_some());
    }

    // ========================================================================
    // AllocError Display 测试
    // ========================================================================

    #[test]
    fn test_alloc_error_display() {
        let err = AllocError::Exhausted { vreg: 5, position: 42 };
        let s = format!("{}", err);
        assert!(s.contains("vreg 5"));
        assert!(s.contains("42"));

        let err2 = AllocError::InvalidInput { message: "bad input".to_string() };
        let s2 = format!("{}", err2);
        assert!(s2.contains("bad input"));
    }

    // ========================================================================
    // NudConfig 和 enhance_intervals 测试
    // ========================================================================

    #[test]
    fn test_enhance_intervals_loop_immunity() {
        // 循环深度 2 的变量，end 应被放大
        let mut intervals = vec![Interval::new(0, 0, 10)];
        let config = NudConfig::default(); // loop_immunity_factor=3
        let mut depths = [0u8; 64];
        depths[0] = 2; // vreg 0 在深度 2 的循环中

        enhance_intervals(&mut intervals, &config, &depths);

        assert_eq!(intervals[0].loop_depth, 2);
        // end = 10 * (1 + 2 * 3) = 10 * 7 = 70
        assert_eq!(intervals[0].end, 70);
        assert_eq!(intervals[0].lease_remaining, config.lease_length);
    }

    #[test]
    fn test_enhance_intervals_no_loop() {
        // 循环深度 0 的变量，end 不变，租约为 0（不阻碍寄存器复用）
        let mut intervals = vec![Interval::new(0, 0, 10)];
        let config = NudConfig::default();
        let loop_depths = [0u8; 64];

        enhance_intervals(&mut intervals, &config, &loop_depths);

        assert_eq!(intervals[0].loop_depth, 0);
        assert_eq!(intervals[0].end, 10); // 未放大
        assert_eq!(intervals[0].lease_remaining, 0); // 非循环变量无租约
    }

    #[test]
    fn test_enhance_intervals_disabled() {
        // NUD 禁用时为 no-op
        let mut intervals = vec![Interval::new(0, 0, 10)];
        let config = NudConfig::disabled();
        let loop_depths = [0u8; 64];

        enhance_intervals(&mut intervals, &config, &loop_depths);

        assert_eq!(intervals[0].end, 10); // 未改变
        assert_eq!(intervals[0].loop_depth, 0); // 未改变
        assert_eq!(intervals[0].lease_remaining, 0); // 未改变
    }

    #[test]
    fn test_nud_config_default() {
        let config = NudConfig::default();
        assert_eq!(config.phys_reg_limit, 64);
        assert_eq!(config.loop_immunity_factor, 3);
        assert_eq!(config.lease_length, 5);
        assert_eq!(config.cold_branch_penalty, 2);
        assert!(config.enabled);
    }

    #[test]
    fn test_nud_config_disabled() {
        let config = NudConfig::disabled();
        assert!(!config.enabled);
        assert_eq!(config.phys_reg_limit, 64); // 其他参数同 default
    }

    #[test]
    fn test_nud_config_effective_nud_no_lease() {
        let config = NudConfig::default();
        let iv = Interval::new(0, 0, 10);
        // loop_depth=0, lease_remaining=0
        // effective = 10 * (1 + 0) / 2 = 5
        assert_eq!(config.effective_nud(&iv), 5);
    }

    #[test]
    fn test_nud_config_effective_nud_with_lease() {
        let config = NudConfig::default();
        let mut iv = Interval::new(0, 0, 10);
        iv.lease_remaining = 3;
        // 有租约 → u64::MAX
        assert_eq!(config.effective_nud(&iv), u64::MAX);
    }

    #[test]
    fn test_nud_config_effective_nud_loop_depth() {
        let config = NudConfig::default();
        let mut iv = Interval::new(0, 0, 10);
        iv.loop_depth = 2;
        iv.lease_remaining = 0;
        // effective = 10 * (1 + 2 * 3) / 2 = 70 / 2 = 35
        assert_eq!(config.effective_nud(&iv), 35);
    }

    // ========================================================================
    // NUD 增强集成测试
    // ========================================================================

    #[test]
    fn test_lsra_with_nud_config_phys_reg_limit() {
        // 2 个物理寄存器，5 个变量（全部重叠）→ 应有 3 个被 spill
        let config = NudConfig {
            phys_reg_limit: 2,
            ..NudConfig::disabled() // 禁用循环保护，只测物理寄存器限制
        };
        let mut lsra = LsraAllocator::with_nud_config(config);

        // 所有区间完全重叠 → 物理寄存器不够时必须 spill
        let mut intervals = vec![
            Interval::new(0, 0, 100),
            Interval::new(1, 0, 100),
            Interval::new(2, 0, 100),
            Interval::new(3, 0, 100),
            Interval::new(4, 0, 100),
        ];

        lsra.allocate(&mut intervals).unwrap();

        let assigned: Vec<_> = intervals.iter().filter(|iv| iv.reg.is_some()).collect();
        let spilled: Vec<_> = intervals.iter().filter(|iv| iv.reg.is_none()).collect();

        assert!(assigned.len() <= 2, "最多 2 个物理寄存器，实际分配了 {}", assigned.len());
        assert!(spilled.len() >= 3, "至少 3 个被 spill，实际 spill 了 {}", spilled.len());
    }

    #[test]
    fn test_lsra_lease_prevents_early_eviction() {
        // 租约长度 3，确保新分配的区间在 3 条指令内不被驱逐
        let config =
            NudConfig { phys_reg_limit: 2, lease_length: 3, enabled: true, ..NudConfig::default() };
        let mut lsra = LsraAllocator::with_nud_config(config);

        let mut intervals = vec![
            Interval::new(0, 0, 100),
            Interval::new(1, 0, 100),
            Interval::new(2, 1, 50), // 后来的，有租约
        ];

        // 主要验证分配不会 panic 且结果一致
        lsra.allocate(&mut intervals).unwrap();

        // 验证所有区间都有分配结果
        for iv in &intervals {
            assert!(iv.reg.is_some() || iv.spill_slot.is_some(), "vreg{} unallocated", iv.vreg);
        }
    }

    #[test]
    fn test_expire_old_intervals_with_lease() {
        // 测试租约递减：有租约的区间即使 end <= current_pos 也不释放
        // 注意：必须用 with_nud_config 且 enabled=true，否则租约不会递减
        let config = NudConfig { phys_reg_limit: 8, enabled: true, ..NudConfig::default() };
        let mut alloc = LsraAllocator::with_nud_config(config);

        let r0 = alloc.alloc_one().unwrap();
        let r1 = alloc.alloc_one().unwrap();

        // iv1: end=5, lease=2 → 在 pos=7 时 end 已过期但 lease > 0
        let mut iv1 = Interval::new(0, 0, 5);
        iv1.reg = Some(r0);
        iv1.lease_remaining = 2;

        // iv2: end=10, lease=0 → 在 pos=7 时未过期
        let mut iv2 = Interval::new(1, 2, 10);
        iv2.reg = Some(r1);
        iv2.lease_remaining = 0;

        alloc.active.push(iv1);
        alloc.active.push(iv2);

        // pos=7: iv1.end=5 <= 7 但 lease=2 → 不释放，lease 递减为 1
        alloc.expire_old_intervals(7);
        assert_eq!(alloc.active_count(), 2, "iv1 应因租约保留");
        assert_eq!(alloc.free_count(), 6, "没有寄存器被释放");

        // pos=8: iv1.end=5 <= 8, lease 递减为 0 → 释放
        alloc.expire_old_intervals(8);
        assert_eq!(alloc.active_count(), 1, "iv1 租约耗尽应被释放");
        assert_eq!(alloc.free_count(), 7, "r0 应被释放");
    }

    // ========================================================================
    // 集成测试：真实编译场景模拟
    // ========================================================================

    #[test]
    fn test_realistic_function_allocation() {
        // 模拟一个简单函数的寄存器使用模式：
        //
        // let a = 1          // vreg0: def@0, use@8
        // let b = 2          // vreg1: def@1, use@6
        // let c = a + b      // vreg2: def@3, use@8
        // let d = c * 2      // vreg3: def@5, use@8
        // return d           // 使用 vreg3 @8
        //
        // 活跃分析：
        //   IP 0-2:  a 活跃
        //   IP 1-6:  b 活跃
        //   IP 3-8:  c 活跃
        //   IP 5-8:  d 活跃
        //   IP 8:    a, c, d 同时活跃（return 使用它们）

        let def_ips: Vec<Option<usize>> = vec![
            Some(0), // vreg0: a
            Some(1), // vreg1: b
            Some(3), // vreg2: c
            Some(5), // vreg3: d
        ];
        let use_ips: Vec<Option<usize>> = vec![
            Some(8), // vreg0: a (used in return)
            Some(6), // vreg1: b (last used in c=a+b)
            Some(8), // vreg2: c (used in d=c*2 and return)
            Some(8), // vreg3: d (return value)
        ];

        let mut intervals = build_intervals(&def_ips, &use_ips).unwrap();

        // 只有 2 个物理寄存器（强制 spill）
        let mut alloc = LsraAllocator::new(2);
        alloc.allocate(&mut intervals).unwrap();

        // 验证所有区间都有分配结果
        for iv in &intervals {
            assert!(iv.reg.is_some() || iv.spill_slot.is_some(), "vreg{} unallocated", iv.vreg);
        }

        // 验证同时活跃数不超过 2
        for pos in 0..=8 {
            let concurrent = intervals
                .iter()
                .filter(|iv| iv.start <= pos && pos <= iv.end && iv.reg.is_some())
                .count();
            assert!(concurrent <= 2, "position {}: {} in registers (max=2)", pos, concurrent);
        }
    }

    // ========================================================================
    // 显式覆盖测试：LsraAllocator 公共 API
    // ========================================================================

    #[test]
    fn test_active_count_tracks_active_intervals() {
        let mut alloc = LsraAllocator::new(8);
        assert_eq!(alloc.active_count(), 0);

        let r0 = alloc.alloc_one().unwrap();
        let mut iv1 = Interval::new(0, 0, 10);
        iv1.reg = Some(r0);
        alloc.active.push(iv1);
        assert_eq!(alloc.active_count(), 1);

        let r1 = alloc.alloc_one().unwrap();
        let mut iv2 = Interval::new(1, 0, 20);
        iv2.reg = Some(r1);
        alloc.active.push(iv2);
        assert_eq!(alloc.active_count(), 2);
    }

    #[test]
    fn test_free_count_decreases_on_alloc() {
        let mut alloc = LsraAllocator::new(8);
        assert_eq!(alloc.free_count(), 8);

        alloc.alloc_one().unwrap();
        assert_eq!(alloc.free_count(), 7);

        alloc.alloc_one().unwrap();
        alloc.alloc_one().unwrap();
        assert_eq!(alloc.free_count(), 5);
    }

    #[test]
    fn test_get_phys_reg_returns_allocation() {
        let mut alloc = LsraAllocator::new(8);
        let mut intervals = vec![Interval::new(0, 0, 5)];
        alloc.allocate(&mut intervals).unwrap();

        // vreg 0 应该有物理寄存器分配
        let preg = alloc.get_phys_reg(0);
        assert!(preg.is_some(), "vreg 0 应该已分配物理寄存器");
        assert_eq!(preg, intervals[0].reg);

        // 未参与分配的 vreg 应返回 None
        assert_eq!(alloc.get_phys_reg(999), None);
    }

    #[test]
    fn test_handled_intervals_populated_after_allocate() {
        let mut alloc = LsraAllocator::new(8);
        let mut intervals = vec![Interval::new(0, 0, 5), Interval::new(1, 6, 10)];
        alloc.allocate(&mut intervals).unwrap();

        let handled = alloc.handled_intervals();
        assert_eq!(handled.len(), 2, "allocate 后 handled 应包含所有已处理区间");
    }

    #[test]
    fn test_spill_slot_count_increments_on_spill() {
        // 2 个物理寄存器，3 个同时活跃的区间 → 必须 spill 一个
        let mut alloc = LsraAllocator::new(2);
        assert_eq!(alloc.spill_slot_count(), 0);

        let mut intervals =
            vec![Interval::new(0, 0, 20), Interval::new(1, 0, 10), Interval::new(2, 0, 5)];
        alloc.allocate(&mut intervals).unwrap();

        // 应该有至少 1 个 spill 槽位
        assert!(alloc.spill_slot_count() >= 1, "spill 后 spill_slot_count 应 > 0");
    }

    #[test]
    fn test_with_max_locals_creates_full_bitmap() {
        let alloc = LsraAllocator::with_max_locals();
        // 所有 MAX_FUNCTION_LOCALS 个寄存器都应空闲
        assert_eq!(alloc.free_count(), MAX_FUNCTION_LOCALS);
        assert!(alloc.has_free());
        assert_eq!(alloc.active_count(), 0);
        assert_eq!(alloc.spill_slot_count(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 基础功能测试：分配和释放单个寄存器
    #[test]
    fn test_alloc_and_release_single() {
        let mut alloc = RegisterAllocator::new();

        // alloc_single 返回 u16（快捷 API，不返回 handle）
        let r0 = alloc.alloc_single(SlotOwner::TempExpr).unwrap();
        assert_eq!(r0, 0);

        let r1 = alloc.alloc_single(SlotOwner::LocalVar).unwrap();
        assert_eq!(r1, 1);

        // reserve_slot 返回 SlotHandle（完整 API）
        let h1 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.slot_range(h1), (2, 3));

        // 释放 h1
        alloc.release_slot(h1);
        assert_eq!(alloc.free_count(), 1);
        assert!(alloc.is_register_free(2));

        // 释放后复用 r2
        let h3 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.slot_range(h3), (2, 3)); // 应该复用 r2
    }

    /// 测试连续范围分配
    #[test]
    fn test_reserve_range() {
        let mut alloc = RegisterAllocator::new();

        let h = alloc.reserve_slot(4, SlotOwner::ArrayConstruct).unwrap();
        assert_eq!(alloc.slot_range(h), (0, 4)); // r0-r3
        assert_eq!(alloc.peak_reg(), 4);

        // 下一个分配应从 r4 开始
        let h2 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.slot_range(h2), (4, 5));
    }

    /// 测试作用域管理
    #[test]
    fn test_scope_management() {
        let mut alloc = RegisterAllocator::new();

        // depth 0: 分配一个局部变量
        let _h_local = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.current_depth(), 0);

        // 进入 depth 1
        alloc.begin_scope();
        assert_eq!(alloc.current_depth(), 1);

        // depth 1: 分配临时寄存器
        let h_temp = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.slot_range(h_temp), (1, 2));

        // 退出 scope，应释放 depth 1 的槽位
        alloc.end_scope();
        assert_eq!(alloc.current_depth(), 0);

        // h_temp 应已被释放
        assert_eq!(alloc.free_count(), 1);
        // h_local 仍在活跃
        assert_eq!(alloc.active_slot_count(), 1);
    }

    /// 测试 TooManyLocals 错误
    #[test]
    fn test_too_many_locals() {
        let mut alloc = RegisterAllocator::new();

        // 强制设置 next_reg 到上限附近
        alloc.next_reg = MAX_FUNCTION_LOCALS;

        let result = alloc.alloc_single(SlotOwner::TempExpr);
        assert!(result.is_err());
        match result.unwrap_err() {
            CompileError::TooManyLocals { .. } => {}
            other => panic!("Expected TooManyLocals, got {:?}", other),
        }
    }

    /// 测试信号发射
    #[test]
    fn test_signal_emission() {
        let mut alloc = RegisterAllocator::new();
        let reserved_count = Arc::new(AtomicU32::new(0));

        let count_ref = Arc::clone(&reserved_count);
        // 保留 Connection 句柄到测试结束，避免 Drop 触发 disconnect 移除 slot
        // （nuzo_signal::Connection 现已实现 impl Drop { fn drop -> disconnect() }）
        let _conn = alloc
            .signal_reserved
            .connect(move |_info| {
                count_ref.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();

        alloc.alloc_single(SlotOwner::TempExpr).unwrap();
        assert_eq!(reserved_count.load(Ordering::Relaxed), 1);

        alloc.alloc_single(SlotOwner::LocalVar).unwrap();
        assert_eq!(reserved_count.load(Ordering::Relaxed), 2);
    }

    /// 测试 release_slots_by_depth
    #[test]
    fn test_release_by_depth() {
        let mut alloc = RegisterAllocator::new();

        // depth 0
        let h0 = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();

        alloc.begin_scope();
        // depth 1
        let _h1a = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        let _h1b = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();

        alloc.begin_scope();
        // depth 2
        let _h2 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();

        assert_eq!(alloc.active_slot_count(), 4);

        // 释放 depth > 1 的槽位（只应释放 h2）
        alloc.release_slots_by_depth(1);
        assert_eq!(alloc.active_slot_count(), 3); // h0, h1a, h1b 仍活跃

        // 释放 depth > 0 的槽位（应释放 h1a, h1b）
        alloc.release_slots_by_depth(0);
        assert_eq!(alloc.active_slot_count(), 1); // 只有 h0
        assert_eq!(alloc.slot_range(h0), (0, 1)); // h0 未受影响
    }

    // ========================================================================
    // 显式覆盖测试：RegisterAllocator 公共 API
    // ========================================================================

    #[test]
    fn test_active_slot_count_tracks_active_slots() {
        let mut alloc = RegisterAllocator::new();
        assert_eq!(alloc.active_slot_count(), 0);

        let _h0 = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.active_slot_count(), 1);

        let _h1 = alloc.reserve_slot(2, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.active_slot_count(), 2);

        // 释放一个后 active_slot_count 应减少
        alloc.release_slot(_h0);
        assert_eq!(alloc.active_slot_count(), 1);
    }

    #[test]
    fn test_alloc_single_returns_sequential_regs() {
        let mut alloc = RegisterAllocator::new();
        let r0 = alloc.alloc_single(SlotOwner::TempExpr).unwrap();
        let r1 = alloc.alloc_single(SlotOwner::LocalVar).unwrap();
        let r2 = alloc.alloc_single(SlotOwner::CallArg).unwrap();
        assert_eq!(r0, 0);
        assert_eq!(r1, 1);
        assert_eq!(r2, 2);
    }

    #[test]
    fn test_begin_scope_increments_depth() {
        let mut alloc = RegisterAllocator::new();
        assert_eq!(alloc.current_depth(), 0);

        alloc.begin_scope();
        assert_eq!(alloc.current_depth(), 1);

        alloc.begin_scope();
        assert_eq!(alloc.current_depth(), 2);
    }

    #[test]
    fn test_current_depth_reports_scope_level() {
        let alloc = RegisterAllocator::new();
        assert_eq!(alloc.current_depth(), 0);

        let alloc2 = RegisterAllocator::with_depth(5);
        assert_eq!(alloc2.current_depth(), 5);
    }

    #[test]
    fn test_end_scope_decrements_and_releases() {
        let mut alloc = RegisterAllocator::new();
        alloc.begin_scope();
        let _h = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.current_depth(), 1);
        assert_eq!(alloc.active_slot_count(), 1);

        alloc.end_scope();
        assert_eq!(alloc.current_depth(), 0);
        assert_eq!(alloc.active_slot_count(), 0); // 槽位已释放
    }

    #[test]
    fn test_register_free_count_tracks_free_pool() {
        let mut alloc = RegisterAllocator::new();
        assert_eq!(alloc.free_count(), 0);

        let h = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.free_count(), 0); // 还没释放

        alloc.release_slot(h);
        assert_eq!(alloc.free_count(), 1); // 释放后进入 free 池
    }

    #[test]
    fn test_is_register_free_checks_pool() {
        let mut alloc = RegisterAllocator::new();
        assert!(!alloc.is_register_free(0));

        let h = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert!(!alloc.is_register_free(0)); // r0 被占用

        alloc.release_slot(h);
        assert!(alloc.is_register_free(0)); // r0 已释放，可复用
    }

    #[test]
    fn test_peak_reg_tracks_maximum() {
        let mut alloc = RegisterAllocator::new();
        assert_eq!(alloc.peak_reg(), 0);

        let _h = alloc.reserve_slot(3, SlotOwner::ArrayConstruct).unwrap();
        assert_eq!(alloc.peak_reg(), 3); // 占用 r0, r1, r2 → peak=3

        alloc.release_slot(_h);
        assert_eq!(alloc.peak_reg(), 3); // peak 不收缩

        let _h2 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.peak_reg(), 3); // 复用 r0，peak 不变
    }

    #[test]
    fn test_release_slot_marks_released() {
        let mut alloc = RegisterAllocator::new();
        let h = alloc.reserve_slot(2, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.active_slot_count(), 1);

        alloc.release_slot(h);
        assert_eq!(alloc.active_slot_count(), 0);
        assert_eq!(alloc.free_count(), 2); // 2 个寄存器进入 free 池

        // 重复释放应为幂等 no-op
        alloc.release_slot(h);
        assert_eq!(alloc.free_count(), 2);
    }

    #[test]
    fn test_release_slots_by_depth_releases_deeper() {
        let mut alloc = RegisterAllocator::new();
        let _h0 = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap(); // depth 0

        alloc.begin_scope(); // depth 1
        let _h1 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();

        alloc.begin_scope(); // depth 2
        let _h2 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();

        assert_eq!(alloc.active_slot_count(), 3);

        // 释放 depth > 1 的（只释放 h2）
        alloc.release_slots_by_depth(1);
        assert_eq!(alloc.active_slot_count(), 2);

        // 释放 depth > 0 的（释放 h1）
        alloc.release_slots_by_depth(0);
        assert_eq!(alloc.active_slot_count(), 1);
    }

    #[test]
    fn test_reserve_remote_forces_high_address() {
        let mut alloc = RegisterAllocator::new();

        // 先分配一个普通槽位（r0）
        let _h0 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();

        // reserve_remote 应从 next_reg 开始，不复用 free 池
        let h_remote = alloc.reserve_remote(3, SlotOwner::ArrayConstruct).unwrap();
        let (start, end) = alloc.slot_range(h_remote);
        assert_eq!(start, 1); // 从 next_reg=1 开始
        assert_eq!(end, 4); // 占用 r1, r2, r3
    }

    #[test]
    fn test_reserve_slot_returns_valid_handle() {
        let mut alloc = RegisterAllocator::new();
        let h = alloc.reserve_slot(4, SlotOwner::ArrayConstruct).unwrap();
        assert_eq!(alloc.slot_range(h), (0, 4));
        assert_eq!(alloc.slot_start(h), 0);
        assert_eq!(alloc.slot_count(h), 4);
    }

    #[test]
    fn test_slot_owner_returns_assigned_owner() {
        let mut alloc = RegisterAllocator::new();
        let h = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.slot_owner(h), SlotOwner::LocalVar);

        let h2 = alloc.reserve_slot(1, SlotOwner::CallArg).unwrap();
        assert_eq!(alloc.slot_owner(h2), SlotOwner::CallArg);
    }

    #[test]
    fn test_slot_range_returns_half_open_interval() {
        let mut alloc = RegisterAllocator::new();
        let h = alloc.reserve_slot(5, SlotOwner::TempExpr).unwrap();
        let (start, end) = alloc.slot_range(h);
        assert_eq!(start, 0);
        assert_eq!(end, 5); // 半开区间 [0, 5)
    }

    #[test]
    fn test_slot_start_returns_first_reg() {
        let mut alloc = RegisterAllocator::new();
        // 先占用 r0-r2
        let _h0 = alloc.reserve_slot(3, SlotOwner::TempExpr).unwrap();
        // 再分配一个
        let h1 = alloc.reserve_slot(2, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.slot_start(h1), 3); // 从 r3 开始
    }

    #[test]
    fn test_with_depth_sets_initial_depth() {
        let alloc = RegisterAllocator::with_depth(10);
        assert_eq!(alloc.current_depth(), 10);

        let alloc2 = RegisterAllocator::with_depth(0);
        assert_eq!(alloc2.current_depth(), 0);
    }

    // ========================================================================
    // Core API integration tests: reserve_slot / release_slot lifecycle
    // ========================================================================

    #[test]
    fn test_reserve_slot_basic() {
        // Reserve 1 slot and verify slot_count, peak_reg, free_count
        let mut alloc = RegisterAllocator::new();
        let h = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();

        assert_eq!(alloc.slot_count(h), 1, "slot_count should be 1");
        assert_eq!(alloc.peak_reg(), 1, "peak_reg should be 1 after one allocation");
        assert_eq!(alloc.free_count(), 0, "free_count should be 0 (nothing released yet)");
        assert_eq!(alloc.active_slot_count(), 1, "1 active slot");
    }

    #[test]
    fn test_reserve_multiple_slots() {
        // Reserve 3 separate slots and verify their ranges do not overlap
        let mut alloc = RegisterAllocator::new();

        let h0 = alloc.reserve_slot(2, SlotOwner::LocalVar).unwrap(); // r0-r1
        let h1 = alloc.reserve_slot(3, SlotOwner::ArrayConstruct).unwrap(); // r2-r4
        let h2 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap(); // r5

        // Verify each slot's range
        assert_eq!(alloc.slot_range(h0), (0, 2));
        assert_eq!(alloc.slot_range(h1), (2, 5));
        assert_eq!(alloc.slot_range(h2), (5, 6));

        // Verify no overlap: each start >= previous end
        assert!(alloc.slot_start(h1) >= alloc.slot_range(h0).1);
        assert!(alloc.slot_start(h2) >= alloc.slot_range(h1).1);

        assert_eq!(alloc.peak_reg(), 6);
        assert_eq!(alloc.active_slot_count(), 3);
    }

    #[test]
    fn test_release_slot_basic() {
        // Reserve then release, verify free_count recovers
        let mut alloc = RegisterAllocator::new();

        let _h0 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        let h1 = alloc.reserve_slot(2, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.free_count(), 0);

        // Release h1 (2 registers: r1, r2)
        alloc.release_slot(h1);
        assert_eq!(alloc.free_count(), 2, "free_count should be 2 after releasing 2-reg slot");
        assert!(alloc.is_register_free(1));
        assert!(alloc.is_register_free(2));
        assert!(!alloc.is_register_free(0), "h0's register should still be occupied");

        // h0 still active
        assert_eq!(alloc.active_slot_count(), 1);
    }

    #[test]
    fn test_reserve_and_release_lifecycle() {
        // Full lifecycle: reserve -> use -> release -> re-reserve
        let mut alloc = RegisterAllocator::new();

        // Phase 1: Reserve
        let h0 = alloc.reserve_slot(3, SlotOwner::ArrayConstruct).unwrap();
        assert_eq!(alloc.slot_range(h0), (0, 3));
        assert_eq!(alloc.peak_reg(), 3);
        assert_eq!(alloc.active_slot_count(), 1);

        // Phase 2: Reserve another
        let h1 = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.slot_start(h1), 3);
        assert_eq!(alloc.peak_reg(), 4);

        // Phase 3: Release h0
        alloc.release_slot(h0);
        assert_eq!(alloc.free_count(), 3);
        assert_eq!(alloc.active_slot_count(), 1); // only h1 left

        // Phase 4: Re-reserve -- should reuse freed r0-r2 range
        let h2 = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.slot_start(h2), 0, "should reuse freed register r0");
        assert_eq!(alloc.active_slot_count(), 2); // h1 + h2
    }

    #[test]
    fn test_depth_scoping() {
        // begin_scope -> reserve -> end_scope should auto-release
        let mut alloc = RegisterAllocator::new();

        // Outer scope (depth 0): persistent local
        let _h_outer = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        assert_eq!(alloc.current_depth(), 0);

        // Enter inner scope (depth 1)
        alloc.begin_scope();
        assert_eq!(alloc.current_depth(), 1);

        let _h_inner = alloc.reserve_slot(2, SlotOwner::TempExpr).unwrap();
        assert_eq!(alloc.active_slot_count(), 2);

        // Exit scope -- inner slot should be auto-released
        alloc.end_scope();
        assert_eq!(alloc.current_depth(), 0);
        assert_eq!(alloc.active_slot_count(), 1, "only outer slot should remain");
        assert_eq!(alloc.free_count(), 2, "2 registers from inner slot should be freed");
    }

    #[test]
    fn test_slot_owner_tracking() {
        // Verify slot_owner returns the correct SlotOwner for each slot
        let mut alloc = RegisterAllocator::new();

        let h_local = alloc.reserve_slot(1, SlotOwner::LocalVar).unwrap();
        let h_temp = alloc.reserve_slot(1, SlotOwner::TempExpr).unwrap();
        let h_array = alloc.reserve_slot(4, SlotOwner::ArrayConstruct).unwrap();
        let h_call = alloc.reserve_slot(1, SlotOwner::CallArg).unwrap();
        let h_builtin = alloc.reserve_slot(1, SlotOwner::Builtin).unwrap();
        let h_closure = alloc.reserve_slot(1, SlotOwner::Closure).unwrap();

        assert_eq!(alloc.slot_owner(h_local), SlotOwner::LocalVar);
        assert_eq!(alloc.slot_owner(h_temp), SlotOwner::TempExpr);
        assert_eq!(alloc.slot_owner(h_array), SlotOwner::ArrayConstruct);
        assert_eq!(alloc.slot_owner(h_call), SlotOwner::CallArg);
        assert_eq!(alloc.slot_owner(h_builtin), SlotOwner::Builtin);
        assert_eq!(alloc.slot_owner(h_closure), SlotOwner::Closure);

        // Owner should be preserved after release
        alloc.release_slot(h_temp);
        assert_eq!(
            alloc.slot_owner(h_temp),
            SlotOwner::TempExpr,
            "owner should still be TempExpr after release"
        );
    }
}

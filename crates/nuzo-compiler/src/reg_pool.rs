//! 单端布局寄存器池 (DualPool)
//!
//! 持久区和临时区共享从 0 向上增长的游标，通过 checkpoint 批量回收持久区，
//! 通过 free 列表复用临时区。消除 `acquire_avoiding` 的 O(N²) 扫描，
//! 使大数组编译 codegen 从 O(N²) 降为 O(N)。
//!
//! ## 设计原理
//!
//! 原始 spec 设计为双端布局（持久区从 0 向上，临时区从 MAX 向下），
//! 但双端布局与 VM 的线性寄存器编号不兼容：
//! - VM 用 `registers.resize(new_base + locals_count)` 分配数组
//! - 寄存器编号直接作为数组索引
//! - 临时区寄存器编号接近 MAX（如 4093-4095）会导致越界访问
//!
//! 单端布局将所有寄存器编号限制在 `0..peak` 范围内，确保 VM 不会越界，
//! 同时通过 checkpoint/restore 机制实现 O(1) 批量回收，消除 O(N²) 瓶颈。
//!
//! ## ArrayNew 工作流示例
//!
//! ```text
//! dest_reg = acquire_persistent()     // reg=0, top=1
//! checkpoint = save_checkpoint()      // checkpoint=1
//! idx_reg = acquire_temp()           // reg=1, top=2
//! elem_reg[0] = acquire_persistent() // reg=2, top=3
//! elem_reg[1] = acquire_persistent() // reg=3, top=4
//! ...                                // peak=N+2
//! deallocate_temp(idx_reg)           // reg=1 push 到 temp_free
//! restore_checkpoint(checkpoint)     // top=1, temp_free truncate 到快照长度
//! // 最终 peak=N+2，所有编号 0..N+1 在范围内
//! ```
//!
//! ## 复杂度
//!
//! | 操作               | 复杂度    |
//! |-------------------|----------|
//! | acquire_persistent | O(1)     |
//! | acquire_temp      | O(1)     |
//! | release_temp      | O(1)     |
//! | save_checkpoint   | O(1)     |
//! | restore_checkpoint| O(1)     |
//!
//! `temp_free` 采用 **LIFO 栈**（`Vec<u16>` + push/pop 末尾，O(1)）：
//! - `release_temp` 直接 `push` 到末尾（无排序，O(1)），消除原二分查找 + `Vec::insert` 的 O(N) 回归
//! - `acquire_temp` 从末尾 `pop` 复用（LIFO：后释放先复用），或从 `top` 递增（O(1)）
//! - `restore_checkpoint` 用 `truncate(temp_free_len)`：`u16` 是 `Copy`（无 `Drop`），
//!   `drop_in_place` 编译为空，仅设置 `len`，实际 O(1)
//!
//! 注意：`restore_checkpoint` 通过 `truncate(cp.temp_free_len)` 仅回滚 checkpoint 之后
//! 释放的寄存器，**保留** checkpoint 之前合法释放的寄存器。这修正了原 `clear()` 的语义
//! 错误——`clear` 会清空所有已释放寄存器（包括 checkpoint 前合法释放的），导致后续
//! `acquire_temp` 无法复用本应可用的低编号寄存器，间接推高 `peak`。
//!
//! ## 设计决策摘要
//!
//! 1. **LIFO 而非 epoch 方案**：LIFO push/pop 恒为 O(1)；epoch 方案 `acquire_temp`
//!    仍需遍历跳过过期条目，最坏 O(N)。LIFO 更简单（无 epoch 字段，纯 Vec）。
//! 2. **Checkpoint 结构体而非独立栈**：`Checkpoint { top, temp_free_len }` 类型安全，
//!    编译器强制所有调用点更新，避免遗漏；嵌套 checkpoint 天然支持（每个快照独立）。
//! 3. **LIFO 不影响正确性**：寄存器编号在 VM 中仅作数组索引，无顺序语义；
//!    codegen 分配的编号都在 `0..peak` 范围内，VM `registers.resize(peak)` 保证不越界。

use nuzo_core::constants::MAX_FUNCTION_LOCALS;

/// 寄存器分配检查点快照
///
/// 携带 `top` 游标与 `temp_free` 长度，使 `restore_checkpoint` 能 O(1) truncate。
/// 替代裸 `u16`：显式携带 `temp_free` 长度快照，仅回滚 checkpoint 之后释放的寄存器，
/// 保留之前合法释放的寄存器（修正原 `clear()` 的语义错误）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    /// 保存时的 `top` 游标值
    pub top: u16,
    /// 保存时的 `temp_free` 长度（truncate 目标）
    pub temp_free_len: usize,
}

/// 单端布局寄存器池
///
/// 持久区和临时区共享从 0 向上增长的游标 `top`。
/// - 持久区寄存器通过 `acquire_persistent` 分配，不单个回收
///   （由 `restore_checkpoint` 批量回收到 checkpoint 位置）
/// - 临时区寄存器通过 `acquire_temp` 分配，可通过 `release_temp` 回收到 free 列表复用
pub(crate) struct DualPool {
    /// 分配游标（从 0 向上增长，持久区和临时区共享）
    top: u16,
    /// 峰值寄存器数（max top，用于设置 locals_count）
    peak: u16,
    /// 临时区已释放的寄存器（LIFO 栈：push/pop 末尾，O(1) 复用后释放的寄存器）
    temp_free: Vec<u16>,
}

/// 寄存器池耗尽错误
///
/// 携带分配失败时的诊断信息：已分配数、剩余可用数、峰值数。
/// 实现 `Display` + `Error`，便于上层 `?` 传播与错误链展示。
#[derive(Debug, Clone)]
pub(crate) struct RegPoolExhausted {
    /// 当前已分配寄存器数（= `top` 游标值）
    pub count: u16,
    /// 剩余可用寄存器数（= `MAX_FUNCTION_LOCALS - top`，耗尽时为 0）
    pub available: u16,
    /// 峰值寄存器数（历史最高 `top`）
    pub peak: u16,
}

impl std::fmt::Display for RegPoolExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "register pool exhausted: allocated={}, available={}, peak={}, max={}",
            self.count, self.available, self.peak, MAX_FUNCTION_LOCALS
        )
    }
}

impl std::error::Error for RegPoolExhausted {}

impl DualPool {
    pub(crate) fn new() -> Self {
        Self { top: 0, peak: 0, temp_free: Vec::new() }
    }

    /// 分配一个持久区寄存器 (O(1))
    ///
    /// 从 `top` 递增分配，不单个回收（由 `restore_checkpoint` 批量回收）。
    /// 碰撞检测：`top >= MAX_FUNCTION_LOCALS` 时返回错误。
    pub(crate) fn acquire_persistent(&mut self) -> Result<u16, RegPoolExhausted> {
        if self.top >= MAX_FUNCTION_LOCALS {
            return Err(RegPoolExhausted {
                count: self.top,
                available: MAX_FUNCTION_LOCALS.saturating_sub(self.top),
                peak: self.peak,
            });
        }
        let reg = self.top;
        self.top += 1;
        self.peak = self.peak.max(self.top);
        Ok(reg)
    }

    /// 分配一个临时区寄存器 (O(1))
    ///
    /// 优先从 `temp_free` 末尾 `pop` 复用（LIFO：后释放先复用），否则从 `top` 递增分配。
    /// 寄存器编号无顺序语义（VM 仅用作数组索引），LIFO 天然 O(1)，消除原降序维护的 O(N) 开销。
    pub(crate) fn acquire_temp(&mut self) -> Result<u16, RegPoolExhausted> {
        if let Some(reg) = self.temp_free.pop() {
            return Ok(reg);
        }
        if self.top >= MAX_FUNCTION_LOCALS {
            return Err(RegPoolExhausted {
                count: self.top,
                available: MAX_FUNCTION_LOCALS.saturating_sub(self.top),
                peak: self.peak,
            });
        }
        let reg = self.top;
        self.top += 1;
        self.peak = self.peak.max(self.top);
        Ok(reg)
    }

    /// 释放临时区寄存器回 free 栈 (O(1))
    ///
    /// 直接 `push` 到 `temp_free` 末尾（LIFO 栈，无排序），消除原二分查找 + `Vec::insert`
    /// 的 O(N) 开销。后续 `acquire_temp` 从末尾 `pop` 复用（后释放先复用）。
    pub(crate) fn release_temp(&mut self, reg: u16) {
        self.temp_free.push(reg);
    }

    /// 分配连续 `count` 个临时寄存器，返回起始寄存器编号 (O(1))
    ///
    /// 从 `top` 推进 `count` 个位置，**不**从 `temp_free` 复用，保证返回的
    /// 区间 `[base, base+count)` 物理连续。供 StringBuild 等 VM 要求操作数
    /// 位于连续寄存器的指令使用。
    ///
    /// # 失败条件
    /// `top + count > MAX_FUNCTION_LOCALS` 时返回 `RegPoolExhausted`。
    pub(crate) fn acquire_temp_block(&mut self, count: u16) -> Result<u16, RegPoolExhausted> {
        if count == 0 {
            return Ok(self.top);
        }
        let new_top = self.top.checked_add(count).ok_or_else(|| RegPoolExhausted {
            count: self.top,
            available: MAX_FUNCTION_LOCALS.saturating_sub(self.top),
            peak: self.peak,
        })?;
        if new_top > MAX_FUNCTION_LOCALS {
            return Err(RegPoolExhausted {
                count: self.top,
                available: MAX_FUNCTION_LOCALS.saturating_sub(self.top),
                peak: self.peak,
            });
        }
        let base = self.top;
        self.top = new_top;
        self.peak = self.peak.max(self.top);
        Ok(base)
    }

    /// 释放 `acquire_temp_block` 分配的连续寄存器块 (O(N))
    ///
    /// 将 `[base, base+count)` 中的每个寄存器 push 到 `temp_free`，
    /// 供后续 `acquire_temp` 复用。VM 寄存器编号无顺序语义，复用安全。
    pub(crate) fn release_temp_block(&mut self, base: u16, count: u16) {
        for i in 0..count {
            self.temp_free.push(base + i);
        }
    }

    /// 保存检查点 (O(1))
    ///
    /// 返回 `Checkpoint { top, temp_free_len }`，携带 `top` 游标与 `temp_free` 长度快照。
    /// 后续可通过 `restore_checkpoint` 回退 `top` 并 `truncate` `temp_free` 到快照长度，
    /// 批量释放 [checkpoint.top, top) 范围内的持久寄存器，仅回滚 checkpoint 后释放的临时寄存器。
    pub(crate) fn save_checkpoint(&self) -> Checkpoint {
        Checkpoint { top: self.top, temp_free_len: self.temp_free.len() }
    }

    /// 恢复检查点 (O(1))
    ///
    /// 将 `top` 回退到 `cp.top`，批量释放持久区寄存器。
    /// 同时 `truncate` `temp_free` 到 `cp.temp_free_len`：仅回滚 checkpoint 之后释放的
    /// 临时寄存器，**保留** checkpoint 之前合法释放的寄存器（修正原 `clear()` 语义错误）。
    /// `Vec::truncate` 对 `u16`（`Copy`，无 `Drop`）实际为 O(1)：仅设置 `len`。
    pub(crate) fn restore_checkpoint(&mut self, cp: Checkpoint) {
        self.top = cp.top;
        self.temp_free.truncate(cp.temp_free_len);
    }

    /// 返回峰值寄存器数（用于设置 locals_count）
    ///
    /// 所有寄存器编号在 `0..peak` 范围内，确保 VM 的寄存器数组不会越界。
    pub(crate) fn peak(&self) -> u16 {
        self.peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dual_pool_empty() {
        let pool = DualPool::new();
        assert_eq!(pool.peak(), 0);
    }

    #[test]
    fn test_acquire_persistent_sequential() {
        let mut pool = DualPool::new();
        assert_eq!(pool.acquire_persistent().unwrap(), 0);
        assert_eq!(pool.acquire_persistent().unwrap(), 1);
        assert_eq!(pool.acquire_persistent().unwrap(), 2);
        assert_eq!(pool.peak(), 3);
    }

    #[test]
    fn test_acquire_temp_sequential() {
        let mut pool = DualPool::new();
        // 单端布局：临时区从 0 向上分配
        assert_eq!(pool.acquire_temp().unwrap(), 0);
        assert_eq!(pool.acquire_temp().unwrap(), 1);
        assert_eq!(pool.acquire_temp().unwrap(), 2);
        assert_eq!(pool.peak(), 3);
    }

    #[test]
    fn test_release_temp_reuse() {
        let mut pool = DualPool::new();
        let r0 = pool.acquire_temp().unwrap(); // 0
        let r1 = pool.acquire_temp().unwrap(); // 1
        assert_eq!(r0, 0);
        assert_eq!(r1, 1);
        pool.release_temp(r0);
        // 复用：从 temp_free 列表弹出 r0=0
        let r2 = pool.acquire_temp().unwrap();
        assert_eq!(r2, 0);
        // 再分配应从 top=2 继续递增
        let r3 = pool.acquire_temp().unwrap();
        assert_eq!(r3, 2);
    }

    #[test]
    fn test_checkpoint_restore() {
        let mut pool = DualPool::new();
        let _r0 = pool.acquire_persistent().unwrap(); // top=1
        let _r1 = pool.acquire_persistent().unwrap(); // top=2
        let checkpoint = pool.save_checkpoint();
        assert_eq!(checkpoint.top, 2);
        let _r2 = pool.acquire_persistent().unwrap(); // top=3
        pool.restore_checkpoint(checkpoint); // top=2
        // 恢复后可重新分配同一位置
        let r3 = pool.acquire_persistent().unwrap();
        assert_eq!(r3, 2);
    }

    #[test]
    fn test_nested_checkpoint() {
        let mut pool = DualPool::new();
        let _r0 = pool.acquire_persistent().unwrap(); // top=1
        let outer = pool.save_checkpoint(); // outer=1
        let _r1 = pool.acquire_persistent().unwrap(); // top=2
        let inner = pool.save_checkpoint(); // inner=2
        let _r2 = pool.acquire_persistent().unwrap(); // top=3
        // 内层 restore
        pool.restore_checkpoint(inner); // top=2
        let r3 = pool.acquire_persistent().unwrap();
        assert_eq!(r3, 2);
        // 外层 restore
        pool.restore_checkpoint(outer); // top=1
        let r4 = pool.acquire_persistent().unwrap();
        assert_eq!(r4, 1);
    }

    #[test]
    fn test_partition_isolation() {
        // 单端布局：持久区和临时区共享 top，但分配顺序保证不冲突
        // 在 ArrayNew 中：dest_persist → checkpoint → idx_temp → elem_persist
        // idx_temp 和 elem_persist 编号不同（top 递增）
        let mut pool = DualPool::new();
        let p0 = pool.acquire_persistent().unwrap(); // 0
        let t0 = pool.acquire_temp().unwrap(); // 1
        let p1 = pool.acquire_persistent().unwrap(); // 2
        let t1 = pool.acquire_temp().unwrap(); // 3
        // 编号递增，不冲突
        assert_eq!(p0, 0);
        assert_eq!(t0, 1);
        assert_eq!(p1, 2);
        assert_eq!(t1, 3);
        assert_ne!(p0, t0);
        assert_ne!(p1, t1);
        assert_ne!(p0, p1);
        assert_ne!(t0, t1);
    }

    #[test]
    fn test_peak_tracking() {
        let mut pool = DualPool::new();
        let _p0 = pool.acquire_persistent().unwrap(); // top=1, peak=1
        let _p1 = pool.acquire_persistent().unwrap(); // top=2, peak=2
        let _t0 = pool.acquire_temp().unwrap(); // top=3, peak=3
        assert_eq!(pool.peak(), 3);
        let _t1 = pool.acquire_temp().unwrap(); // top=4, peak=4
        assert_eq!(pool.peak(), 4);
        // restore 不影响 peak（高水位不下降）；此处 temp_free 为空，truncate 到 0 等效清空
        pool.restore_checkpoint(Checkpoint { top: 0, temp_free_len: 0 });
        assert_eq!(pool.peak(), 4);
    }

    #[test]
    fn test_collision_detection() {
        let mut pool = DualPool::new();
        // 分配直到耗尽
        for _ in 0..MAX_FUNCTION_LOCALS {
            assert!(pool.acquire_persistent().is_ok());
        }
        // top == MAX，耗尽
        assert!(matches!(pool.acquire_persistent(), Err(RegPoolExhausted { .. })));
        // temp 也应耗尽（temp_free 为空，top == MAX）
        assert!(matches!(pool.acquire_temp(), Err(RegPoolExhausted { .. })));
    }

    #[test]
    fn test_temp_free_does_not_affect_persist() {
        // 释放 temp 后再分配 persist，两者不冲突
        let mut pool = DualPool::new();
        let _p0 = pool.acquire_persistent().unwrap(); // 0, top=1
        let t0 = pool.acquire_temp().unwrap(); // 1, top=2
        pool.release_temp(t0); // t0=1 回到 temp_free
        let _p1 = pool.acquire_persistent().unwrap(); // 2, top=3 (不从 temp_free 复用)
        // temp 复用不影响 persist 分配
        let t1 = pool.acquire_temp().unwrap(); // 从 temp_free 复用 1
        assert_eq!(t1, t0);
        assert_eq!(t1, 1);
    }

    #[test]
    fn test_mixed_persist_temp_peak() {
        let mut pool = DualPool::new();
        let _ = pool.acquire_persistent().unwrap(); // 0, top=1
        let _ = pool.acquire_persistent().unwrap(); // 1, top=2
        let _ = pool.acquire_temp().unwrap(); // 2, top=3, peak=3
        assert_eq!(pool.peak(), 3);
        // 此处 temp_free 为空，truncate 到 0 等效清空
        pool.restore_checkpoint(Checkpoint { top: 0, temp_free_len: 0 });
        // peak 不变（高水位不下降）
        assert_eq!(pool.peak(), 3);
        let _ = pool.acquire_persistent().unwrap(); // 0, top=1
        // peak 仍为 3
        assert_eq!(pool.peak(), 3);
    }

    #[test]
    fn test_restore_checkpoint_clears_temp_free() {
        // restore_checkpoint 通过 truncate 回滚 checkpoint 后释放的寄存器。
        // 此处 checkpoint 前 temp_free 为空，truncate 到 0 等效清空。
        let mut pool = DualPool::new();
        let _p0 = pool.acquire_persistent().unwrap(); // 0, top=1
        let checkpoint = pool.save_checkpoint(); // top=1, temp_free_len=0
        let t0 = pool.acquire_temp().unwrap(); // 1, top=2
        pool.release_temp(t0); // 1 push 到 temp_free=[1]
        let _p1 = pool.acquire_persistent().unwrap(); // 2, top=3
        // restore: top=1, truncate 到 len=0 → temp_free=[]（checkpoint 前为空）
        pool.restore_checkpoint(checkpoint);
        // 下一个 acquire_temp 不应复用已回滚的 1，而是从 top=1 分配
        let r = pool.acquire_temp().unwrap();
        assert_eq!(r, 1);
    }

    #[test]
    fn test_release_temp_lifo_reuse() {
        // 验证 LIFO 栈的复用语义：乱序释放多个寄存器后，
        // acquire_temp 按后释放先复用（LIFO）顺序弹出。
        // temp_free 保持 push 顺序（无排序），pop 从末尾取。
        let mut pool = DualPool::new();
        let _ = pool.acquire_temp().unwrap(); // 0, top=1
        let _ = pool.acquire_temp().unwrap(); // 1, top=2
        let _ = pool.acquire_temp().unwrap(); // 2, top=3
        let _ = pool.acquire_temp().unwrap(); // 3, top=4
        // 乱序释放：3, 1, 2, 0 —— push 顺序保留，temp_free=[3,1,2,0]
        pool.release_temp(3);
        pool.release_temp(1);
        pool.release_temp(2);
        pool.release_temp(0);
        // LIFO pop 顺序：0, 2, 1, 3（末尾先 pop）
        assert_eq!(pool.acquire_temp().unwrap(), 0);
        assert_eq!(pool.acquire_temp().unwrap(), 2);
        assert_eq!(pool.acquire_temp().unwrap(), 1);
        assert_eq!(pool.acquire_temp().unwrap(), 3);
        // temp_free 耗尽后从 top=4 继续
        assert_eq!(pool.acquire_temp().unwrap(), 4);
    }

    #[test]
    fn test_multi_array_temp_reuse() {
        // 模拟两个数组字面量的 codegen：dest(persist) + idx/elem(temp)。
        // 第二个数组应复用第一个数组释放的临时寄存器（LIFO），而非从 top 推进。
        let mut pool = DualPool::new();

        // === 数组 A ===
        let dest_a = pool.acquire_persistent().unwrap(); // 0, top=1
        let idx_a = pool.acquire_temp().unwrap(); // 1, top=2
        let elem_a = pool.acquire_temp().unwrap(); // 2, top=3, peak=3
        assert_eq!((dest_a, idx_a, elem_a), (0, 1, 2));
        // 数组 A 构造完成：释放临时寄存器（push 到 temp_free）
        pool.release_temp(idx_a); // temp_free=[1]
        pool.release_temp(elem_a); // temp_free=[1,2]（push 顺序）
        assert_eq!(pool.temp_free, vec![1, 2]);

        // === 数组 B ===
        let dest_b = pool.acquire_persistent().unwrap(); // 3, top=4, peak=4
        // 关键验证：acquire_temp 优先 pop temp_free（LIFO：末尾先 pop），而非推进 top
        let idx_b = pool.acquire_temp().unwrap(); // pop 2（末尾）
        let elem_b = pool.acquire_temp().unwrap(); // pop 1（末尾）
        assert_eq!((dest_b, idx_b, elem_b), (3, 2, 1));
        // 复用后 temp_free 应为空，top 未因 B 的 temp 分配而推进
        assert!(pool.temp_free.is_empty());
        assert_eq!(pool.peak(), 4, "B 的 temp 复用不应推进 peak");

        // 下一个 acquire_temp 从 top=4 分配（temp_free 已空）
        let next = pool.acquire_temp().unwrap();
        assert_eq!(next, 4);
    }

    #[test]
    fn test_checkpoint_isolation_between_arrays() {
        // 模拟嵌套数组 [[1,2],[3,4]] 的 codegen：
        // 外层 A 在 codegen 中间 save_checkpoint，内层 B 完成后 restore_checkpoint，
        // 验证外层 A 继续从 checkpoint 后状态分配，且 B 释放的临时寄存器被回滚。
        let mut pool = DualPool::new();

        // === 外层数组 A ===
        let dest_a = pool.acquire_persistent().unwrap(); // 0, top=1
        let checkpoint_outer = pool.save_checkpoint(); // 1
        let idx_a = pool.acquire_temp().unwrap(); // 1, top=2

        // === 内层数组 B: [1, 2] ===
        let dest_b = pool.acquire_persistent().unwrap(); // 2, top=3
        let checkpoint_inner = pool.save_checkpoint(); // 3
        let idx_b = pool.acquire_temp().unwrap(); // 3, top=4
        let e0 = pool.acquire_persistent().unwrap(); // 4, top=5
        let e1 = pool.acquire_persistent().unwrap(); // 5, top=6, peak=6
        // 验证所有 persistent 寄存器编号符合预期（模拟 codegen 分配顺序）
        assert_eq!((dest_a, dest_b, e0, e1), (0, 2, 4, 5));
        // B 完成：释放 idx_b 进 temp_free
        pool.release_temp(idx_b); // temp_free=[3]
        assert_eq!(pool.temp_free, vec![3], "B 释放 idx_b 后 temp_free 应含 3");
        // B restore：top 回到 3，truncate 到 checkpoint_inner 的 temp_free_len=0
        pool.restore_checkpoint(checkpoint_inner);

        // 验证 1: B 释放的临时寄存器被回滚（truncate 到 0）
        assert!(
            pool.temp_free.is_empty(),
            "内层 restore 应 truncate 回滚 temp_free，B 的 idx_b 被回滚"
        );

        // 验证 2: 外层 A 继续从 checkpoint 后状态分配
        // 当前 top=3，acquire_temp 不复用已回滚的 3，而是从 top 分配
        let outer_elem = pool.acquire_temp().unwrap();
        assert_eq!(outer_elem, 3, "外层应从 top 分配，不复用已回滚的 B 寄存器");
        assert!(pool.temp_free.is_empty());

        // === 外层 A 完成 ===
        pool.release_temp(idx_a); // temp_free=[1]
        pool.release_temp(outer_elem); // temp_free=[1,3]（push 顺序）
        assert_eq!(pool.temp_free, vec![1, 3]);
        pool.restore_checkpoint(checkpoint_outer); // top=1, truncate 到 len=0

        // 最终验证：peak 保持高水位，可重新分配
        assert_eq!(pool.peak(), 6, "peak 保持高水位不下降");
        let r = pool.acquire_persistent().unwrap();
        assert_eq!(r, 1, "restore 后从 checkpoint_outer=1 重新分配");
    }

    #[test]
    fn test_release_temp_lifo_push_order() {
        // 乱序释放多个临时寄存器（5,1,3,0,4），验证 temp_free 保持 push 顺序（无排序），
        // 后续 acquire_temp 按 LIFO 弹出（4,0,3,1,5：末尾先 pop）。
        // 逐步断言内部 Vec 状态，比 test_release_temp_lifo_reuse 更严格。
        let mut pool = DualPool::new();

        // 分配 0..=5（6 个临时寄存器）
        for i in 0..=5u16 {
            assert_eq!(pool.acquire_temp().unwrap(), i);
        }
        assert_eq!(pool.peak(), 6);

        // 乱序释放：5, 1, 3, 0, 4（保留 2 不释放），逐步验证 push 顺序
        pool.release_temp(5);
        assert_eq!(pool.temp_free, vec![5]);
        pool.release_temp(1);
        assert_eq!(pool.temp_free, vec![5, 1]);
        pool.release_temp(3);
        assert_eq!(pool.temp_free, vec![5, 1, 3]);
        pool.release_temp(0);
        assert_eq!(pool.temp_free, vec![5, 1, 3, 0]);
        pool.release_temp(4);
        assert_eq!(pool.temp_free, vec![5, 1, 3, 0, 4]);

        // LIFO pop 顺序（末尾先 pop）：4, 0, 3, 1, 5
        assert_eq!(pool.acquire_temp().unwrap(), 4);
        assert_eq!(pool.acquire_temp().unwrap(), 0);
        assert_eq!(pool.acquire_temp().unwrap(), 3);
        assert_eq!(pool.acquire_temp().unwrap(), 1);
        assert_eq!(pool.acquire_temp().unwrap(), 5);

        // temp_free 耗尽后从 top=6 继续
        let next = pool.acquire_temp().unwrap();
        assert_eq!(next, 6);
    }

    // ----- Task 5: LIFO 语义单元测试（push/pop O(1)、truncate 保留 checkpoint 前寄存器）-----
    // 注意: test_release_temp_lifo_push_order 已由 Task 3.4 改造（原 descending_order_maintained），
    //       此处不再重复新增同名测试，避免编译冲突。以下 3 个测试覆盖其余 LIFO 场景。

    #[test]
    fn test_acquire_temp_lifo_pop_order() {
        // LIFO: 后 push 先 pop。与 test_release_temp_lifo_reuse 互补：
        // 后者验证 pop 返回值，此测试额外验证耗尽后从 top 继续分配。
        let mut pool = DualPool::new();
        for _ in 0..4 {
            let _ = pool.acquire_temp().unwrap();
        } // top=4
        pool.release_temp(3);
        pool.release_temp(1);
        pool.release_temp(2);
        pool.release_temp(0);
        // temp_free = [3, 1, 2, 0]，pop 顺序: 0, 2, 1, 3
        assert_eq!(pool.acquire_temp().unwrap(), 0);
        assert_eq!(pool.acquire_temp().unwrap(), 2);
        assert_eq!(pool.acquire_temp().unwrap(), 1);
        assert_eq!(pool.acquire_temp().unwrap(), 3);
        // 耗尽后从 top=4 继续
        assert_eq!(pool.acquire_temp().unwrap(), 4);
    }

    #[test]
    fn test_restore_checkpoint_truncate_preserves_earlier() {
        // truncate 保留 checkpoint 前合法释放的寄存器（修正原 clear() 语义错误的核心验证）
        let mut pool = DualPool::new();
        for _ in 0..6 {
            let _ = pool.acquire_temp().unwrap();
        } // top=6
        pool.release_temp(5); // checkpoint 前释放
        let cp = pool.save_checkpoint(); // top=6, temp_free_len=1
        pool.release_temp(3); // checkpoint 后释放
        pool.release_temp(2); // checkpoint 后释放
        assert_eq!(pool.temp_free, vec![5, 3, 2]);
        pool.restore_checkpoint(cp);
        assert_eq!(pool.temp_free, vec![5], "truncate 保留 checkpoint 前的 reg=5");
        // 下次 acquire_temp 复用 5（checkpoint 前合法释放）
        assert_eq!(pool.acquire_temp().unwrap(), 5);
    }

    #[test]
    fn test_restore_checkpoint_large_stack_o1() {
        // 验证大 temp_free 栈的 restore 为 O(1)（truncate 对 u16 仅设置 len，无 drop_in_place）
        // MAX_FUNCTION_LOCALS=4096，1000 个寄存器在限制内
        let mut pool = DualPool::new();
        for _ in 0..1000 {
            let _ = pool.acquire_temp().unwrap();
        } // top=1000
        // checkpoint 前 push 500 个
        for i in 0..500u16 {
            pool.release_temp(i);
        }
        let cp = pool.save_checkpoint(); // temp_free_len=500
        // checkpoint 后 push 500 个
        for i in 500..1000u16 {
            pool.release_temp(i);
        }
        assert_eq!(pool.temp_free.len(), 1000);
        let start = std::time::Instant::now();
        pool.restore_checkpoint(cp);
        let elapsed = start.elapsed();
        assert_eq!(pool.temp_free.len(), 500, "truncate 到 500");
        assert!(elapsed.as_micros() < 100, "restore 应 < 100μs，实际 {:?}", elapsed);
    }

    // ----- Core register pool tests (coverage expansion) -----

    #[test]
    fn test_reserve_slot_basic() {
        // Acquire one persistent slot, verify it returns reg=0 and top advances to 1
        let mut pool = DualPool::new();
        let reg = pool.acquire_persistent().unwrap();
        assert_eq!(reg, 0, "first persistent slot should be reg 0");
        assert_eq!(pool.peak(), 1, "peak should be 1 after one allocation");
    }

    #[test]
    fn test_reserve_slot_multiple_no_overlap() {
        // Acquire multiple persistent slots, verify each gets a unique, non-overlapping register
        let mut pool = DualPool::new();
        let mut regs = Vec::new();
        for i in 0..10u16 {
            let reg = pool.acquire_persistent().unwrap();
            assert_eq!(reg, i, "persistent slot {} should be reg {}", i, i);
            regs.push(reg);
        }
        // Verify all registers are distinct
        let unique: std::collections::HashSet<u16> = regs.iter().copied().collect();
        assert_eq!(unique.len(), 10, "all 10 persistent slots must be distinct");
        assert_eq!(pool.peak(), 10);
    }

    #[test]
    fn test_release_slot_then_reacquire() {
        // Acquire a temp, release it, then acquire again — should reuse the released slot
        let mut pool = DualPool::new();
        let r0 = pool.acquire_temp().unwrap(); // 0, top=1
        let r1 = pool.acquire_temp().unwrap(); // 1, top=2
        assert_eq!((r0, r1), (0, 1));
        pool.release_temp(r0); // r0=0 back to temp_free
        let r2 = pool.acquire_temp().unwrap(); // reuse 0 from temp_free (LIFO)
        assert_eq!(r2, 0, "should reuse released slot 0");
        // Verify peak unchanged — reuse doesn't push top
        assert_eq!(pool.peak(), 2);
    }

    #[test]
    fn test_allocate_temp_basic_from_top() {
        // When temp_free is empty, acquire_temp allocates from top
        let mut pool = DualPool::new();
        let r = pool.acquire_temp().unwrap();
        assert_eq!(r, 0, "first temp from top should be reg 0");
        assert_eq!(pool.peak(), 1);
    }

    #[test]
    fn test_allocate_temp_reuse_after_release() {
        // Release temp registers in specific order, verify LIFO reuse
        let mut pool = DualPool::new();
        let r0 = pool.acquire_temp().unwrap(); // 0
        let _r1 = pool.acquire_temp().unwrap(); // 1
        let r2 = pool.acquire_temp().unwrap(); // 2, top=3
        pool.release_temp(r2); // temp_free=[2]
        pool.release_temp(r0); // temp_free=[2,0]
        // LIFO pop: 0 first (last pushed), then 2, then from top
        let reused_first = pool.acquire_temp().unwrap();
        assert_eq!(reused_first, 0, "LIFO: should pop last-pushed reg 0");
        let reused_second = pool.acquire_temp().unwrap();
        assert_eq!(reused_second, 2, "LIFO: should pop reg 2 next");
        // temp_free empty, next from top=3
        let from_top = pool.acquire_temp().unwrap();
        assert_eq!(from_top, 3, "after temp_free exhausted, allocate from top");
    }

    #[test]
    fn test_persist_vs_temp_isolation_sequential() {
        // Interleave persistent and temp allocations — no register number conflicts
        let mut pool = DualPool::new();
        let p0 = pool.acquire_persistent().unwrap(); // 0
        let t0 = pool.acquire_temp().unwrap(); // 1
        let p1 = pool.acquire_persistent().unwrap(); // 2
        let t1 = pool.acquire_temp().unwrap(); // 3
        // All four must be distinct
        let all = [p0, t0, p1, t1];
        let unique: std::collections::HashSet<u16> = all.iter().copied().collect();
        assert_eq!(unique.len(), 4, "all 4 registers must be distinct");
        assert_eq!(pool.peak(), 4);
    }

    #[test]
    fn test_save_restore_checkpoint_semantics() {
        // Verify that restore_checkpoint reverts top and truncates temp_free
        let mut pool = DualPool::new();
        let _p0 = pool.acquire_persistent().unwrap(); // 0, top=1
        let cp = pool.save_checkpoint(); // top=1, temp_free_len=0
        let _t0 = pool.acquire_temp().unwrap(); // 1, top=2
        pool.release_temp(1); // temp_free=[1]
        let _t1 = pool.acquire_temp().unwrap(); // 1 from temp_free, top still 2
        let _p1 = pool.acquire_persistent().unwrap(); // 2, top=3
        // peak should be 3
        assert_eq!(pool.peak(), 3);
        // restore: top=1, temp_free truncated to len=0
        pool.restore_checkpoint(cp);
        // Next persistent should be reg=1
        let p2 = pool.acquire_persistent().unwrap();
        assert_eq!(p2, 1, "after restore, allocate from checkpoint top");
        // temp_free should be empty (truncated)
        let t2 = pool.acquire_temp().unwrap();
        assert_eq!(t2, 2, "temp from top after restore+new persist");
    }

    #[test]
    fn test_checkpoint_restores_temp_not_persist() {
        // Verify: restore only rolls back temps released after checkpoint,
        // not those released before
        let mut pool = DualPool::new();
        pool.acquire_temp().unwrap(); // 0, top=1
        pool.acquire_temp().unwrap(); // 1, top=2
        pool.release_temp(0); // temp_free=[0], len=1 (pre-checkpoint)
        let cp = pool.save_checkpoint(); // top=2, temp_free_len=1
        pool.release_temp(1); // temp_free=[0,1], len=2 (post-checkpoint)
        pool.restore_checkpoint(cp); // truncate to len=1 -> temp_free=[0]
        // Pre-checkpoint release (reg 0) is preserved; post-checkpoint (reg 1) is removed
        let reused = pool.acquire_temp().unwrap();
        assert_eq!(reused, 0, "should reuse pre-checkpoint released reg 0");
    }

    #[test]
    fn test_peak_reg_tracking_across_restore() {
        // Peak is a high-water mark — restore does not decrease it
        let mut pool = DualPool::new();
        // Allocate 1 persistent, then save checkpoint at top=1
        pool.acquire_persistent().unwrap(); // top=1
        let cp = pool.save_checkpoint(); // cp.top=1
        // Allocate 49 more to reach top=50
        for _ in 0..49 {
            pool.acquire_persistent().unwrap();
        }
        assert_eq!(pool.peak(), 50);
        // Restore to top=1
        pool.restore_checkpoint(cp);
        assert_eq!(pool.peak(), 50, "peak must not decrease after restore");
        // Allocate again — peak should stay at 50 until top exceeds it
        pool.acquire_persistent().unwrap(); // top=2
        assert_eq!(pool.peak(), 50, "peak unchanged when top < previous peak");
        // top=2 currently, need 48 more to reach top=50 (equals peak, no change)
        for _ in 2..50 {
            pool.acquire_persistent().unwrap();
        }
        assert_eq!(pool.peak(), 50, "peak unchanged when top equals previous peak");
        // top=50 now, one more makes top=51 which exceeds peak=50
        pool.acquire_persistent().unwrap(); // top=51
        assert_eq!(pool.peak(), 51, "peak increases when top exceeds previous peak");
    }

    #[test]
    fn test_large_allocation_no_panic() {
        // Allocate 100 persistent + 100 temp without panic
        let mut pool = DualPool::new();
        let mut persist_regs = Vec::new();
        for _ in 0..100 {
            persist_regs.push(pool.acquire_persistent().unwrap());
        }
        let mut temp_regs = Vec::new();
        for _ in 0..100 {
            temp_regs.push(pool.acquire_temp().unwrap());
        }
        // All 200 registers should be distinct
        let mut all_regs: Vec<u16> = persist_regs;
        all_regs.extend(temp_regs.iter().copied());
        let unique: std::collections::HashSet<u16> = all_regs.iter().copied().collect();
        assert_eq!(unique.len(), 200, "all 200 registers must be distinct");
        assert_eq!(pool.peak(), 200);
        // Release all temps and verify they can be re-acquired
        for r in &temp_regs {
            pool.release_temp(*r);
        }
        for _ in 0..100 {
            assert!(pool.acquire_temp().is_ok(), "should re-acquire after release");
        }
    }

    #[test]
    fn test_exhaustion_error_contains_diagnostics() {
        // When pool is exhausted, the error should contain count/available/peak
        let mut pool = DualPool::new();
        for _ in 0..MAX_FUNCTION_LOCALS {
            pool.acquire_persistent().unwrap();
        }
        let err = pool.acquire_persistent().unwrap_err();
        assert_eq!(err.count, MAX_FUNCTION_LOCALS);
        assert_eq!(err.available, 0);
        assert_eq!(err.peak, MAX_FUNCTION_LOCALS);
        // Verify Display output
        let msg = err.to_string();
        assert!(msg.contains(&MAX_FUNCTION_LOCALS.to_string()), "msg={msg}");
    }

    #[test]
    fn test_acquire_temp_after_persist_exhaustion() {
        // If all slots used by persistent, temp should also fail (temp_free empty, top=MAX)
        let mut pool = DualPool::new();
        for _ in 0..MAX_FUNCTION_LOCALS {
            pool.acquire_persistent().unwrap();
        }
        assert!(matches!(pool.acquire_temp(), Err(RegPoolExhausted { .. })));
    }

    #[test]
    fn test_temp_reuse_keeps_peak_low() {
        // Reusing temp registers from free list should not increase peak
        let mut pool = DualPool::new();
        for i in 0..5u16 {
            let _ = pool.acquire_temp().unwrap();
            assert_eq!(pool.peak(), i + 1);
        } // peak=5, top=5
        // Release all 5
        for i in 0..5u16 {
            pool.release_temp(i);
        }
        let peak_before = pool.peak();
        // Re-acquire 5 temps from free list — peak should not change
        for _ in 0..5 {
            let _ = pool.acquire_temp().unwrap();
        }
        assert_eq!(pool.peak(), peak_before, "reusing free list should not increase peak");
    }

    #[test]
    fn test_interleaved_persist_temp_with_checkpoint() {
        // Simulate ArrayNew pattern: dest(persist) → checkpoint → idx(temp) → elem(persist) × N → release idx → restore
        let mut pool = DualPool::new();
        let dest = pool.acquire_persistent().unwrap(); // 0, top=1
        assert_eq!(dest, 0);
        let cp = pool.save_checkpoint(); // top=1, temp_free_len=0
        let idx = pool.acquire_temp().unwrap(); // 1, top=2
        assert_eq!(idx, 1);
        let e0 = pool.acquire_persistent().unwrap(); // 2, top=3
        let e1 = pool.acquire_persistent().unwrap(); // 3, top=4
        let e2 = pool.acquire_persistent().unwrap(); // 4, top=5
        assert_eq!((e0, e1, e2), (2, 3, 4));
        // Release idx before restore
        pool.release_temp(idx); // temp_free=[1]
        // Restore: top=1, truncate temp_free to len=0
        pool.restore_checkpoint(cp);
        // Peak should be 5 (high-water mark preserved)
        assert_eq!(pool.peak(), 5);
        // Next persistent starts from checkpoint top=1
        let next = pool.acquire_persistent().unwrap();
        assert_eq!(next, 1);
    }
}

//! # 作用域管理 — 编译器作用域栈操作
//!
//! 负责局部变量的声明、解析、作用域嵌套管理和 upvalue 捕获。
//! 被 [`Compiler`](crate::compiler::Compiler) 的方法调用。
//!
//! ## 公开 API（均为 Compiler 的 impl 方法）
//!
//! - `begin_scope()` / `end_scope()` — 作用域入栈/出栈
//! - `declare_local(name)` — 声明局部变量并分配寄存器
//! - `find_local(name)` — 查找局部变量的寄存器编号
//! - `enter_scope(type)` / `exit_scope()` — 作用域信号发射封装
//! - 寄存器分配器代理方法（reserve_slot/release_slot 等）

use crate::allocator::SlotHandle;
use crate::compiler::{CompileError, Compiler};
use nuzo_signal::SlotOwner;
use nuzo_signal::{BusScope, ScopeEnteredInfo, ScopeExitedInfo};
use std::cmp::Reverse;
use std::collections::HashSet;

// ── 类型化信号键（作用域信号）────────────────────────────────────────────
nuzo_signal::declare_signal!(COMPILE_SCOPE_ENTERED_KEY, ScopeEnteredInfo, BusScope::Compiler);
nuzo_signal::declare_signal!(COMPILE_SCOPE_EXITED_KEY, ScopeExitedInfo, BusScope::Compiler);

impl Compiler {
    // ========================================================================
    // 作用域信号发射辅助方法（Scope Signal Helpers）
    // ========================================================================

    /// 进入词法作用域并发射 `COMPILE_SCOPE_ENTERED` 信号
    ///
    /// 封装 `self.scope.begin_scope()` + 信号发射，确保所有进入作用域的
    /// 代码路径都正确通知监听者。信号仅在有人订阅时发射（零开销保证）。
    ///
    /// # 参数
    /// * `scope_type` - 作用域类型描述（"block" / "function" / "loop" 等）
    pub(crate) fn enter_scope(&mut self, scope_type: &str) {
        self.scope.begin_scope();
        if let Ok(sig) = self.bus.get(&COMPILE_SCOPE_ENTERED_KEY)
            && !sig.is_empty()
        {
            sig.emit(&ScopeEnteredInfo {
                depth: self.scope.depth(),
                scope_type: scope_type.to_string(),
            });
        }
    }

    /// 退出词法作用域并发射 `COMPILE_SCOPE_EXITED` 信号
    ///
    /// 封装 `self.scope.end_scope()` + 信号发射。`depth` 为退出**前**的深度，
    /// 便于监听者识别退出的是哪一层。信号仅在有人订阅时发射。
    pub(crate) fn exit_scope(&mut self) {
        let depth_before_exit = self.scope.depth();
        self.scope.end_scope();
        if let Ok(sig) = self.bus.get(&COMPILE_SCOPE_EXITED_KEY)
            && !sig.is_empty()
        {
            sig.emit(&ScopeExitedInfo { depth: depth_before_exit });
        }
    }

    // ========================================================================
    // 新寄存器分配 API（信号槽代理方法）
    // ========================================================================

    /// 预订一组连续寄存器（委托给 RegisterAllocator）
    ///
    /// # 参数
    /// * `count` - 需要的连续寄存器数量
    /// * `owner` - 槽位所有者标识
    ///
    /// # 返回值
    /// * `Ok(SlotHandle)` - 槽位句柄，用于后续释放或查询
    /// * `Err(CompileError)` - 寄存器耗尽等错误
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn reserve_slot(
        &mut self,
        count: u16,
        owner: SlotOwner,
    ) -> Result<SlotHandle, CompileError> {
        let handle = self.allocator.reserve_slot(count, owner)?;
        // 同步旧字段（迁移期兼容）
        let allocator_peak = self.allocator.peak_reg();
        if allocator_peak > self.next_reg {
            self.next_reg = allocator_peak;
        }
        self.peak_reg = self.peak_reg.max(allocator_peak);
        Ok(handle)
    }

    /// 释放单个槽位（委托给 RegisterAllocator）
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn release_slot(&mut self, handle: SlotHandle) {
        self.allocator.release_slot(handle)
    }

    /// 分配单个寄存器（新 API 快捷方法，替代 alloc_register）
    ///
    /// 返回分配到的寄存器编号（u16），内部使用 SlotOwner::TempExpr。
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn alloc_single(&mut self, owner: SlotOwner) -> Result<u16, CompileError> {
        let reg = self.allocator.alloc_single(owner)?;
        // 同步旧字段（迁移期兼容）
        let allocator_peak = self.allocator.peak_reg();
        if allocator_peak > self.next_reg {
            self.next_reg = allocator_peak;
        }
        self.peak_reg = self.peak_reg.max(allocator_peak);
        Ok(reg)
    }

    /// 进入分配作用域（委托给 RegisterAllocator）
    pub(crate) fn begin_alloc_scope(&mut self) {
        self.allocator.begin_scope()
    }

    /// 退出分配作用域并释放该深度的槽位（委托给 RegisterAllocator）
    pub(crate) fn end_alloc_scope(&mut self) {
        self.allocator.end_scope()
    }

    /// 按深度批量释放槽位（委托给 RegisterAllocator）
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn release_slots_by_depth(&mut self, target_depth: usize) {
        self.allocator.release_slots_by_depth(target_depth)
    }

    /// 获取槽位的寄存器范围 (start, end)
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn slot_range(&self, handle: SlotHandle) -> (u16, u16) {
        self.allocator.slot_range(handle)
    }

    /// 远端基地址分配（专门用于数组/对象构造）
    ///
    /// 强制从 next_reg 分配连续寄存器，不复用已释放的低地址寄存器。
    #[allow(dead_code)] // 新寄存器分配 API 代理方法，保留供后续迁移期使用
    pub(crate) fn reserve_remote(
        &mut self,
        count: u16,
        owner: SlotOwner,
    ) -> Result<SlotHandle, CompileError> {
        let handle = self.allocator.reserve_remote(count, owner)?;
        let allocator_peak = self.allocator.peak_reg();
        if allocator_peak > self.next_reg {
            self.next_reg = allocator_peak;
        }
        self.peak_reg = self.peak_reg.max(allocator_peak);
        Ok(handle)
    }

    // ========================================================================
    // 作用域管理（Scope Management）
    // ========================================================================

    /// Begin a new scope (for block scoping)
    #[allow(dead_code)] // 直接调用已由 enter_scope() 替代，保留供底层直接操作
    fn begin_scope(&mut self) {
        self.scope.begin_scope();
    }

    /// End current scope
    #[allow(dead_code)] // 直接调用已由 exit_scope() 替代，保留供底层直接操作
    fn end_scope(&mut self) {
        self.scope.end_scope();
    }

    // ========================================================================
    // 变量管理（Variable Management Helpers）
    // ========================================================================

    /// Declare a new local variable
    ///
    /// Allocates a register and binds the name to it.
    pub(crate) fn declare_local(&mut self, name: String) -> Result<u16, CompileError> {
        let reg = self.alloc_register()?;
        self.scope.define(&name, reg);
        Ok(reg)
    }

    /// Find a local variable by name
    ///
    /// Searches from innermost scope outward.
    /// Returns Some(register_index) if found, None otherwise.
    #[allow(dead_code)] // 变量查询 API，保留供调试/后续功能使用
    pub(crate) fn find_local(&self, name: &str) -> Option<u16> {
        match self.scope.resolve(name) {
            Some(nuzo_bytecode::scope::ScopeKind::Local(reg)) => Some(reg),
            _ => None,
        }
    }

    // ========================================================================
    // 寄存器管理（Register Management with reuse optimization）
    // ========================================================================

    /// Release registers back to the free pool
    ///
    /// Call this when leaving a scope to allow register reuse.
    /// Only releases registers that are NOT active locals in the current scope,
    /// preventing premature freeing of registers still referenced by outer scopes.
    pub(crate) fn release_registers(&mut self, from_reg: u16) {
        if from_reg >= self.next_reg {
            return;
        }

        let active_locals: HashSet<u16> = self.scope.active_locals().into_iter().collect();

        for reg in from_reg..self.next_reg {
            if !active_locals.contains(&reg)
                && !self.free_registers.iter().any(|&Reverse(r)| r == reg)
            {
                self.free_registers.push(Reverse(reg));
            }
        }

        while self.next_reg > from_reg && self.next_reg > self.reserve_watermark {
            let prev = self.next_reg - 1;
            let prev_is_local = active_locals.contains(&prev);
            let prev_is_free = self.free_registers.iter().any(|Reverse(r)| *r == prev);

            if prev_is_local || !prev_is_free {
                break;
            }

            self.next_reg = prev;
            let kept: Vec<_> =
                self.free_registers.drain().filter(|&Reverse(r)| r != prev).collect();
            self.free_registers.extend(kept);
        }
    }

    /// Save current register state for scope tracking
    pub(crate) fn save_register_state(&self) -> u16 {
        self.next_reg
    }

    /// Release a single temporary register (expression-level optimization)
    ///
    /// Respects `reserve_watermark`: the shrink loop will not reduce `next_reg`
    /// below the watermark, protecting pre-allocated contiguous regions.
    pub(crate) fn release_temp_register(&mut self, reg: u16) {
        if reg >= self.next_reg {
            return;
        }

        let active_locals: HashSet<u16> = self.scope.active_locals().into_iter().collect();
        if active_locals.contains(&reg) {
            return;
        }

        if !self.free_registers.iter().any(|&Reverse(r)| r == reg) {
            self.free_registers.push(Reverse(reg));
        }

        if reg == self.next_reg.saturating_sub(1) {
            while self.next_reg > self.reserve_watermark {
                let prev = self.next_reg.saturating_sub(1);
                let prev_is_local = active_locals.contains(&prev);
                let prev_is_free = self.free_registers.iter().any(|Reverse(r)| *r == prev);

                if prev_is_local || !prev_is_free {
                    break;
                }

                self.next_reg = prev;
                let kept: Vec<_> =
                    self.free_registers.drain().filter(|&Reverse(r)| r != prev).collect();
                self.free_registers.extend(kept);
            }
        }
    }

    /// Allocate next available register with intelligent reuse
    ///
    /// Strategy:
    /// 1. First try to reuse a freed register from the pool
    /// 2. Otherwise allocate a new register incrementially
    ///
    /// ## LSRA 集成（Def 点记录）
    ///
    /// 当编译器启用 LSRA 模式时（`use_lsra == true`），每次成功分配寄存器后
    /// 自动调用 `note_vreg_def()` 记录定义点。
    pub(crate) fn alloc_register(&mut self) -> Result<u16, CompileError> {
        if let Some(Reverse(reg)) = self.free_registers.pop() {
            self.peak_reg = self.peak_reg.max(reg + 1);
            self.note_vreg_def(reg);
            return Ok(reg);
        }

        if self.next_reg >= nuzo_core::MAX_FUNCTION_LOCALS {
            return Err(CompileError::TooManyLocals {
                count: self.next_reg as usize + 1,
                line: self.current_line,
                column: self.current_column,
            });
        }

        let reg = self.next_reg;
        self.next_reg += 1;
        self.peak_reg = self.peak_reg.max(self.next_reg);
        self.note_vreg_def(reg);
        Ok(reg)
    }

    /// Update peak_reg after any external next_reg modification
    #[allow(dead_code)] // 寄存器峰值同步 API，保留供外部修改 next_reg 后调用
    pub(crate) fn update_peak_reg(&mut self) {
        self.peak_reg = self.peak_reg.max(self.next_reg);
    }
}

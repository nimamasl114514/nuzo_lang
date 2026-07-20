//! # 字符串常量池与全局变量 — 编译期字符串驻留和赋值
//!
//! 负责源码中的字符串和标识符常量化、全局变量读写。
//! 避免重复存储相同的字符串常量。
//!
//! ## 公开 API（均为 Compiler 的 impl 方法）
//!
//! - `add_constant_checked(value)` — 将值添加到常量池（带边界检查）
//! - `emit_set_global(name, val_reg)` — 发射 SetGlobal 指令
//! - `compile_assign_target(target, val_reg)` — 编译赋值目标（Ident/Index/Field）

use crate::compiler::{CompileError, Compiler};
use crate::macros::emit_typed;
use nuzo_frontend::ast;
use nuzo_values::ValueExt;

impl Compiler {
    // ========================================================================
    // 字符串常量池管理（String Constant Pool Management）
    // ========================================================================

    /// Add a constant to the pool with u16 index boundary check
    ///
    /// Returns the constant index as u16, or ConstantPoolOverflow error
    /// if the constant pool exceeds u16::MAX entries.
    ///
    /// C1 增强: 使用 `try_add_constant` (Result API) 替代 `add_constant` + 手动检查。
    /// 原实现中 `add_constant` 会在溢出时 panic,导致后续的 `u16::MAX` 检查成为死代码;
    /// 现在改用 `try_add_constant`,溢出时返回 `ChunkError` 并映射为 `CompileError`,
    /// 使常量池溢出能被上层优雅处理而非 panic。
    pub(crate) fn add_constant_checked(
        &mut self,
        value: nuzo_core::Value,
    ) -> Result<u16, CompileError> {
        let idx = self.chunk.try_add_constant(value).map_err(|e| match e {
            nuzo_bytecode::ChunkError::ConstantPoolOverflow { count } => {
                CompileError::ConstantPoolOverflow {
                    count,
                    line: self.current_line,
                    column: self.current_column,
                }
            }
        })?;
        Ok(idx as u16)
    }

    // ========================================================================
    // 赋值编译辅助方法（Assignment Compilation Helpers）
    // ========================================================================

    /// 发射 SetGlobal 指令：将寄存器中的值写入全局变量。
    ///
    /// 全局变量通过名称字符串在常量池中索引。
    /// 此方法将名称添加到常量池并发射 SetGlobal 指令。
    ///
    /// # 参数
    ///
    /// * `name`：全局变量名称
    /// * `val_reg`：值所在的寄存器编号
    pub(crate) fn emit_set_global(&mut self, name: &str, val_reg: u16) -> Result<(), CompileError> {
        let name_idx = self.add_constant_checked(nuzo_core::Value::from_string(name))?;
        emit_typed!(self, SetGlobal, val_reg, name_idx);
        Ok(())
    }

    /// 编译赋值目标：根据目标类型发射对应的赋值指令。
    ///
    /// 支持三种赋值目标：
    /// - **Ident**：局部变量（Mov）、捕获变量（SetCaptured）或全局变量（SetGlobal）
    /// - **Index**：索引赋值（SetIndex），如 `arr[i] = x`
    /// - **Field**：属性赋值（SetProp），如 `obj.prop = x`
    ///
    /// # 参数
    ///
    /// * `target`：赋值目标 AST 节点
    /// * `val_reg`：值所在的寄存器编号
    pub(crate) fn compile_assign_target(
        &mut self,
        target: &ast::AssignTarget,
        val_reg: u16,
    ) -> Result<(), CompileError> {
        match target {
            ast::AssignTarget::Ident { name } => {
                // 优先级 1：局部变量 → Mov
                let local_reg = self.scope.resolve(name).and_then(|k| {
                    if let nuzo_bytecode::scope::ScopeKind::Local(reg) = k {
                        Some(reg)
                    } else {
                        None
                    }
                });
                if let Some(local_reg) = local_reg {
                    self.emit_mov(local_reg, val_reg);
                    return Ok(());
                }

                // 优先级 2：闭包捕获变量 → SetCaptured
                let capture_idx = self
                    .current_captured
                    .as_ref()
                    .and_then(|caps| caps.iter().find(|c| c.name == *name))
                    .map(|info| info.capture_index);

                if let Some(cap_idx) = capture_idx {
                    emit_typed!(self, SetCaptured, cap_idx as u16, val_reg);
                    return Ok(());
                }

                // 优先级 3：全局变量 → SetGlobal
                self.emit_set_global(name, val_reg)?;
            }
            ast::AssignTarget::Index { object, index } => {
                let is_simple_ident = matches!(object.as_ref(), ast::Expr::Ident { .. });

                let obj_reg = self.compile_expr(object)?;
                let idx_reg = self.compile_expr(index)?;

                if is_simple_ident {
                    emit_typed!(self, SetIndexMut, obj_reg, idx_reg, val_reg);
                } else {
                    emit_typed!(self, SetIndex, obj_reg, idx_reg, val_reg);

                    if let ast::Expr::Ident { name, .. } = object.as_ref()
                        && self.scope.resolve(name).is_none()
                    {
                        self.emit_set_global(name, obj_reg)?;
                    }
                }

                self.release_temp_register(idx_reg);
                self.release_temp_register(obj_reg);
            }
            ast::AssignTarget::Field { object, name } => {
                let global_name = if let ast::Expr::Ident { name: obj_name, .. } = object.as_ref() {
                    if self.scope.resolve(obj_name).is_none() {
                        Some(obj_name.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                let obj_reg = self.compile_expr(object)?;
                let prop_idx = self.add_constant_checked(nuzo_core::Value::from_string(name))?;
                emit_typed!(self, SetProp, obj_reg, prop_idx, val_reg);

                if let Some(ref gname) = global_name {
                    self.emit_set_global(gname, obj_reg)?;
                }

                self.release_temp_register(obj_reg);
            }
        }
        Ok(())
    }

    // ========================================================================
    // InitModule 发射（lazy import 模块初始化）
    // ========================================================================

    /// 为模块路径分配 `init_flag_slot`。
    ///
    /// 同一模块路径（字符串完全匹配）多次导入时共享同一 slot，
    /// 确保 VM `globals[slot]` 标志位唯一，模块顶层代码只执行一次。
    ///
    /// # 返回值
    /// 该模块路径对应的 slot 索引（u16，从 0 递增）。
    pub(crate) fn allocate_init_flag_slot(&mut self, path: &str) -> u16 {
        if let Some(&slot) = self.init_flag_slots.get(path) {
            return slot;
        }
        let slot = self.next_init_flag_slot;
        // slot 递增；u16 溢出时回绕到 0（实践中 65535 个模块不会触发）
        self.next_init_flag_slot = self.next_init_flag_slot.wrapping_add(1);
        self.init_flag_slots.insert(path.to_string(), slot);
        slot
    }

    /// 发射 `InitModule` 字节码指令。
    ///
    /// 字节码格式（共 5 字节）：
    /// ```text
    /// [Opcode::InitModule] [module_idx:u16 LE] [init_flag_slot:u16 LE]
    /// ```
    ///
    /// # 参数
    /// - `path`: 模块路径字符串（添加到常量池，`module_idx` 指向它）
    /// - `slot`: `init_flag_slot`（由 [`allocate_init_flag_slot`](Self::allocate_init_flag_slot) 分配）
    /// - `line`: 源码行号（用于调试信息）
    pub(crate) fn emit_init_module(
        &mut self,
        path: &str,
        slot: u16,
        line: usize,
    ) -> Result<(), CompileError> {
        let module_idx = self.add_constant_checked(nuzo_core::Value::from_string(path))?;
        self.emit_opcode(nuzo_bytecode::Opcode::InitModule, line);
        self.emit_u16(module_idx);
        self.emit_u16(slot);
        Ok(())
    }

    /// Flush 所有 pending lazy imports（发射它们的 `InitModule`）。
    ///
    /// # v1 策略
    /// 在编译单元结束时（`Halt` 之前）统一发射所有 pending lazy import。
    /// 这不是"首次引用时发射"的精确语义，但保证了 lazy import 的 `InitModule`
    /// 不在 import 语句位置立即发射（与 eager import 区分开）。
    ///
    /// 精确的"首次引用时发射"需要符号→模块映射（`ImportRecord.resolved_symbols`），
    /// 将在 IR 路径集成中实现。
    pub(crate) fn flush_pending_lazy_imports(&mut self) -> Result<(), CompileError> {
        let pending = std::mem::take(&mut self.pending_lazy_imports);
        for imp in pending {
            self.emit_init_module(&imp.path, imp.slot, imp.line)?;
        }
        Ok(())
    }
}

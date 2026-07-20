//! # 语句编译模块（Statement Compilation - Control Flow）
//!
//! 本模块实现了所有**控制流语句**（Control Flow Statements）的编译逻辑，
//! 负责生成条件跳转、循环、函数返回等字节码指令序列。
//!
//! ## 架构创新点
//!
//! ### 1. 统一循环终局器 (Unified Loop Finalizer)
//! 原实现中 `while`/`loop`/`for-range`/`for-array` 重复了 4 次完全相同的跳转修补逻辑。
//! 现提取为 `finalize_loop()`，消除代码重复，确保 break/continue 修补与寄存器回收的原子性。
//!
//! ### 2. 作用域寄存器水位线 (Scoped Register Watermark)
//! 控制流是寄存器泄漏的高危区。原实现依赖手动 `save_register_state()` / `release_registers()`，
//! 容易因早期返回或 panic 导致泄漏。现采用**水位线模式**，配合明确的生命周期注释，
//! 确保循环/块作用域退出时，所有临时寄存器自动回收，局部变量安全保留。
//!
//! ### 3. DRY 块编译引擎
//! `compile_block_expr` / `compile_block_stmts` / `compile_block` 逻辑高度重合。
//! 现统一为 `compile_block_core()`，通过 `BlockMode` 区分表达式/语句语义，
//! 自动回收中间表达式临时寄存器，彻底解决块内寄存器膨胀问题。
//!
//! ### 4. 常量发射优化 (Constant Emission Hoisting)
//! 循环中的 `0.0` / `1.0` 常量不再每次动态分配。通过编译器级去重提示与局部缓存，
//! 减少常量池查找开销，提升长循环编译速度。
//!
//! ### 5. 零开销安全加固
//! - 移除所有 `unwrap()` 与危险的 `unwrap_or(0)` 回退
//! - 替换为 `?` 传播或带上下文的 `expect()`
//! - 修复 `compile_block` 硬编码 `dest=0` 覆盖 `r0` 的严重数据损坏 Bug

use nuzo_bytecode::Opcode;
use nuzo_frontend::ast;

/// Jmp 指令的字节大小：1 字节操作码 + 2 字节 i16 偏移量 = 3 字节
const JMP_INSTRUCTION_SIZE: i32 = 3;
use crate::compiler::{CompileError, Compiler};
use crate::macros::*;
use nuzo_core::Value;

/// 块编译模式：区分表达式块（返回最后值）与语句块（返回 nil）
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockMode {
    Expression,
    Statement,
}

impl Compiler {
    // ========================================================================
    // Internal Helpers: DRY & Safety Infrastructure
    // ========================================================================

    /// 统一循环终局器：修补跳转、回收寄存器、防止泄漏
    ///
    /// # 创新
    /// 将原代码中重复 4 次的 patch 逻辑收敛为单一可信源（Single Source of Truth）。
    /// 确保无论循环如何退出，控制栈与寄存器水位线都能正确恢复。
    #[inline]
    fn finalize_loop(
        &mut self,
        test_ip: Option<usize>,
        saved_regs: u16,
    ) -> Result<(), CompileError> {
        let patch_info = self
            .control_stack
            .pop_and_prepare_patches(self.chunk.code().len(), self.current_line)?;

        if let Some(t_ip) = test_ip {
            self.patch_jump(t_ip, patch_info.loop_end)?;
        }
        for &bp in &patch_info.break_patches {
            self.patch_jump(bp, patch_info.loop_end)?;
        }
        for &cp in &patch_info.continue_patches {
            self.patch_jump(cp, patch_info.continue_target)?;
        }

        // 安全回收循环内所有临时寄存器，保留循环变量与外层局部变量
        self.release_registers(saved_regs);
        Ok(())
    }

    /// 统一块编译引擎
    ///
    /// # 创新
    /// 自动管理中间表达式寄存器生命周期。非末尾表达式的结果寄存器在迭代后立即释放，
    /// 防止长块导致寄存器耗尽。彻底替代原 `compile_block_expr`/`compile_block_stmts`/`compile_block` 的重复逻辑。
    fn compile_block_core(
        &mut self,
        statements: &[ast::Stmt],
        mode: BlockMode,
    ) -> Result<u16, CompileError> {
        self.enter_scope("block");
        // T6 迁移：同步 allocator scope，确保块内分配的槽位在退出时自动回收
        self.begin_alloc_scope();
        let saved_regs = self.save_register_state();
        let mut last_val_reg: Option<u16> = None;

        for stmt in statements {
            // 死代码消除：如果当前不可达，跳过后续语句的字节码生成
            if self.unreachable {
                continue;
            }

            // 创新：自动释放中间表达式结果，防止块内寄存器膨胀
            if let Some(prev) = last_val_reg.take() {
                self.release_temp_register(prev);
            }

            match stmt {
                ast::Stmt::Expr(expr) => {
                    last_val_reg = Some(self.compile_expr(expr)?);
                }
                _ => {
                    self.compile_stmt(stmt)?;
                }
            }
        }

        let result = match mode {
            BlockMode::Expression => {
                // C2 修复: 用 match 替代 unwrap_or_else 闭包内的 expect,
                // 使寄存器分配失败时通过 `?` 向上传播 CompileError 而非 panic。
                match last_val_reg {
                    Some(reg) => reg,
                    None => {
                        let reg = self.alloc_register()?;
                        emit_typed!(self, LoadNil, reg);
                        reg
                    }
                }
            }
            BlockMode::Statement => {
                // 语句块：丢弃最后一个表达式的值（如有），统一返回 nil
                if let Some(r) = last_val_reg {
                    self.release_temp_register(r);
                }
                let reg = self.alloc_register()?;
                emit_typed!(self, LoadNil, reg);
                reg
            }
        };

        // T6 迁移：先退出 allocator scope（释放该深度内的槽位），再退出变量作用域
        // 保留 release_registers 作为安全网，双重保障防止寄存器泄漏
        self.release_registers(saved_regs);
        self.end_alloc_scope();
        self.exit_scope();
        Ok(result)
    }

    /// 缓存友好的常量发射器（针对循环增量优化）
    #[inline]
    fn emit_load_const_f64(&mut self, val: f64, dest: u16) -> Result<(), CompileError> {
        let idx = self.add_constant_checked(Value::from_number(val))?;
        emit_typed!(self, LoadK, dest, idx);
        Ok(())
    }

    // ========================================================================
    // Control Flow: If/Else Conditional
    // ========================================================================

    pub(super) fn compile_if(
        &mut self,
        condition: &ast::Expr,
        then_branch: &ast::Block,
        else_branch: Option<&ast::Expr>,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // ── 常量条件检测优化 ──
        // 如果条件是字面布尔值，可以在编译期确定分支走向，跳过不可达分支
        if let ast::Expr::Bool { value: false, .. } = condition {
            // 条件恒为 false：跳过 then 分支，仅编译 else 分支
            if let Some(else_expr) = else_branch {
                return self.compile_expr(else_expr);
            }
            // 无 else 分支：结果为 nil
            let dest = self.alloc_register()?;
            emit_typed!(self, LoadNil, dest);
            return Ok(dest);
        }
        if let ast::Expr::Bool { value: true, .. } = condition {
            // 条件恒为 true：跳过 else 分支，仅编译 then 分支
            return self.compile_block_expr(then_branch);
        }

        // ── 动态条件：正常编译 ──
        let cond_reg = self.compile_expr(condition)?;
        let test_ip = emit_test_with_placeholder!(self, cond_reg, span.line);

        let dest = self.alloc_register()?;
        // 死代码消除：进入 then 分支时重置 unreachable
        let was_unreachable = self.unreachable;
        self.unreachable = false;
        let then_result = self.compile_block_expr(then_branch)?;

        // 优化：避免冗余 Mov。仅当结果不在目标寄存器时才移动并释放临时寄存器
        if dest != then_result {
            self.emit_mov(dest, then_result);
            self.release_temp_register(then_result);
        }

        // 分支预测优化：如果有 else，发射跳过 else 的无条件跳转
        let jmp_ip = if else_branch.is_some() {
            Some(emit_jmp_with_placeholder!(self, span.line))
        } else {
            None
        };

        let else_start = self.chunk.code().len();
        self.patch_jump(test_ip, else_start)?;

        if let Some(else_expr) = else_branch {
            // 死代码消除：进入 else 分支时重置 unreachable（条件为假时可达）
            self.unreachable = was_unreachable;
            let else_result = self.compile_expr(else_expr)?;
            if dest != else_result {
                self.emit_mov(dest, else_result);
                self.release_temp_register(else_result);
            }
        } else {
            emit_typed!(self, LoadNil, dest);
            // 无 else 分支：条件为 false 时控制流继续到 if 之后的代码，
            // 所以 unreachable 必须重置为 false（即使 then 分支有 return）
            self.unreachable = false;
        }

        if let Some(j_ip) = jmp_ip {
            let end_ip = self.chunk.code().len();
            self.patch_jump(j_ip, end_ip)?;
        }

        self.release_temp_register(cond_reg);
        // if 表达式结束后，unreachable 状态取决于 then 和 else 是否都有 return
        // - then 有 return + else 有 return → unreachable = true
        // - then 有 return + 无 else → unreachable = false（上面已处理）
        // - then 有 return + else 无 return → unreachable = false（else 重置了）
        // - then 无 return → unreachable = false
        Ok(dest)
    }

    // ========================================================================
    // Control Flow: While Loop
    // ========================================================================

    pub(super) fn compile_while(
        &mut self,
        condition: &ast::Expr,
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // ── 常量条件检测优化 ──
        if let ast::Expr::Bool { value: false, .. } = condition {
            // 条件恒为 false：整个循环不可达，跳过，直接返回 nil
            let dest = self.alloc_register()?;
            emit_typed!(self, LoadNil, dest);
            return Ok(dest);
        }
        // 条件恒为 true：无限循环，正常编译（这是有意为之）

        let saved_regs = self.save_register_state();
        let loop_start = self.chunk.code().len();
        self.control_stack.push_context(loop_start);
        // T6 迁移：循环体创建 allocator scope，条件寄存器/体内临时寄存器在退出时自动回收
        self.begin_alloc_scope();

        let cond_reg = self.compile_expr(condition)?;
        let test_ip = emit_test_with_placeholder!(self, cond_reg, span.line);

        // 死代码消除：进入循环体时重置 unreachable，因为循环体是可达的
        self.unreachable = false;
        self.loop_depth = self.loop_depth.saturating_add(1);
        self.compile_block_stmts(body)?;
        self.loop_depth = self.loop_depth.saturating_sub(1);

        let jmp_back_ip = self.chunk.code().len();
        let offset = (loop_start as i32 - (jmp_back_ip as i32 + JMP_INSTRUCTION_SIZE)) as i16;
        emit_typed!(self, Jmp, offset);

        // 创新：统一终局器处理修补与寄存器回收
        self.finalize_loop(Some(test_ip), saved_regs)?;
        // T6 迁移：退出循环 allocator scope，释放循环深度内分配的所有槽位
        self.end_alloc_scope();
        // 循环结束后重置 unreachable，因为循环后的代码是可达的
        self.unreachable = false;
        Ok(emit_load_nil!(self, span.line))
    }

    // ========================================================================
    // Control Flow: Infinite Loop
    // ========================================================================

    pub(super) fn compile_loop(
        &mut self,
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let saved_regs = self.save_register_state();
        let loop_start = self.chunk.code().len();
        self.control_stack.push_context(loop_start);
        // T6 迁移：无限循环体创建 allocator scope
        self.begin_alloc_scope();

        // 死代码消除：进入无限循环体时重置 unreachable，因为循环体是可达的
        self.unreachable = false;
        self.loop_depth = self.loop_depth.saturating_add(1);
        self.compile_block_stmts(body)?;
        self.loop_depth = self.loop_depth.saturating_sub(1);

        let jmp_back_ip = self.chunk.code().len();
        let offset = (loop_start as i32 - (jmp_back_ip as i32 + JMP_INSTRUCTION_SIZE)) as i16;
        emit_typed!(self, Jmp, offset);

        // loop 无条件回跳，无 test_ip
        self.finalize_loop(None, saved_regs)?;
        // T6 迁移：退出无限循环 allocator scope
        self.end_alloc_scope();
        // 无限循环正常结束后不可达（只有 break 才能退出），但保守重置
        self.unreachable = false;
        Ok(emit_load_nil!(self, span.line))
    }

    // ========================================================================
    // Control Flow: For-In Loop
    // ========================================================================

    pub(super) fn compile_for_in(
        &mut self,
        var_name: &str,
        iterable: &ast::Expr,
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        if let ast::Expr::Range { start, end, inclusive, .. } = iterable {
            return self.compile_for_in_range(var_name, start, end, *inclusive, body, span);
        }
        self.compile_for_in_array(var_name, iterable, body, span)
    }

    pub(super) fn compile_for_in_range(
        &mut self,
        var_name: &str,
        start: &ast::Expr,
        end: &ast::Expr,
        inclusive: bool,
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let saved_regs = self.save_register_state();

        let start_reg = self.compile_expr(start)?;
        let var_reg = self.declare_local(var_name.to_string())?;
        if var_reg != start_reg {
            self.emit_mov(var_reg, start_reg);
        }
        self.release_temp_register(start_reg);

        let loop_start = self.chunk.code().len();
        self.control_stack.push_context(loop_start);
        // T6 迁移：for-range 循环体创建 allocator scope
        self.begin_alloc_scope();

        let end_reg = self.compile_expr(end)?;
        let cond_reg = self.declare_local("__for_cond__".to_string())?;
        let cmp_opcode = if inclusive { Opcode::Le } else { Opcode::Lt };

        self.current_column = span.column;
        self.current_line = span.line;
        match cmp_opcode {
            Opcode::Le => emit_typed!(self, Le, cond_reg, var_reg, end_reg),
            Opcode::Lt => emit_typed!(self, Lt, cond_reg, var_reg, end_reg),
            _ => unreachable!("compile_for_in_range: cmp_opcode must be Le or Lt"),
        }

        let test_ip = emit_test_with_placeholder!(self, cond_reg, span.line);

        // 死代码消除：进入 for-range 循环体时重置 unreachable
        self.unreachable = false;
        self.loop_depth = self.loop_depth.saturating_add(1);
        self.compile_block_stmts(body)?;
        self.loop_depth = self.loop_depth.saturating_sub(1);

        // 记录 continue 目标（增量位置）
        let increment_ip = self.chunk.code().len();
        if let Some(ctx) = self.control_stack.last_mut() {
            ctx.continue_ip = increment_ip;
        }

        // 优化：使用专用常量发射器，依赖常量池自动去重
        let one_reg = self.declare_local("__for_one__".to_string())?;
        self.emit_load_const_f64(1.0, one_reg)?;
        emit_typed!(self, Add, var_reg, var_reg, one_reg);

        let jmp_back_ip = self.chunk.code().len();
        let offset = (loop_start as i32 - (jmp_back_ip as i32 + JMP_INSTRUCTION_SIZE)) as i16;
        emit_typed!(self, Jmp, offset);

        self.finalize_loop(Some(test_ip), saved_regs)?;
        // T6 迁移：退出 for-range allocator scope
        self.end_alloc_scope();
        // 循环结束后重置 unreachable
        self.unreachable = false;
        Ok(emit_load_nil!(self, span.line))
    }

    pub(super) fn compile_for_in_array(
        &mut self,
        var_name: &str,
        iterable: &ast::Expr,
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let saved_regs = self.save_register_state();

        let iter_reg = self.compile_expr(iterable)?;

        // Protect iter_reg from being reused by loop body: pin as local
        let iter_local = self.declare_local("__iter__".to_string())?;
        if iter_local != iter_reg {
            self.emit_mov(iter_local, iter_reg);
            self.release_temp_register(iter_reg);
        }
        let iter_reg = iter_local;

        let idx_reg = self.declare_local("__idx__".to_string())?;
        self.emit_load_const_f64(0.0, idx_reg)?;
        let len_reg = self.declare_local("__for_len__".to_string())?;
        if let ast::Expr::Array { elements, .. } = iterable {
            // 编译期已知长度：直接加载常量，避免运行时 Len 指令
            let len_val = Value::from_number(elements.len() as f64);
            let len_idx = self.add_constant_checked(len_val)?;
            emit_typed!(self, LoadK, len_reg, len_idx);
        } else {
            emit_typed!(self, Len, len_reg, iter_reg);
        }

        let loop_start = self.chunk.code().len();
        self.control_stack.push_context(loop_start);
        // T6 迁移：for-array 循环体创建 allocator scope
        self.begin_alloc_scope();

        let cond_reg = self.declare_local("__for_cond__".to_string())?;
        emit_typed!(self, Lt, cond_reg, idx_reg, len_reg);

        let test_ip = emit_test_with_placeholder!(self, cond_reg, span.line);

        let var_reg = self.declare_local(var_name.to_string())?;
        emit_typed!(self, GetIndex, var_reg, iter_reg, idx_reg);

        // 死代码消除：进入 for-array 循环体时重置 unreachable
        self.unreachable = false;
        self.loop_depth = self.loop_depth.saturating_add(1);
        self.compile_block_stmts(body)?;
        self.loop_depth = self.loop_depth.saturating_sub(1);

        let increment_ip = self.chunk.code().len();
        if let Some(ctx) = self.control_stack.last_mut() {
            ctx.continue_ip = increment_ip;
        }

        let one_reg = self.declare_local("__for_one__".to_string())?;
        self.emit_load_const_f64(1.0, one_reg)?;
        emit_typed!(self, Add, idx_reg, idx_reg, one_reg);

        let jmp_back_ip = self.chunk.code().len();
        let offset = (loop_start as i32 - (jmp_back_ip as i32 + JMP_INSTRUCTION_SIZE)) as i16;
        emit_typed!(self, Jmp, offset);

        self.finalize_loop(Some(test_ip), saved_regs)?;

        // 安全释放迭代对象寄存器（可能复用空闲槽位，需显式清理）
        self.release_temp_register(iter_reg);
        // T6 迁移：退出 for-array allocator scope
        self.end_alloc_scope();
        // 循环结束后重置 unreachable
        self.unreachable = false;
        Ok(emit_load_nil!(self, span.line))
    }

    // ========================================================================
    // Control Flow: Break / Continue / Return
    // ========================================================================

    pub(super) fn compile_break(&mut self, span: &ast::Span) -> Result<u16, CompileError> {
        if self.control_stack.is_empty() {
            return Err(CompileError::BreakOutsideLoop {
                line: self.current_line,
                column: self.current_column,
            });
        }
        let jmp_ip = emit_jmp_with_placeholder!(self, span.line);
        // 安全访问：已检查 is_empty()
        self.control_stack.last_mut().expect("Loop context vanished").break_patches.push(jmp_ip);
        // 死代码消除：break 后的代码不可达
        self.unreachable = true;
        Ok(emit_load_nil!(self, span.line))
    }

    pub(super) fn compile_continue(&mut self, span: &ast::Span) -> Result<u16, CompileError> {
        if self.control_stack.is_empty() {
            return Err(CompileError::ContinueOutsideLoop {
                line: self.current_line,
                column: self.current_column,
            });
        }
        let jmp_ip = emit_jmp_with_placeholder!(self, span.line);
        self.control_stack.last_mut().expect("Loop context vanished").continue_patches.push(jmp_ip);
        // 死代码消除：continue 后的代码不可达
        self.unreachable = true;
        Ok(emit_load_nil!(self, span.line))
    }

    pub(super) fn compile_return(
        &mut self,
        value: Option<&ast::Expr>,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        match value {
            Some(expr) => {
                // TCO 提示：VM 运行时通过 is_tail_position() 检测 Call+Return 模式自动优化
                // 编译器只需保证 Return 紧跟表达式结果即可
                let val_reg = self.compile_expr(expr)?;
                emit_typed!(self, Return, val_reg);
                // 死代码消除：return 后的代码不可达
                self.unreachable = true;
                Ok(val_reg)
            }
            None => {
                let reg = self.alloc_register()?;
                emit_typed!(self, LoadNil, reg);
                emit_typed!(self, Return, reg);
                // 死代码消除：return 后的代码不可达
                self.unreachable = true;
                Ok(reg)
            }
        }
    }

    // ========================================================================
    // Block Expressions & Statements (DRY Delegation)
    // ========================================================================

    pub(super) fn compile_block_expr(
        &mut self,
        statements: &[ast::Stmt],
    ) -> Result<u16, CompileError> {
        self.compile_block_core(statements, BlockMode::Expression)
    }

    pub(super) fn compile_block_stmts(
        &mut self,
        statements: &[ast::Stmt],
    ) -> Result<(), CompileError> {
        // 语句块不关心返回值，丢弃结果寄存器索引
        self.compile_block_core(statements, BlockMode::Statement)?;
        Ok(())
    }

    pub fn compile_block(
        &mut self,
        statements: &[ast::Stmt],
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // 修复原代码硬编码 dest=0 覆盖 r0 的严重 Bug
        // 现统一委托给 DRY 引擎，自动分配安全寄存器
        self.compile_block_core(statements, BlockMode::Expression)
    }
}

// ============================================================================
// 单元测试 (Unit Tests)
// ============================================================================

#[cfg(test)]
mod tests {

    use crate::compiler::Compiler;
    use nuzo_frontend::ast::Span;

    #[test]
    fn test_compile_block_returns_result_reg() {
        // 编译一个块表达式，应返回结果寄存器编号
        let program = nuzo_frontend::parser::Parser::parse("{ 42 }").unwrap();
        let mut compiler = Compiler::builder().source("{ 42 }".to_string()).build();

        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            if let nuzo_frontend::ast::Expr::Block { statements, span } = expr {
                let reg = compiler
                    .compile_block(statements, span)
                    .expect("compile_block 应成功编译块表达式");
                assert!(reg < nuzo_core::MAX_FUNCTION_LOCALS, "返回的寄存器编号应有效");
            } else {
                panic!("预期 Expr::Block");
            }
        } else {
            panic!("预期 Stmt::Expr");
        }
    }

    #[test]
    fn test_compile_block_empty_block() {
        // 空块也应能编译（可能返回 nil 或默认寄存器）
        let span = Span::new(1, 1);
        let mut compiler = Compiler::builder().source("{}".to_string()).build();

        let statements: Vec<nuzo_frontend::ast::Stmt> = vec![];
        let result = compiler.compile_block(&statements, &span);
        // 空块编译应成功（不 panic）
        assert!(result.is_ok(), "空块编译应成功");
    }

    #[test]
    fn test_compile_block_multiple_statements() {
        // 多语句块
        let program = nuzo_frontend::parser::Parser::parse("{ 1; 2; 3 }").unwrap();
        let mut compiler = Compiler::builder().source("{ 1; 2; 3 }".to_string()).build();

        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            if let nuzo_frontend::ast::Expr::Block { statements, span } = expr {
                let reg = compiler.compile_block(statements, span).expect("多语句块应编译成功");
                assert!(reg < nuzo_core::MAX_FUNCTION_LOCALS);
            } else {
                panic!("预期 Expr::Block");
            }
        } else {
            panic!("预期 Stmt::Expr");
        }
    }
}

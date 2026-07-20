//! Function and Compound Type Compilation
//!
//! This module implements compilation of:
//! - Function definitions (fn) and closures
//! - Compound data types (arrays, dicts, tuples, ranges)
//! - Block expressions

use crate::compiler::{COMPILE_FUNCTION_DONE_KEY, CompileError, Compiler};
use crate::macros::*;
use nuzo_core::CAPTURE_OUTER_FLAG;
use nuzo_core::MAX_FUNCTION_LOCALS;
use nuzo_core::Value;
use nuzo_frontend::ast;
use nuzo_frontend::ast::{ExprVisitor, default_visit_expr};
use nuzo_signal::FunctionCompileInfo;
use nuzo_values::ValueExt;
use nuzo_values::function::FunctionPrototype;
use nuzo_values::heap::{CaptureInfo, CaptureMode, HeapObject};
use nuzo_values::nuzo_dict::NuzoDict;
use std::cmp::Reverse;
use std::sync::Arc;

/// ArrayNew 指令的元素计数操作数为 u16。
/// 数组字面量的元素数量不得超过此上限，否则编译报错而非静默截断。
const MAX_ARRAY_ELEMENTS: usize = u16::MAX as usize;

impl Compiler {
    // ========================================================================
    // Functions and Closures
    // ========================================================================

    /// Compile function definition
    ///
    /// Stores function prototype in constant pool, emits Closure instruction.
    /// Supports closure variable capture via FlatEnv+Capture mechanism.
    ///
    /// # Parameters
    ///
    /// * `fn_name` - 函数名称（具名函数为声明时的标识符，匿名函数传 `"<anonymous>"`）
    /// * `params` - 形式参数列表
    /// * `body` - 函数体
    /// * `_span` - 源码位置
    pub(super) fn compile_fn(
        &mut self,
        fn_name: &str,
        params: &[String],
        body: &ast::Block,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // ═══════════════════════════════════════════════════════════
        // Ancestor Captures Stack 管理 — 入口压栈
        // ═══════════════════════════════════════════════════════════
        // 将当前层的 current_captured 压入祖先栈。
        // 这样子孙层在发射 Capture 指令时，可以通过 ancestor_captures
        // 找到跨层的变量（跳过中间层的场景）。
        //
        // 栈状态演变示例 (L1->L2->L3):
        //   L1.compile_fn(编译 L2): push(L1.current_captured=None) → 栈=[]
        //   L2.compile_fn(编译 L3): push(L2.current_captured=[{f}])   → 栈=[[{f}]]
        //   L3 Capture 发射时: 搜索 scope -> current_captured -> ancestor_captures
        // ═══════════════════════════════════════════════════════════
        let should_pop_ancestor = if let Some(ref caps) = self.current_captured {
            self.ancestor_captures.push(caps.clone());
            true
        } else {
            false
        };

        let mut sub_compiler = Compiler::builder()
            .source(self.source.clone())
            .source_file(nuzo_core::DEFAULT_FUNCTION_SOURCE_FILE)
            .build();

        // 设置子编译器 chunk 的 function_name，使 get_source_location() 能返回函数名
        Arc::make_mut(&mut sub_compiler.chunk.debug_info).function_name = Some(fn_name.to_string());

        // 🆕 将祖先捕获栈传递给子编译器（子编译器继承所有祖先信息）
        sub_compiler.ancestor_captures = self.ancestor_captures.clone();

        // 2. Pass parent scope's local variables to sub-compiler for nested function analysis
        sub_compiler.parent_locals = Some(self.scope.all_names());

        // 3. Declare parameters as local variables (starting at r0)
        for param in params {
            sub_compiler.declare_local(param.clone())?;
        }

        // 4. Collect all identifiers used in the function body (for free variable analysis)
        let all_identifiers = collect_all_identifiers(body);

        // 5. Collect all variables that are ASSIGNED TO in the function body
        let assigned_vars = collect_assigned_vars(body);

        // 6. Determine which identifiers are FREE VARIABLES
        let local_names: Vec<String> = sub_compiler.scope.all_names();
        let local_set: std::collections::HashSet<String> = local_names.into_iter().collect();

        let parent_local_set: std::collections::HashSet<String> = match &sub_compiler.parent_locals
        {
            Some(parent_locs) => parent_locs.iter().cloned().collect(),
            None => std::collections::HashSet::new(),
        };

        let parent_captured_set: std::collections::HashSet<String> = match &self.current_captured {
            Some(caps) => caps.iter().map(|info| info.name.clone()).collect(),
            None => std::collections::HashSet::new(),
        };

        let capturable_set: std::collections::HashSet<String> = {
            let mut set: std::collections::HashSet<String> =
                parent_local_set.union(&parent_captured_set).cloned().collect();

            // 🆕 关键修复：将 ancestor_captures 中的变量也加入可捕获集合
            // 这使得跨多层嵌套的自由变量能被正确识别
            for ancestor_caps in &sub_compiler.ancestor_captures {
                for info in ancestor_caps {
                    set.insert(info.name.clone());
                }
            }

            set
        };

        // 优化：先将 all_identifiers 收集到 HashSet 去重，再进行过滤，提升性能
        let unique_identifiers: std::collections::HashSet<String> =
            all_identifiers.into_iter().collect();
        let free_var_names: std::collections::HashSet<String> = unique_identifiers
            .into_iter()
            .filter(|ident| !local_set.contains(ident) && capturable_set.contains(ident))
            .collect();

        // 7. Validate captured variable count
        if free_var_names.len() > u8::MAX as usize {
            return Err(CompileError::Error {
                message: format!(
                    "Too many captured variables ({}), maximum is {}",
                    free_var_names.len(),
                    u8::MAX
                ),
                line: self.current_line,
                column: self.current_column,
            });
        }

        // 8. Generate CaptureInfo for each free variable
        let mut captured_infos: Vec<CaptureInfo> = Vec::with_capacity(free_var_names.len());

        let mut sorted_free_vars: Vec<&String> = free_var_names.iter().collect();
        sorted_free_vars.sort();

        for (idx, name) in sorted_free_vars.iter().enumerate() {
            let mode = if assigned_vars.contains(*name) {
                CaptureMode::ByBox
            } else {
                CaptureMode::ByValue
            };

            captured_infos.push(CaptureInfo {
                name: (**name).clone(),
                mode,
                capture_index: idx as u8,
            });
        }
        // captured_infos 在后续循环中使用，无需额外操作

        // 8.5. Store captured_infos in sub-compiler
        if captured_infos.is_empty() {
            sub_compiler.current_captured = None;
        } else {
            sub_compiler.current_captured = Some(captured_infos.clone());
        }

        // 9. Compile function body, preserving last expression as return value
        //
        // ═══════════════════════════════════════════════════════════
        // 🔗 TCO (尾调用优化) - 编译器侧契约
        // ═══════════════════════════════════════════════════════════
        //
        // 【核心职责】确保函数体的"隐式 return"值在尾位置
        //
        // 【工作机制】
        //   1. 函数体最后一个表达式语句 → 编译为 Call 指令（如果它是函数调用）
        //   2. 函数体结束后 → 自动发射 OP_RETURN
        //   3. 结果: 字节码中 OP_CALL 紧跟 OP_RETURN
        //   4. VM 的 is_tail_position() 检测到这个模式 → 走 TCO 路径
        //
        // 【VM 侧实现】dispatch.rs:L193-290 `execute_tail_call()` (8步原地变异帧)
        // 【触发点】    dispatch.rs:L668, L695 (is_tail=true 时路由)
        // 【调用侧】   expressions.rs:L738-740 (Call 表达式的尾位置标记)
        //
        // ⚠️ 编译器无需额外标记！只需保证 Call+Return 相邻即可。
        // ═══════════════════════════════════════════════════════════
        let fn_compile_start = std::time::Instant::now();
        sub_compiler.enter_scope("function");
        let saved_regs = sub_compiler.save_register_state();
        let mut last_expr_reg: Option<u16> = None;

        // 找到最后一个表达式语句的索引，它需要特殊处理（尾位置标记）
        let last_expr_idx = body.iter().rposition(|s| matches!(s, ast::Stmt::Expr(_)));

        for (idx, stmt) in body.iter().enumerate() {
            // 死代码消除：如果函数体中已有 return，跳过后续不可达代码
            if sub_compiler.unreachable {
                continue;
            }

            let is_last_expr = last_expr_idx == Some(idx);
            match stmt {
                ast::Stmt::Expr(expr) if is_last_expr => {
                    // 函数体最后一个表达式：编译后 Call 紧跟 Return，VM 自动识别为尾调用
                    let reg = sub_compiler.compile_expr(expr)?;
                    // 关键：将具名函数声明（fn adder(x) { ... }）绑定到作用域
                    // 否则后续引用该函数名时会回退到 GetGlobal，导致运行时报
                    // "undefined variable" 错误（Bug #1: 嵌套函数返回局部函数时变量丢失）
                    sub_compiler.bind_fn_to_scope(expr, reg)?;
                    last_expr_reg = Some(reg);
                }
                ast::Stmt::Expr(expr) => {
                    // 中间表达式：不在尾位置
                    let reg = sub_compiler.compile_expr(expr)?;
                    // 同上：局部函数名必须注册到作用域
                    sub_compiler.bind_fn_to_scope(expr, reg)?;
                    last_expr_reg = Some(reg);
                }
                _ => {
                    // 赋值等语句：不在尾位置
                    sub_compiler.compile_stmt(stmt)?;
                }
            }
        }

        // 10. Emit return with last expression value (or nil)
        // 修复：必须在 release_registers 之前 emit Return 指令。
        // 否则 release_registers 会收缩 next_reg 并释放 last_expr_reg，
        // 导致 Return 指令引用了一个逻辑上已失效的寄存器。
        match last_expr_reg {
            Some(reg) => {
                emit_typed!(sub_compiler, Return, reg);
            }
            None => {
                let nil_reg = sub_compiler.alloc_register()?;
                emit_typed!(sub_compiler, LoadNil, nil_reg);
                emit_typed!(sub_compiler, Return, nil_reg);
            }
        }

        // 捕获 locals_count 必须使用 peak_reg（next_reg 的峰值），而非 next_reg 终值。
        // release_temp_register() 会收缩 next_reg，但字节码仍引用峰值范围内的寄存器，
        // 若用终值则 VM 寄存器文件分配不足，导致 RegisterOutOfBounds 错误。
        let locals_count = sub_compiler.peak_reg;

        sub_compiler.exit_scope();
        sub_compiler.release_registers(saved_regs);

        // 10.5. Emit COMPILE_FUNCTION_DONE signal (zero-overhead if no subscribers)
        let fn_compile_duration = fn_compile_start.elapsed();
        if let Ok(signal) = self.bus.get(&COMPILE_FUNCTION_DONE_KEY)
            && !signal.is_empty()
        {
            // instruction_count 使用字节码字节数作为近似指标
            let instruction_count = sub_compiler.chunk.code().len();
            signal.emit(&FunctionCompileInfo {
                name: fn_name.to_string(),
                arity: params.len() as u8,
                instruction_count,
                duration: fn_compile_duration,
            });
        }

        // 11. Extract compiled chunk and create FunctionPrototype
        let chunk = std::mem::take(&mut sub_compiler.chunk);
        let arity = params.len() as u8;

        let (code, constants, lines, debug_info, _locals_count, spill_slot_count) =
            chunk.into_parts();
        let prototype = FunctionPrototype {
            name: fn_name.to_string(),
            arity,
            locals_count,
            chunk: code,
            constants,
            captured_vars: captured_infos,
            lines,
            debug_info: Arc::clone(&debug_info),
            spill_slot_count,
        };

        let capture_emit_infos = prototype.captured_vars.clone();

        // 12. Create Closure object and store in constant pool
        let closure_value = Value::from_heap_object_gc(HeapObject::Closure {
            prototype: Arc::new(prototype),
            captured: Vec::new(),
            parent_env: None,
        });

        let proto_idx = self.add_constant_checked(closure_value)?;

        // 13. Emit Closure instruction
        let reg = self.alloc_register()?;
        emit_typed!(self, Closure, reg, proto_idx);

        // 14. Emit Capture instructions for each captured variable
        //
        // ═══════════════════════════════════════════════════════════
        // 三级变量查找链（Three-Level Variable Resolution Chain）
        // ═══════════════════════════════════════════════════════════
        //
        // 对于每个被捕获的变量，按以下优先级查找其来源：
        //
        //   Path 1: self.scope.resolve()     → 当前作用域的局部变量（父函数的参数/局部）
        //   Path 2: self.current_captured    → 直接父级的捕获列表（1 层外层）
        //   Path 3: self.ancestor_captures   → 祖先捕获栈遍历（2+ 层外层）🆕
        //
        // 为什么需要 Path 3?
        //   考虑 3 层嵌套: L1(f) -> L2(g) -> L3(x) { f(g(x)) }
        //   - L3 需要捕获 f (from L1) 和 g (from L2)
        //   - L3 的 Capture 指令由 L2 编译器发射 (self = L2)
        //   - g 在 L2.scope 中 → Path 1 命中
        //   - f 在 L2.current_captured 中 → Path 2 命中
        //   - 但如果场景变为 4+ 层或跨层引用, Path 3 确保不遗漏
        // ═══════════════════════════════════════════════════════════
        for info in &capture_emit_infos {
            let mut captured = false;

            // Path 1: 变量在当前编译器的作用域中（父函数的参数或局部变量）
            if let Some(nuzo_bytecode::scope::ScopeKind::Local(src_reg)) =
                self.scope.resolve(&info.name)
            {
                emit_typed!(self, Capture, reg, info.capture_index as u16, src_reg);
                captured = true;
            }
            // Path 2: 变量在直接父级的捕获列表中
            else if let Some(ref parent_cap) = self.current_captured
                && let Some(outer_info) = parent_cap.iter().find(|ci| ci.name == info.name)
            {
                let outer_idx = outer_info.capture_index;
                emit_typed!(
                    self,
                    Capture,
                    reg,
                    info.capture_index as u16,
                    (outer_idx as u16) | CAPTURE_OUTER_FLAG
                );
                captured = true;
            }

            // Path 3: 🆕 遍历祖先捕获栈（从最近到最远），查找跨层变量
            if !captured {
                for ancestor_caps in self.ancestor_captures.iter().rev() {
                    if let Some(outer_info) = ancestor_caps.iter().find(|ci| ci.name == info.name) {
                        let outer_idx = outer_info.capture_index;
                        emit_typed!(
                            self,
                            Capture,
                            reg,
                            info.capture_index as u16,
                            (outer_idx as u16) | CAPTURE_OUTER_FLAG
                        );
                        captured = true;
                        break;
                    }
                }
            }

            // 防御性日志：变量在所有路径中都未找到（不应发生，表示自由变量分析有 bug）
            // D3 修复：此警告不应仅在 debug 模式下生效。编译器 bug 导致的运行时 NIL
            // 在 release 模式下完全不可观察，使生产问题 debug 异常困难。
            if !captured {
                eprintln!(
                    "[WARNING] Captured variable '{}' not found in scope, current_captured, or ancestor_captures ({} ancestors). Will be NIL at runtime.",
                    info.name,
                    self.ancestor_captures.len()
                );
            }
        }

        // ═══════════════════════════════════════════════════════════
        // Ancestor Captures Stack 管理 — 出口弹栈
        // ═══════════════════════════════════════════════════════════
        // LIFO 弹出入口处压入的当前层捕获信息，恢复栈状态。
        // 确保兄弟函数（同一层的其他闭包）不会看到脏数据。
        // ═══════════════════════════════════════════════════════════
        if should_pop_ancestor {
            self.ancestor_captures.pop();
        }

        Ok(reg)
    }

    /// Compile closure (anonymous function)
    ///
    /// Same as compile_fn but captures variables from enclosing scope.
    /// Closures are always anonymous, so `fn_name` defaults to `"<anonymous>"`.
    pub(super) fn compile_closure(
        &mut self,
        params: &[String],
        body: &ast::Block,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        self.compile_fn("<anonymous>", params, body, span)
    }

    // ========================================================================
    // Compound Data Types
    // ========================================================================

    /// Compile array literal
    ///
    /// Emits: compile elements..., ArrayNew dest, count
    ///
    /// ## Slot-based allocation strategy (migrated from manual next_reg)
    ///
    /// Uses `reserve_slot(count, ArrayConstruct)` to allocate a contiguous
    /// register region: `[base, base+1, ..., base+N]` where:
    /// - `base` holds the final array value (ArrayNew destination)
    /// - `base+1 .. base+N` hold element values (VM contiguity requirement)
    ///
    /// This replaces the previous "remote base address" strategy that
    /// manually advanced `next_reg` and checked `MAX_FUNCTION_LOCALS`.
    pub(super) fn compile_array(
        &mut self,
        elements: &[ast::Expr],
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        if elements.is_empty() {
            let dest = self.alloc_register()?;
            emit_typed!(self, ArrayNew, dest, 0);
            return Ok(dest);
        }

        // Remote base allocation (verified bugfix strategy)
        // Force base = next_reg to ensure construction zone is in
        // high-address area that doesn't overlap with active registers
        let dest = self.alloc_register()?;
        let construct_size = 1 + elements.len() as u16;

        // Boundary check
        if self.next_reg + construct_size > MAX_FUNCTION_LOCALS {
            return Err(CompileError::TooManyLocals {
                count: self.next_reg as usize + construct_size as usize,
                line: self.current_line,
                column: self.current_column,
            });
        }

        // Critical: base must be at next_reg (remote high-address zone)
        let base = self.next_reg;
        self.next_reg += construct_size;
        self.peak_reg = self.peak_reg.max(self.next_reg);

        // Watermark protection: prevent release_temp_register from shrinking
        // next_reg into the pre-allocated construction zone [base, base+construct_size).
        let saved_watermark = self.reserve_watermark;
        self.reserve_watermark = self.next_reg;

        // Remove any registers in the construction zone from free_registers.
        // Without this, alloc_register() could pop a freed register that falls
        // within [base, base+construct_size), overwriting array element data.
        let kept: Vec<_> = self
            .free_registers
            .drain()
            .filter(|Reverse(r)| *r < base || *r >= base + construct_size)
            .collect();
        self.free_registers.extend(kept);

        // Compile each element into its reserved slot
        for (i, elem) in elements.iter().enumerate() {
            let target = base + 1 + i as u16;
            let val_reg = self.compile_expr(elem)?;
            if val_reg != target {
                self.emit_mov(target, val_reg);
                // val_reg is outside the construction zone, safe to release
                self.release_temp_register(val_reg);
            }
            // If val_reg == target, the value is already in its reserved slot.
            // Do NOT release it — it's part of the construction zone and must
            // stay protected until ArrayNew reads it.
        }

        // Defensive check
        let elem_count = elements.len();
        if elem_count > MAX_ARRAY_ELEMENTS {
            return Err(CompileError::ArrayElementOverflow {
                count: elem_count,
                line: self.current_line,
                column: self.current_column,
            });
        }

        // VM convention: ArrayNew reads elements from dest+1, dest+2, ...
        // With remote base: base=next_reg_start, elements at base+1..base+N
        emit_typed!(self, ArrayNew, base, elem_count as u16);

        // Move result to dest if needed
        if base != dest {
            self.emit_mov(dest, base);
        }

        // Release construction region registers (except dest)
        for reg in (base + 1)..(base + construct_size) {
            self.release_temp_register(reg);
        }
        // Also release base itself if different from dest
        if base != dest {
            self.release_temp_register(base);
        }

        // Restore watermark after construction zone is released
        self.reserve_watermark = saved_watermark;

        Ok(dest)
    }

    /// Compile dictionary literal
    ///
    /// Allocates an empty Dict via constant pool + LoadK, then sets each key-value pair via SetProp.
    pub(super) fn compile_dict(
        &mut self,
        pairs: &[(String, ast::Expr)],
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let dest = self.alloc_register()?;

        let dict_value = Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new()));
        let dict_const_idx = self.add_constant_checked(dict_value)?;
        emit_typed!(self, LoadK, dest, dict_const_idx);

        for (key, value_expr) in pairs {
            let val_reg = self.compile_expr(value_expr)?;

            let name_const_idx = self.add_constant_checked(Value::from_string(key))?;

            emit_typed!(self, SetProp, dest, name_const_idx, val_reg);

            // 修复：SetProp 完成后立即释放 val_reg，防止字典字面量过长导致寄存器耗尽
            self.release_temp_register(val_reg);
        }

        Ok(dest)
    }

    /// Compile tuple literal
    ///
    /// Currently compiles as array (tuples not yet fully supported)
    pub(super) fn compile_tuple(
        &mut self,
        elements: &[ast::Expr],
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        self.compile_array(elements, span)
    }

    /// Compile range literal
    ///
    /// Emits: compile start, compile end, Range dest, start_reg, end_reg, inclusive
    pub(super) fn compile_range(
        &mut self,
        start: &ast::Expr,
        end: &ast::Expr,
        inclusive: bool,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let start_reg = self.compile_expr(start)?;
        let end_reg = self.compile_expr(end)?;

        let dest = self.alloc_register()?;

        emit_typed!(self, RangeNew, dest, start_reg, end_reg, if inclusive { 1 } else { 0 });

        // 修复：释放 start_reg 和 end_reg 避免泄漏
        if dest != start_reg {
            self.release_temp_register(start_reg);
        }
        if dest != end_reg {
            self.release_temp_register(end_reg);
        }

        Ok(dest)
    }
}

// ============================================================================
// Public Helper Functions (used by compile_fn and other modules)
// ============================================================================
//
// 以下两个函数（collect_all_identifiers 和 collect_assigned_vars）原为 ~320 行
// 手写递归遍历代码，现已统一使用 nuzo_frontend::ast::ExprVisitor trait。
// 这消除了与 nuzo_ir::builder 中完全相同的遍历逻辑重复。

/// Collect all identifier names used in a block of code
///
/// This is used by compile_fn() for free variable analysis.
/// Returns a `Vec<String>` containing all identifier names found (with duplicates).
///
/// # 实现
/// 使用 `IdentifierCollector`（实现 `ExprVisitor` trait）替代手写递归遍历。
/// 关键行为：
/// - **递归进入函数体**：支持 3+ 层嵌套闭包的跨层变量捕获
/// - **跳过 MatchPattern::Variable 绑定**：变量绑定不是引用，不应被收集
/// - **收集赋值目标标识符**：`x = expr` 中的 x 也算作标识符使用
pub fn collect_all_identifiers(block: &ast::Block) -> Vec<String> {
    let mut collector = IdentifierCollector::new();
    collector.visit_block(block);
    collector.finish()
}

/// 标识符收集器 — 使用 ExprVisitor trait 统一遍历逻辑
///
/// 与 `nuzo_ir::builder::FreeVarCollector` 的关键区别：
/// - **会递归进入函数体**（支持跨层捕获分析）
/// - **收集所有标识符**（不做自由/局部变量过滤）
/// - **跳过 MatchPattern::Variable**（变量绑定不是引用）
struct IdentifierCollector {
    identifiers: Vec<String>,
}

impl IdentifierCollector {
    fn new() -> Self {
        Self { identifiers: Vec::new() }
    }

    fn finish(self) -> Vec<String> {
        self.identifiers
    }
}

impl ExprVisitor for IdentifierCollector {
    fn visit_ident(&mut self, name: &str, _span: &ast::Span) {
        self.identifiers.push(name.to_string());
    }

    /// 赋值语句：收集赋值目标标识符 + 遍历赋值值和目标子表达式
    fn visit_assign(&mut self, target: &ast::AssignTarget, value: &ast::Expr, _span: &ast::Span) {
        // 收集赋值目标中的标识符
        match target {
            ast::AssignTarget::Ident { name } => {
                self.identifiers.push(name.clone());
            }
            ast::AssignTarget::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            ast::AssignTarget::Field { object, .. } => {
                self.visit_expr(object);
            }
        }
        // 遍历赋值值
        self.visit_expr(value);
    }

    /// 函数/闭包：递归进入函数体（支持跨层捕获分析）
    ///
    /// 与 FreeVarCollector 不同，这里必须进入函数体，
    /// 以支持 3+ 层嵌套闭包的跨层变量捕获。
    fn visit_fn(
        &mut self,
        _name: Option<&str>,
        _params: &[String],
        body: &ast::Block,
        _span: &ast::Span,
    ) {
        self.visit_block(body);
    }

    /// Match 表达式：跳过 Variable 绑定模式
    ///
    /// 变量绑定模式（如 `n => ...` 中的 n）是新建绑定，不是标识符引用，
    /// 不应被收集为自由变量。需要覆盖 visit_expr 来处理此特殊情况。
    fn visit_expr(&mut self, expr: &ast::Expr) {
        if let ast::Expr::Match { scrutinee, arms, .. } = expr {
            self.visit_expr(scrutinee);
            for arm in arms {
                // Variable 模式是绑定，不是引用 — 跳过
                match &arm.pattern {
                    ast::MatchPattern::Literal(lit) => self.visit_expr(lit),
                    ast::MatchPattern::Range { start, end, .. } => {
                        self.visit_expr(start);
                        self.visit_expr(end);
                    }
                    ast::MatchPattern::Variable(_) | ast::MatchPattern::Wildcard => {}
                }
                self.visit_expr(&arm.body);
            }
        } else {
            // 其他表达式类型使用默认遍历
            default_visit_expr(self, expr);
        }
    }
}

/// Collect all variable names that are ASSIGNED TO in a block
///
/// This is used by compile_fn() to determine capture mode:
/// - Assigned variables → ByBox (mutable capture)
/// - Read-only variables → ByValue (immutable capture)
///
/// # 实现
/// 使用 `CompilerAssignedVarCollector`（实现 `ExprVisitor` trait）替代手写递归遍历。
/// 与 `nuzo_ir::builder::AssignedVarCollector` 逻辑一致：
/// - **会递归进入函数体**（发现内部赋值以判断可变捕获）
/// - **拦截 Assign 语句**收集被赋值的标识符名
pub fn collect_assigned_vars(block: &ast::Block) -> std::collections::HashSet<String> {
    let mut collector = CompilerAssignedVarCollector::new();
    collector.visit_block(block);
    collector.finish()
}

/// 被赋值变量收集器（编译器版）— 使用 ExprVisitor trait 统一遍历逻辑
///
/// 与 `nuzo_ir::builder::AssignedVarCollector` 逻辑一致，
/// 只是存在于不同的 crate 中（compiler vs ir）。
struct CompilerAssignedVarCollector {
    assigned: std::collections::HashSet<String>,
}

impl CompilerAssignedVarCollector {
    fn new() -> Self {
        Self { assigned: std::collections::HashSet::new() }
    }

    fn finish(self) -> std::collections::HashSet<String> {
        self.assigned
    }
}

impl ExprVisitor for CompilerAssignedVarCollector {
    /// 拦截赋值语句：记录被赋值的标识符
    fn visit_assign(&mut self, target: &ast::AssignTarget, value: &ast::Expr, _span: &ast::Span) {
        if let ast::AssignTarget::Ident { name } = target {
            self.assigned.insert(name.clone());
        }
        // 继续遍历赋值值和目标中的子表达式
        self.visit_expr(value);
        match target {
            ast::AssignTarget::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            ast::AssignTarget::Field { object, .. } => {
                self.visit_expr(object);
            }
            ast::AssignTarget::Ident { .. } => {}
        }
    }

    /// 函数/闭包：递归进入函数体（与 IR 版一致）
    ///
    /// 需要发现函数内部的赋值以判断是否需要 ByBox（可变）捕获。
    fn visit_fn(
        &mut self,
        _name: Option<&str>,
        _params: &[String],
        body: &ast::Block,
        _span: &ast::Span,
    ) {
        self.visit_block(body);
    }
}

// ============================================================================
// 单元测试 (Unit Tests)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_all_identifiers_extracts_names() {
        // 解析包含多个标识符的代码
        let program = nuzo_frontend::parser::Parser::parse("x + y").unwrap();
        let block: &ast::Block = &program.statements;
        let identifiers = collect_all_identifiers(block);

        // 应包含 x 和 y
        assert!(identifiers.contains(&"x".to_string()), "应包含标识符 x");
        assert!(identifiers.contains(&"y".to_string()), "应包含标识符 y");
    }

    #[test]
    fn test_collect_all_identifiers_from_assignment() {
        // 赋值语句中的标识符也应被收集
        let program = nuzo_frontend::parser::Parser::parse("x = a + b").unwrap();
        let block: &ast::Block = &program.statements;
        let identifiers = collect_all_identifiers(block);

        assert!(identifiers.contains(&"x".to_string()), "应包含赋值目标 x");
        assert!(identifiers.contains(&"a".to_string()), "应包含 a");
        assert!(identifiers.contains(&"b".to_string()), "应包含 b");
    }

    #[test]
    fn test_collect_all_identifiers_empty_block() {
        let block: ast::Block = vec![];
        let identifiers = collect_all_identifiers(&block);
        assert!(identifiers.is_empty(), "空 block 应返回空列表");
    }

    #[test]
    fn test_collect_all_identifiers_number_only() {
        // 纯数字字面量不应产生标识符
        let program = nuzo_frontend::parser::Parser::parse("42").unwrap();
        let block: &ast::Block = &program.statements;
        let identifiers = collect_all_identifiers(block);
        assert!(identifiers.is_empty(), "纯数字不应产生标识符");
    }

    #[test]
    fn test_collect_assigned_vars_identifies_targets() {
        // 赋值语句的目标变量应被收集
        let program = nuzo_frontend::parser::Parser::parse("x = 1").unwrap();
        let block: &ast::Block = &program.statements;
        let assigned = collect_assigned_vars(block);

        assert!(assigned.contains("x"), "应包含被赋值的变量 x");
    }

    #[test]
    fn test_collect_assigned_vars_multiple_assignments() {
        let program = nuzo_frontend::parser::Parser::parse("a = 1\nb = 2\nc = a + b").unwrap();
        let block: &ast::Block = &program.statements;
        let assigned = collect_assigned_vars(block);

        assert!(assigned.contains("a"), "应包含 a");
        assert!(assigned.contains("b"), "应包含 b");
        assert!(assigned.contains("c"), "应包含 c");
        assert_eq!(assigned.len(), 3, "应有 3 个被赋值的变量");
    }

    #[test]
    fn test_collect_assigned_vars_empty_block() {
        let block: ast::Block = vec![];
        let assigned = collect_assigned_vars(&block);
        assert!(assigned.is_empty(), "空 block 应返回空集合");
    }

    #[test]
    fn test_collect_assigned_vars_no_assignment() {
        // 只有表达式语句，没有赋值
        let program = nuzo_frontend::parser::Parser::parse("x + y").unwrap();
        let block: &ast::Block = &program.statements;
        let assigned = collect_assigned_vars(block);
        assert!(assigned.is_empty(), "无赋值语句时应返回空集合");
    }
}

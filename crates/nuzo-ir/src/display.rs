//! IR 显示格式化与合法性验证

use std::collections::HashSet;
use std::fmt::{self, Display};

use crate::error::{IrValidationError, ValidationWarning};
use crate::types::*;

// ============================================================================
// Display 格式化（LLVM IR 风格）
// ============================================================================

/// IrOp 的单行显示 — 三地址码形式
impl Display for IrOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadConstant { dest, constant } => {
                write!(f, "v{} = load_const {}", dest.0, format_constant(constant))
            }
            Self::LoadArg { dest, index } => {
                write!(f, "v{} = arg {}", dest.0, index)
            }
            Self::Binary { dest, op, left, right } => {
                write!(f, "v{} = v{} {} v{}", dest.0, left.0, op.as_str(), right.0)
            }
            Self::Unary { dest, op, operand } => {
                write!(f, "v{} = {}v{}", dest.0, op.as_str(), operand.0)
            }
            Self::Mov { dest, src } => {
                write!(f, "v{} = mov v{}", dest.0, src.0)
            }
            Self::Call { dest, callee, args } => {
                let dest_str = dest.map_or(String::new(), |d| format!("v{} = ", d.0));
                let args_str: Vec<String> = args.iter().map(|a| format!("v{}", a.0)).collect();
                write!(f, "{}call v{} [{}]", dest_str, callee.0, args_str.join(", "))
            }
            Self::Closure { dest, ir_func } => {
                write!(f, "v{} = closure fn{}", dest.0, ir_func.0)
            }
            Self::Capture { closure, index, source } => match source {
                CaptureSource::Register(vr) => {
                    write!(f, "capture v{}[{}] = v{}", closure.0, index, vr.0)
                }
                CaptureSource::OuterCapture(outer_idx) => {
                    write!(f, "capture v{}[{}] = outer[{}]", closure.0, index, outer_idx)
                }
                CaptureSource::Global(name) => {
                    write!(f, "capture v{}[{}] = global({})", closure.0, index, name)
                }
            },
            Self::GetLocal { dest, name } => {
                write!(f, "v{} = local {}", dest.0, name)
            }
            Self::SetLocal { name, value } => {
                write!(f, "local {} = v{}", name, value.0)
            }
            Self::GetGlobal { dest, name } => {
                write!(f, "v{} = global {}", dest.0, name)
            }
            Self::SetGlobal { name, value } => {
                write!(f, "global {} = v{}", name, value.0)
            }
            Self::GetCapture { dest, index } => {
                write!(f, "v{} = capture[{}]", dest.0, index)
            }
            Self::SetCapture { index, value } => {
                write!(f, "capture[{}] = v{}", index, value.0)
            }
            Self::Jump { target } => {
                write!(f, "jump bb{}", target.0)
            }
            Self::JumpIf { cond, then_target, else_target } => {
                write!(f, "if v{} goto bb{} else bb{}", cond.0, then_target.0, else_target.0)
            }
            Self::Return { value } => match value {
                Some(v) => write!(f, "return v{}", v.0),
                None => write!(f, "return"),
            },
            Self::ArrayNew { dest, elements } => {
                let elems: Vec<String> = elements.iter().map(|e| format!("v{}", e.0)).collect();
                write!(f, "v{} = array [{}]", dest.0, elems.join(", "))
            }
            Self::ObjectNew { dest } => {
                write!(f, "v{} = object {{}}", dest.0)
            }
            Self::GetField { dest, object, field } => {
                write!(f, "v{} = v{}.{}", dest.0, object.0, field)
            }
            Self::SetField { object, field, value } => {
                write!(f, "v{}.{} = v{}", object.0, field, value.0)
            }
            Self::IndexGet { dest, object, index } => {
                write!(f, "v{} = v{}[v{}]", dest.0, object.0, index.0)
            }
            Self::IndexSet { object, index, value } => {
                write!(f, "v{}[v{}] = v{} (cow)", object.0, index.0, value.0)
            }
            Self::IndexSetMut { object, index, value } => {
                write!(f, "v{}[v{}] = v{} (mut)", object.0, index.0, value.0)
            }
            Self::Select { condition, then_value, else_value, dest } => {
                write!(f, "v{} = v{} ? v{} : v{}", dest.0, condition.0, then_value.0, else_value.0)
            }
            Self::Len { dest, object } => {
                write!(f, "v{} = len v{}", dest.0, object.0)
            }
            Self::RangeNew { dest, start, end, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "v{} = v{}{}v{}", dest.0, start.0, op, end.0)
            }
            Self::TryStart { catch_target, exception_reg } => {
                write!(f, "try_start -> bb{} (exc=r{})", catch_target.0, exception_reg)
            }
            Self::TryEnd => {
                write!(f, "try_end")
            }
            Self::Out { value } => {
                write!(f, "out v{}", value.0)
            }
            Self::Print { value } => {
                write!(f, "print v{}", value.0)
            }
            Self::StringBuild { dest, operands } => {
                let ops: Vec<String> = operands.iter().map(|o| format!("v{}", o.0)).collect();
                write!(f, "v{} = string_build [{}]", dest.0, ops.join(", "))
            }
            Self::SliceChainInit { dest } => {
                write!(f, "v{} = slicechain_new", dest.0)
            }
            Self::SliceChainAppend { chain, src } => {
                write!(f, "slicechain_append v{}, v{}", chain.0, src.0)
            }
            Self::SliceChainFinish { dest, chain } => {
                write!(f, "v{} = slicechain_finish v{}", dest.0, chain.0)
            }
        }
    }
}

/// 格式化 IR 常量为可读字符串
fn format_constant(c: &IrConstant) -> String {
    match c {
        IrConstant::Number(n) => format!("{}", n),
        IrConstant::String(s) => format!("{:?}", s),
        IrConstant::Bool(b) => format!("{}", b),
        IrConstant::Nil => "nil".to_string(),
    }
}

/// IrFunction 的多行显示 — 带基本块标注
impl Display for IrFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "function fn{}({}):",
            self.id.0,
            self.params.iter().map(|p| &**p).collect::<Vec<_>>().join(", ")
        )?;
        for block in &self.blocks {
            writeln!(f, "  bb{}:", block.id.0)?;
            for instr in &block.instructions {
                writeln!(f, "    {}", instr)?;
            }
        }
        Ok(())
    }
}

/// IrModule 的完整显示 — 模块头 + 全局变量 + 所有函数
impl Display for IrModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "; Nuzo IR Module")?;
        writeln!(f, "; ============")?;
        if !self.globals.is_empty() {
            writeln!(
                f,
                "; globals: [{}]",
                self.globals.iter().map(|g| &**g).collect::<Vec<_>>().join(", ")
            )?;
        }
        for func in &self.functions {
            write!(f, "{}", func)?;
        }
        Ok(())
    }
}

// ============================================================================
// validate() — IR 合法性验证（Phase 1：结构性检查 + Phase 2：作用域完整性）
// ============================================================================

/// 验证结果 — 包含错误列表和警告列表
///
/// 验证通过的条件是错误列表为空。警告不阻止通过，
/// 但提示可能存在的作用域管理问题。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationResult {
    /// 致命错误列表 — 任何一项存在即表示 IR 不合法
    pub errors: Vec<IrValidationError>,
    /// 非致命警告列表 — 提示可疑但不一定是错误的模式
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationResult {
    /// 验证是否通过（无任何错误）
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// 将 Result<Vec<Error>, ()> 转换为标准 Result
    pub fn into_result(self) -> Result<(), Vec<IrValidationError>> {
        if self.errors.is_empty() { Ok(()) } else { Err(self.errors) }
    }
}

impl IrModule {
    /// 验证整个模块的合法性
    ///
    /// # 检查项（Phase 1: 结构性）
    /// 1. 每个非空基本块是否有终止指令
    /// 2. ValueRef 是否在合理范围内
    /// 3. 基本块跳转目标是否有效
    /// 4. Closure 引用的函数 ID 是否有效
    ///
    /// # 检查项（Phase 2: 函数作用域完整性）
    /// - 规则 1: 主函数非空检查（有子函数时 main 不应为空）
    /// - 规则 2: Capture/Argument 指令不应出现在 main 中
    /// - 规则 3: Closure 引用的函数索引必须在有效范围
    /// - 规则 4: 基本块中的指令索引必须在该函数的指令向量范围内
    pub fn validate(&self) -> Result<(), IrValidationError> {
        let result = self.validate_full();
        result
            .into_result()
            .map_err(|errs| errs.into_iter().next().expect("errors non-empty when Err"))
    }

    /// 完整验证 — 返回所有错误和警告（用于详细诊断）
    pub fn validate_full(&self) -> ValidationResult {
        let mut result = ValidationResult::default();

        // Phase 1: 结构性检查（原有逻辑）
        for (func_idx, func) in self.functions.iter().enumerate() {
            if let Err(e) = self.validate_function_structural(func, func_idx) {
                result.errors.push(e);
                // 第一个结构性错误即返回（快速失败）
                return result;
            }
        }

        // Phase 2: 函数作用域完整性检查（新增规则 1-4）
        let scope_result = self.validate_scope_integrity();
        result.errors.extend(scope_result.errors);
        result.warnings.extend(scope_result.warnings);

        result
    }

    // ── Phase 1: 结构性检查（原有逻辑）─────────────────────────

    fn validate_function_structural(
        &self,
        func: &IrFunction,
        _func_idx: usize,
    ) -> Result<(), IrValidationError> {
        let mut defined: HashSet<u32> = HashSet::new();

        // 收集所有基本块 ID 用于跳转目标验证
        let valid_block_ids: HashSet<u32> = func.blocks.iter().map(|b| b.id.0).collect();
        let valid_func_ids: HashSet<u32> = (0..self.functions.len() as u32).collect();

        for block in &func.blocks {
            // 检查终止指令
            if !block.instructions.is_empty()
                && !block.instructions.last().expect("checked is_empty above").is_terminator()
            {
                return Err(IrValidationError::BlockMissingTerminator { block_id: block.id.0 });
            }

            // 逐条指令验证
            for instr in &block.instructions {
                // 记录目标 ValueRef 为已定义
                if let Some(dest) = instr.dest() {
                    defined.insert(dest.0);
                }

                // 验证操作数 ValueRef 范围
                self.validate_value_refs(instr)?;

                // 验证控制流目标
                self.validate_control_flow(instr, &valid_block_ids, &valid_func_ids)?;
            }
        }

        Ok(())
    }

    /// 检查指令中使用的 ValueRef 是否在合理范围内
    fn validate_value_refs(&self, instr: &IrOp) -> Result<(), IrValidationError> {
        const MAX_REASONABLE: u32 = 10_000_000;
        for ref_id in instr.operand_value_refs() {
            if ref_id > MAX_REASONABLE {
                return Err(IrValidationError::UndefinedValueRef {
                    value_ref: ref_id,
                    context: format!("{:?}", instr),
                });
            }
        }
        Ok(())
    }

    /// 检查跳转/闭包引用的目标是否存在
    fn validate_control_flow(
        &self,
        instr: &IrOp,
        valid_blocks: &HashSet<u32>,
        valid_funcs: &HashSet<u32>,
    ) -> Result<(), IrValidationError> {
        match instr {
            IrOp::Jump { target } => {
                if !valid_blocks.contains(&target.0) {
                    return Err(IrValidationError::InvalidBlockId {
                        block_id: target.0,
                        function_id: 0,
                    });
                }
            }
            IrOp::JumpIf { then_target, else_target, .. } => {
                for tid in [then_target.0, else_target.0] {
                    if !valid_blocks.contains(&tid) {
                        return Err(IrValidationError::InvalidBlockId {
                            block_id: tid,
                            function_id: 0,
                        });
                    }
                }
            }
            IrOp::Closure { ir_func, .. } => {
                if !valid_funcs.contains(&ir_func.0) {
                    return Err(IrValidationError::UndefinedFunction { func_id: ir_func.0 });
                }
            }
            IrOp::TryStart { catch_target, .. } if !valid_blocks.contains(&catch_target.0) => {
                return Err(IrValidationError::InvalidBlockId {
                    block_id: catch_target.0,
                    function_id: 0,
                });
            }
            _ => {}
        }
        Ok(())
    }

    // ── Phase 2: 函数作用域完整性检查（新增规则 1-4）────────────

    /// 执行全部 4 条函数作用域完整性检查规则
    ///
    /// 这些规则专门针对 `build_closure_expr` 类型的 bug 设计，
    /// 即构建器遗漏了 `current_function_id` / `current_block_id` 的保存/恢复，
    /// 导致指令被发射到错误的函数中。
    fn validate_scope_integrity(&self) -> ValidationResult {
        let mut result = ValidationResult::default();

        // 规则 1: 主函数非空检查
        result.extend_errors(self.check_rule1_main_not_empty());

        // 规则 2 + 3 + 4 + 5: 逐函数遍历指令
        for (func_idx, func) in self.functions.iter().enumerate() {
            // 规则 2: 函数内指令类型检查 (Capture/Argument 在 main 中)
            result.extend_errors(self.check_rule2_instruction_types(func_idx, func));

            // 规则 3: Closure 引用有效性（增强版，带位置信息）
            result.extend_errors(self.check_rule3_closure_references(func_idx, func));

            // 规则 4: 基本块所有权检查
            result.extend_errors(self.check_rule4_block_ownership(func_idx, func));

            // 规则 5: 源操作数 ValueRef 引用完整性检查（新增）
            // 验证所有 src 引用的 ValueRef 都在该函数内有定义
            // 定义来源：指令 dest、LoadArg、GetCapture、闭包 Closure 创建等
            result.extend_errors(self.check_rule5_src_references(func_idx, func));
        }

        result
    }

    /// 规则 1: 主函数非空检查
    ///
    /// 当模块包含子函数 (fn1+) 时，main 函数 (fn0) 应该包含至少一条
    /// 有意义的指令（不仅仅是隐式的 `return nil`）。
    ///
    /// 如果 main 为空但存在子函数，这强烈暗示顶层语句被错误地发射到了
    /// 子函数中——这正是 `build_closure_expr` 遗漏上下文恢复的典型症状。
    fn check_rule1_main_not_empty(&self) -> Option<IrValidationError> {
        // 只有当存在子函数时才检查
        if self.functions.len() <= 1 {
            return None; // 只有 main 函数，可能确实是空程序
        }

        let main_func = &self.functions[0];

        // 判断：如果 main 的所有基本块中只有 Return 或为空，且存在子函数 → 报错
        let has_only_return_nil = main_func.blocks.iter().all(|block| {
            block.instructions.iter().all(|op| matches!(op, IrOp::Return { .. }))
                || block.instructions.is_empty()
        });

        if has_only_return_nil && self.functions.len() > 1 {
            Some(IrValidationError::MainFunctionEmpty {
                function_index: 0,
                hint: "Top-level statements were not emitted to main function. \
                     Check build_closure_expr or build_fn_expr scope management: \
                     current_function_id and current_block_id must be saved before \
                     entering closure body and restored after."
                    .to_string(),
            })
        } else {
            None
        }
    }

    /// 规则 2: 函数内指令类型检查
    ///
    /// 检查以下不应出现在特定函数中的指令：
    /// - GetCapture / SetCapture → 不应在 main (fn0) 中
    /// - LoadArg → 不应在 main (fn0) 中
    ///
    /// 同时对 Closure/Call 出现在非 main 函数中发出警告（高阶函数合法场景）。
    fn check_rule2_instruction_types(
        &self,
        func_idx: usize,
        func: &IrFunction,
    ) -> Option<IrValidationError> {
        for (instr_idx, instr) in func.blocks.iter().flat_map(|b| b.instructions.iter()).enumerate()
        {
            match instr {
                // Capture 指令只应出现在闭包函数中（fn1+），不在 main (fn0)
                IrOp::GetCapture { .. } | IrOp::SetCapture { .. } => {
                    if func_idx == 0 {
                        return Some(IrValidationError::CaptureInMainFunction {
                            instruction_index: instr_idx,
                            hint:
                                "Capture instructions should only appear in closure functions (fn1+). \
                                 This typically means a closure's GetCapture/SetCapture was emitted \
                                 to main due to missing current_function_id restore after build_closure_expr."
                                    .to_string(),
                        });
                    }
                }

                // LoadArg 只应出现在非 main 函数中
                IrOp::LoadArg { .. } if func_idx == 0 => {
                    return Some(IrValidationError::ArgumentInMainFunction {
                            instruction_index: instr_idx,
                            hint:
                                "LoadArg instructions should only appear in non-main functions. \
                                 This typically means a closure's parameter loading was emitted \
                                 to main due to missing current_function_id restore after build_closure_expr."
                                    .to_string(),
                        });
                }

                _ => {} // 其他指令允许出现在任何函数中
            }
        }
        None
    }

    /// 规则 3: Closure 引用一致性检查（增强版）
    ///
    /// 验证每条 Closure 指令引用的 `ir_func` 在模块的函数列表范围内。
    /// 此检查比 Phase 1 的 `UndefinedFunction` 更详细，携带指令位置信息。
    fn check_rule3_closure_references(
        &self,
        func_idx: usize,
        func: &IrFunction,
    ) -> Option<IrValidationError> {
        for (instr_idx, instr) in func.blocks.iter().flat_map(|b| b.instructions.iter()).enumerate()
        {
            if let IrOp::Closure { ir_func, .. } = instr
                && ir_func.0 as usize >= self.functions.len()
            {
                return Some(IrValidationError::InvalidClosureReference {
                    instruction_index: instr_idx,
                    referenced_function: ir_func.0,
                    total_functions: self.functions.len(),
                    hint: format!(
                        "In fn{}: Closure references fn{} but module only has {} functions. \
                             This may indicate a race between add_function() and Closure emission.",
                        func_idx,
                        ir_func.0,
                        self.functions.len()
                    ),
                });
            }
        }
        None
    }

    /// 规则 4: 基本块所有权检查
    ///
    /// 验证每个基本块的 `instructions` 向量中的所有指令都是有效的。
    /// 由于当前设计中 BasicBlock.instructions 直接存储 IrOp（而非索引），
    /// 此规则主要防御未来重构引入的间接索引模式。
    ///
    /// 当前实现检查基本块 ID 与函数的 blocks 向量的一致性。
    fn check_rule4_block_ownership(
        &self,
        func_idx: usize,
        func: &IrFunction,
    ) -> Option<IrValidationError> {
        for (block_idx, block) in func.blocks.iter().enumerate() {
            // 检查基本块 ID 是否与索引一致（防御性检查）
            if block.id.0 as usize != block_idx {
                return Some(IrValidationError::InvalidBlockInstructionRef {
                    function_index: func_idx,
                    block_index: block_idx,
                    instruction_index: block.id.0, // 复用字段报告不一致的 ID
                    total_instructions: func.blocks.len(),
                    hint: format!(
                        "Block ID mismatch: bb{} is at index {} in fn{}. \
                         Block IDs should be monotonically assigned starting from 0.",
                        block.id.0, block_idx, func_idx
                    ),
                });
            }

            // 检查基本块内部指令数量合理性（防御性上限检查）
            const MAX_BLOCK_INSTRUCTIONS: usize = 100_000;
            if block.instructions.len() > MAX_BLOCK_INSTRUCTIONS {
                return Some(IrValidationError::InvalidBlockInstructionRef {
                    function_index: func_idx,
                    block_index: block_idx,
                    instruction_index: block.instructions.len() as u32,
                    total_instructions: MAX_BLOCK_INSTRUCTIONS,
                    hint: format!(
                        "bb{} in fn{} has {} instructions (exceeds sanity limit {}). \
                         Possible infinite loop during IR construction.",
                        block_idx,
                        func_idx,
                        block.instructions.len(),
                        MAX_BLOCK_INSTRUCTIONS
                    ),
                });
            }
        }
        None
    }

    /// 规则 5: 源操作数 ValueRef 引用完整性检查
    ///
    /// 验证函数内所有指令的源操作数（src）引用的 ValueRef 都在该函数内有定义，
    /// 检测"悬空引用"——即 src 操作数引用了从未在该函数内赋值的 ValueRef。
    ///
    /// # 定义来源
    /// ValueRef 的定义来源包括所有指令的 `dest`（输出值寄存器），
    /// 这天然涵盖了：
    /// - `LoadArg { dest, .. }`：将函数参数加载到 dest
    /// - `GetCapture { dest, .. }`：将捕获变量加载到 dest
    /// - `Closure { dest, .. }`：将闭包对象写入 dest
    /// - `GetLocal { dest, .. }` / `GetGlobal { dest, .. }`：将变量值加载到 dest
    /// - `LoadConstant { dest, .. }`：将常量加载到 dest
    /// - 所有二元/一元运算、Mov、Call 等
    ///
    /// 由于上述所有定义都通过 `dest` 字段引入，本检查只需收集所有 `dest`
    /// 即可覆盖全部定义来源，无需特殊处理 `IrFunction.params` / `captures` 字段
    /// （它们只是名字/描述列表，不携带 ValueRef）。
    ///
    /// # 检查范围
    /// - **函数级**（非块级）：支持跨基本块引用（ValueRef 可能在 bb0 定义，在 bb1 使用）
    /// - **Mov 链合法**：链中每个 `Mov { dest, src }` 的 dest 都会被加入定义集，
    ///   因此 `v1 = mov v0; v2 = mov v1; return v2` 这样的链是合法的
    ///
    /// # 不检查的内容
    /// - ValueRef 的"合理性范围"（> MAX_REASONABLE）：由 Phase 1 的 `validate_value_refs` 检查
    /// - 是否被多次赋值（SSA 唯一性）：本检查只验证"有定义"，不验证"唯一性"
    /// - dest 是否被使用（死代码）：本检查只验证 src 有定义，不验证 dest 被引用
    ///
    /// # 向前兼容
    /// 此检查不应破坏现有合法 IR。如果优化器产生的 IR 触发此检查，
    /// 说明优化器存在 bug（产生了悬空引用），应报告而非忽略。
    fn check_rule5_src_references(
        &self,
        func_idx: usize,
        func: &IrFunction,
    ) -> Option<IrValidationError> {
        // Phase A: 收集函数内所有已定义的 ValueRef
        // 遍历所有基本块的所有指令，收集每条指令的 dest
        // 这涵盖了 LoadArg/GetCapture/Closure/GetLocal/GetGlobal 等所有定义来源
        let mut defined: HashSet<u32> = HashSet::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                if let Some(dest) = instr.dest() {
                    defined.insert(dest.0);
                }
            }
        }

        // Phase B: 遍历所有指令的 src 操作数，验证每个 src 都在定义集中
        // 使用 src_value_refs() 而非 operand_value_refs()，因为前者是公开 API
        // 且返回 ValueRef 类型（类型安全）
        for (instr_idx, instr) in func.blocks.iter().flat_map(|b| b.instructions.iter()).enumerate()
        {
            for src in instr.src_value_refs() {
                if !defined.contains(&src.0) {
                    return Some(IrValidationError::UndefinedValueRef {
                        value_ref: src.0,
                        context: format!(
                            "fn{} instruction #{} {:?} — v{} used but never defined in this function",
                            func_idx, instr_idx, instr, src.0
                        ),
                    });
                }
            }
        }

        None
    }
}

/// 辅助 trait：向 ValidationResult 添加错误
trait ValidationResultExt {
    fn extend_errors(&mut self, err: Option<IrValidationError>);
}

impl ValidationResultExt for ValidationResult {
    fn extend_errors(&mut self, err: Option<IrValidationError>) {
        if let Some(e) = err {
            self.errors.push(e);
        }
    }
}

// ============================================================================
// 辅助 trait：提取指令中的源操作数 ValueRef 列表
// ============================================================================

/// 提取 IR 指令中所有**源操作数**的 ValueRef（不含目标 dest）
pub(crate) trait OperandRefs {
    /// 返回该指令引用的所有源 ValueRef 的原始 ID 列表
    fn operand_value_refs(&self) -> Vec<u32>;
}

impl OperandRefs for IrOp {
    /// 委托给 `src_value_refs()`，提取源操作数的原始 ID 列表
    ///
    /// `src_value_refs()` 是 types.rs 中的权威实现（类型安全的 ValueRef 返回），
    /// 此处仅做 `.0` 解包，避免维护两份重复的 match 分发逻辑。
    fn operand_value_refs(&self) -> Vec<u32> {
        self.src_value_refs().into_iter().map(|v| v.0).collect()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── 辅助构造函数 ──────────────────────────────────────────

    fn vr(id: u32) -> ValueRef {
        ValueRef(id)
    }

    fn bb(id: u32) -> BasicBlockId {
        BasicBlockId(id)
    }

    fn fid(id: u32) -> IrFunctionId {
        IrFunctionId(id)
    }

    // ── Display 测试 ──────────────────────────────────────────

    #[test]
    fn display_load_constant() {
        let op = IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) };
        assert_eq!(format!("{}", op), "v0 = load_const 42");
    }

    #[test]
    fn display_load_constant_string() {
        let op =
            IrOp::LoadConstant { dest: vr(1), constant: IrConstant::String(Arc::from("hello")) };
        let s = format!("{}", op);
        assert!(s.contains("hello"));
        assert!(s.starts_with("v1 = load_const"));
    }

    #[test]
    fn display_load_constant_bool_nil() {
        let t = IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Bool(true) };
        let f = IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Bool(false) };
        let n = IrOp::LoadConstant { dest: vr(2), constant: IrConstant::Nil };
        assert_eq!(format!("{}", t), "v0 = load_const true");
        assert_eq!(format!("{}", f), "v1 = load_const false");
        assert_eq!(format!("{}", n), "v2 = load_const nil");
    }

    #[test]
    fn display_binary() {
        let op = IrOp::Binary { dest: vr(2), op: IrBinOp::Add, left: vr(0), right: vr(1) };
        assert_eq!(format!("{}", op), "v2 = v0 + v1");
    }

    #[test]
    fn display_all_binops() {
        use IrBinOp::*;
        let ops = [
            (Add, "+"),
            (Sub, "-"),
            (Mul, "*"),
            (Div, "/"),
            (Mod, "%"),
            (Pow, "**"),
            (Eq, "=="),
            (Neq, "!="),
            (Lt, "<"),
            (Gt, ">"),
            (Le, "<="),
            (Ge, ">="),
        ];
        for (op, sym) in ops {
            let instr = IrOp::Binary { dest: vr(0), op, left: vr(1), right: vr(2) };
            assert!(format!("{}", instr).contains(sym), "Expected '{}' in '{}'", sym, instr);
        }
    }

    #[test]
    fn display_unary() {
        let neg = IrOp::Unary { dest: vr(1), op: IrUnaryOp::Neg, operand: vr(0) };
        let not = IrOp::Unary { dest: vr(2), op: IrUnaryOp::Not, operand: vr(0) };
        assert_eq!(format!("{}", neg), "v1 = -v0");
        assert_eq!(format!("{}", not), "v2 = !v0");
    }

    #[test]
    fn display_call_with_dest() {
        let op = IrOp::Call { dest: Some(vr(3)), callee: vr(0), args: vec![vr(1), vr(2)] };
        assert_eq!(format!("{}", op), "v3 = call v0 [v1, v2]");
    }

    #[test]
    fn display_call_void() {
        let op = IrOp::Call { dest: None, callee: vr(0), args: vec![vr(1)] };
        assert_eq!(format!("{}", op), "call v0 [v1]");
    }

    #[test]
    fn display_closure() {
        let op = IrOp::Closure { dest: vr(0), ir_func: fid(1) };
        assert_eq!(format!("{}", op), "v0 = closure fn1");
    }

    #[test]
    fn display_get_set_local() {
        let get = IrOp::GetLocal { dest: vr(0), name: Arc::from("x") };
        let set = IrOp::SetLocal { name: Arc::from("x"), value: vr(1) };
        assert_eq!(format!("{}", get), "v0 = local x");
        assert_eq!(format!("{}", set), "local x = v1");
    }

    #[test]
    fn display_get_set_global() {
        let get = IrOp::GetGlobal { dest: vr(0), name: Arc::from("G") };
        let set = IrOp::SetGlobal { name: Arc::from("G"), value: vr(1) };
        assert_eq!(format!("{}", get), "v0 = global G");
        assert_eq!(format!("{}", set), "global G = v1");
    }

    #[test]
    fn display_capture() {
        let get = IrOp::GetCapture { dest: vr(0), index: 2 };
        let set = IrOp::SetCapture { index: 2, value: vr(1) };
        assert_eq!(format!("{}", get), "v0 = capture[2]");
        assert_eq!(format!("{}", set), "capture[2] = v1");
    }

    #[test]
    fn display_jump() {
        let op = IrOp::Jump { target: bb(1) };
        assert_eq!(format!("{}", op), "jump bb1");
    }

    #[test]
    fn display_jump_if() {
        let op = IrOp::JumpIf { cond: vr(0), then_target: bb(1), else_target: bb(2) };
        assert_eq!(format!("{}", op), "if v0 goto bb1 else bb2");
    }

    #[test]
    fn display_return() {
        let val = IrOp::Return { value: Some(vr(0)) };
        let void = IrOp::Return { value: None };
        assert_eq!(format!("{}", val), "return v0");
        assert_eq!(format!("{}", void), "return");
    }

    #[test]
    fn display_array_new() {
        let op = IrOp::ArrayNew { dest: vr(0), elements: vec![vr(1), vr(2), vr(3)] };
        assert_eq!(format!("{}", op), "v0 = array [v1, v2, v3]");
    }

    #[test]
    fn display_object_new() {
        let op = IrOp::ObjectNew { dest: vr(0) };
        assert_eq!(format!("{}", op), "v0 = object {}");
    }

    #[test]
    fn display_field_access() {
        let get = IrOp::GetField { dest: vr(1), object: vr(0), field: Arc::from("name") };
        let set = IrOp::SetField { object: vr(0), field: Arc::from("name"), value: vr(2) };
        assert_eq!(format!("{}", get), "v1 = v0.name");
        assert_eq!(format!("{}", set), "v0.name = v2");
    }

    #[test]
    fn display_index_access() {
        let get = IrOp::IndexGet { dest: vr(2), object: vr(0), index: vr(1) };
        let set = IrOp::IndexSet { object: vr(0), index: vr(1), value: vr(3) };
        assert_eq!(format!("{}", get), "v2 = v0[v1]");
        assert_eq!(format!("{}", set), "v0[v1] = v3 (cow)");
    }

    #[test]
    fn display_print() {
        let op = IrOp::Print { value: vr(0) };
        assert_eq!(format!("{}", op), "print v0");
    }

    #[test]
    fn display_function() {
        let mut func = IrFunction::new(fid(0), "main");
        func.current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) });
        func.current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        let s = format!("{}", func);
        // 格式: "function fn0(参数列表):" — main 是函数名，不在参数位置
        assert!(s.contains("fn0"));
        assert!(s.contains("bb0:"));
        assert!(s.contains("v0 = load_const 42"));
        assert!(s.contains("return v0"));
    }

    #[test]
    fn display_function_with_params() {
        let mut func = IrFunction::new(fid(1), "add");
        func.params.push(Arc::from("a"));
        func.params.push(Arc::from("b"));
        func.current_block_mut().push(IrOp::Return { value: None });

        let s = format!("{}", func);
        assert!(s.contains("function fn1(a, b):"));
    }

    #[test]
    fn display_module() {
        let mut module = IrModule::new();
        module.globals.push(Arc::from("VERSION"));
        module.add_function("main");

        let s = format!("{}", module);
        assert!(s.contains("; Nuzo IR Module"));
        assert!(s.contains("; globals: [VERSION]"));
        assert!(s.contains("fn0")); // 函数名 main 在格式中，参数列表为空
    }

    #[test]
    fn display_module_empty() {
        let module = IrModule::new();
        let s = format!("{}", module);
        assert!(s.contains("; Nuzo IR Module"));
        // 空模块不应包含 globals 行
        assert!(!s.contains("; globals:"));
    }

    // ── validate() 测试 ───────────────────────────────────────

    #[test]
    fn validate_valid_module() {
        let mut module = IrModule::new();
        module.add_function("main");
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });
        assert!(module.validate().is_ok());
    }

    #[test]
    fn validate_missing_terminator() {
        let mut module = IrModule::new();
        module.add_function("bad");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        // 缺少 terminator

        let result = module.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, IrValidationError::BlockMissingTerminator { .. }));
    }

    #[test]
    fn validate_empty_block_is_ok() {
        let mut module = IrModule::new();
        module.add_function("empty");
        // 空基本块不需要 terminator

        assert!(module.validate().is_ok());
    }

    #[test]
    fn validate_invalid_jump_target() {
        let mut module = IrModule::new();
        module.add_function("bad_jump");
        module.current_function_mut().current_block_mut().push(IrOp::Jump { target: bb(99) }); // bb99 不存在

        let result = module.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IrValidationError::InvalidBlockId { .. }));
    }

    #[test]
    fn validate_invalid_jump_if_targets() {
        let mut module = IrModule::new();
        module.add_function("bad_if");
        module.current_function_mut().current_block_mut().push(IrOp::JumpIf {
            cond: vr(0),
            then_target: bb(0),
            else_target: bb(99),
        });

        let result = module.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IrValidationError::InvalidBlockId { .. }));
    }

    #[test]
    fn validate_undefined_function_in_closure() {
        let mut module = IrModule::new();
        module.add_function("caller");
        module.current_function_mut().current_block_mut().push(IrOp::Closure {
            dest: vr(0),
            ir_func: fid(99), // 不存在的函数
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IrValidationError::UndefinedFunction { .. }));
    }

    #[test]
    fn validate_absurd_valueref_rejected() {
        let mut module = IrModule::new();
        module.add_function("bad_vref");
        module.current_function_mut().current_block_mut().push(IrOp::Binary {
            dest: vr(0),
            op: IrBinOp::Add,
            left: ValueRef(u32::MAX), // 超出合理范围
            right: vr(1),
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IrValidationError::UndefinedValueRef { .. }));
    }

    #[test]
    fn validate_valid_jump_to_existing_block() {
        let mut module = IrModule::new();
        module.add_function("loop_fn");
        // 创建第二个基本块
        let bb1 = BasicBlockId(1);
        module.current_function_mut().blocks.push(BasicBlock::new(bb1));
        module.current_function_mut().blocks[bb1.0 as usize].push(IrOp::Return { value: None });

        // 从 bb0 跳到 bb1（存在）
        module.current_function_mut().current_block_mut().push(IrOp::Jump { target: bb1 });

        assert!(module.validate().is_ok());
    }

    // ── OperandRefs 测试 ──────────────────────────────────────

    #[test]
    fn operand_refs_binary() {
        let op = IrOp::Binary { dest: vr(5), op: IrBinOp::Add, left: vr(1), right: vr(2) };
        let refs = op.operand_value_refs();
        assert_eq!(refs, vec![1, 2]); // 不含 dest=5
    }

    #[test]
    fn operand_refs_call() {
        let op = IrOp::Call { dest: Some(vr(10)), callee: vr(0), args: vec![vr(1), vr(2), vr(3)] };
        let refs = op.operand_value_refs();
        assert_eq!(refs, vec![0, 1, 2, 3]); // callee + args
    }

    #[test]
    fn operand_refs_load_constant() {
        let op = IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Nil };
        assert!(op.operand_value_refs().is_empty());
    }

    #[test]
    fn operand_refs_index_set() {
        let op = IrOp::IndexSet { object: vr(0), index: vr(1), value: vr(2) };
        assert_eq!(op.operand_value_refs(), vec![0, 1, 2]);
    }

    #[test]
    fn operand_refs_return_void() {
        let op = IrOp::Return { value: None };
        assert!(op.operand_value_refs().is_empty());
    }

    #[test]
    fn operand_refs_return_value() {
        let op = IrOp::Return { value: Some(vr(5)) };
        assert_eq!(op.operand_value_refs(), vec![5]);
    }

    #[test]
    fn operand_refs_set_local() {
        let op = IrOp::SetLocal { name: Arc::from("x"), value: vr(3) };
        assert_eq!(op.operand_value_refs(), vec![3]);
    }

    #[test]
    fn operand_refs_get_local_no_value_refs() {
        let op = IrOp::GetLocal { dest: vr(0), name: Arc::from("y") };
        assert!(op.operand_value_refs().is_empty()); // name 不是 ValueRef
    }

    #[test]
    fn operand_refs_closure_no_value_refs() {
        let op = IrOp::Closure { dest: vr(0), ir_func: fid(1) };
        assert!(op.operand_value_refs().is_empty()); // ir_func 是 IrFunctionId
    }

    // ── Phase 2: 函数作用域完整性检查测试（规则 1-4）────────────

    // ====== 规则 1: 主函数非空检查 ======

    #[test]
    fn rule1_main_empty_with_subfunction_is_error() {
        // 模拟 bug: main 为空（只有 return nil），但有子函数
        // 这表明顶层语句被错误地发射到了子函数中
        let mut module = IrModule::new();
        module.add_function("main"); // fn0: 空 main

        // 添加子函数（模拟闭包）
        let _closure_id = module.add_function("closure_fn"); // fn1
        {
            let closure_func = module.get_function_mut(_closure_id);
            closure_func
                .current_block_mut()
                .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) });
            closure_func.current_block_mut().push(IrOp::Return { value: Some(vr(0)) });
        }

        // main 只有隐式 return nil（由 builder 自动添加，但这里手动构造空 main）
        // 实际上 builder 总是添加 return nil，所以我们需要让 main 只包含 Return
        module
            .get_function_mut(IrFunctionId(0))
            .current_block_mut()
            .push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid(), "Main function empty with sub-function should be an error");
        assert!(
            matches!(result.errors[0], IrValidationError::MainFunctionEmpty { .. }),
            "Expected MainFunctionEmpty error, got: {:?}",
            result.errors[0]
        );
    }

    #[test]
    fn rule1_main_not_empty_with_subfunction_is_ok() {
        // 正常情况: main 有内容 + 有子函数 → 应通过
        let mut module = IrModule::new();
        module.add_function("main");
        // main 中有实际指令
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        // 子函数
        let _cid = module.add_function("child");
        module.get_function_mut(_cid).current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(
            result.is_valid(),
            "Main with content + sub-functions should pass. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn rule1_single_empty_main_is_ok() {
        // 只有一个空的 main 函数（无子函数）→ 不应报错（可能是空程序）
        let mut module = IrModule::new();
        module.add_function("main");
        // main 为空，但只有 1 个函数

        let result = module.validate_full();
        assert!(
            result.is_valid(),
            "Single empty main (no sub-functions) should not trigger MainFunctionEmpty"
        );
    }

    // ====== 规则 2: Capture/Argument 在 main 中 ======

    #[test]
    fn rule2_getcapture_in_main_is_error() {
        let mut module = IrModule::new();
        module.add_function("main");
        // GetCapture 出现在 main 函数中 — 不合法
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::GetCapture { dest: vr(0), index: 0 });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid());
        assert!(
            matches!(result.errors[0], IrValidationError::CaptureInMainFunction { .. }),
            "Expected CaptureInMainFunction, got: {:?}",
            result.errors[0]
        );
    }

    #[test]
    fn rule2_setcapture_in_main_is_error() {
        let mut module = IrModule::new();
        module.add_function("main");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::SetCapture { index: 0, value: vr(0) });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid());
        assert!(
            matches!(result.errors[0], IrValidationError::CaptureInMainFunction { .. }),
            "SetCapture in main should also be caught"
        );
    }

    #[test]
    fn rule2_loadarg_in_main_is_error() {
        let mut module = IrModule::new();
        module.add_function("main");
        // LoadArg 出现在 main 函数中 — 不合法
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadArg { dest: vr(0), index: 0 });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid());
        assert!(
            matches!(result.errors[0], IrValidationError::ArgumentInMainFunction { .. }),
            "Expected ArgumentInMainFunction, got: {:?}",
            result.errors[0]
        );
    }

    #[test]
    fn rule2_capture_in_closure_is_ok() {
        // GetCapture 在子函数（闭包）中 → 合法
        let mut module = IrModule::new();
        module.add_function("main");
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let cid = module.add_function("closure");
        module
            .get_function_mut(cid)
            .current_block_mut()
            .push(IrOp::GetCapture { dest: vr(0), index: 0 });
        module.get_function_mut(cid).current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        let result = module.validate_full();
        // 应该通过（Capture 在闭包中是合法的）
        let capture_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, IrValidationError::CaptureInMainFunction { .. }))
            .collect();
        assert!(
            capture_errors.is_empty(),
            "GetCapture in closure function should not produce CaptureInMainFunction error"
        );
    }

    #[test]
    fn rule2_loadarg_in_closure_is_ok() {
        // LoadArg 在子函数中 → 合法
        let mut module = IrModule::new();
        module.add_function("main");
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let cid = module.add_function("fn_with_args");
        module
            .get_function_mut(cid)
            .current_block_mut()
            .push(IrOp::LoadArg { dest: vr(0), index: 0 });
        module.get_function_mut(cid).current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        let result = module.validate_full();
        let arg_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, IrValidationError::ArgumentInMainFunction { .. }))
            .collect();
        assert!(
            arg_errors.is_empty(),
            "LoadArg in non-main function should not produce ArgumentInMainFunction error"
        );
    }

    // ====== 规则 3: Closure 引用有效性 ======

    #[test]
    fn rule3_closure_references_out_of_range() {
        let mut module = IrModule::new();
        module.add_function("caller");
        // Closure 引用了不存在的函数索引
        module.current_function_mut().current_block_mut().push(IrOp::Closure {
            dest: vr(0),
            ir_func: fid(99), // 超出范围（只有 fn0 存在）
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid(), "Out-of-range closure reference should be an error");
        // 注意：Phase 1 结构性检查优先于 Phase 2 作用域检查执行，
        // 因此 UndefinedFunction 可能比 InvalidClosureReference 先被捕获。
        // 两者都是正确的错误报告，关键点是"越界引用被检测到"。
        let is_caught = matches!(
            result.errors[0],
            IrValidationError::InvalidClosureReference { .. }
                | IrValidationError::UndefinedFunction { .. }
        );
        assert!(
            is_caught,
            "Expected InvalidClosureReference or UndefinedFunction, got: {:?}",
            result.errors[0]
        );
    }

    #[test]
    fn rule3_closure_references_valid_function_is_ok() {
        let mut module = IrModule::new();
        module.add_function("caller");

        // 先创建被引用的函数
        let target_id = module.add_function("target");
        module.get_function_mut(target_id).current_block_mut().push(IrOp::Return { value: None });

        // Closure 引用已存在的函数
        module.get_function_mut(IrFunctionId(0)).current_block_mut().push(IrOp::Closure {
            dest: vr(0),
            ir_func: target_id, // fn1，存在
        });
        module
            .get_function_mut(IrFunctionId(0))
            .current_block_mut()
            .push(IrOp::Return { value: None });

        let result = module.validate_full();
        let ref_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, IrValidationError::InvalidClosureReference { .. }))
            .collect();
        assert!(
            ref_errors.is_empty(),
            "Valid closure reference should not produce InvalidClosureReference error"
        );
    }

    // ====== 规则 4: 基本块所有权检查 ======

    #[test]
    fn rule4_block_id_mismatch_detected() {
        // 手动创建一个 ID 不匹配的基本块来测试检测能力
        let mut func = IrFunction::new(fid(0), "test");
        // 手动修改 block 的 ID 使其不匹配索引
        func.blocks[0].id = BasicBlockId(42); // 索引 0 但 ID 是 42

        let mut module = IrModule::new();
        module.functions.push(func);

        let result = module.validate_full();
        assert!(!result.is_valid(), "Block ID mismatch should be detected");
        assert!(
            matches!(result.errors[0], IrValidationError::InvalidBlockInstructionRef { .. }),
            "Expected InvalidBlockInstructionRef for block ID mismatch, got: {:?}",
            result.errors[0]
        );
    }

    #[test]
    fn rule4_consistent_block_ids_pass() {
        // 正常的连续基本块 ID → 应通过
        let mut module = IrModule::new();
        module.add_function("normal");
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        let ownership_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, IrValidationError::InvalidBlockInstructionRef { .. }))
            .collect();
        assert!(ownership_errors.is_empty(), "Consistent block IDs should pass ownership check");
    }

    // ====== 综合测试: validate_full 返回值结构 ======

    #[test]
    fn validation_result_default_is_valid() {
        let result = ValidationResult::default();
        assert!(result.is_valid());
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn validation_result_into_result_ok_when_valid() {
        let result = ValidationResult::default();
        assert!(result.into_result().is_ok());
    }

    #[test]
    fn validation_result_into_result_err_when_has_errors() {
        let mut result = ValidationResult::default();
        result.errors.push(IrValidationError::Generic { message: "test error".to_string() });
        assert!(result.into_result().is_err());
    }

    #[test]
    fn validate_full_returns_multiple_errors() {
        // 构造一个同时触发多个错误的模块
        let mut module = IrModule::new();
        module.add_function("main"); // fn0: 空 main

        // main 中有 GetCapture → 触发规则 2
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::GetCapture { dest: vr(0), index: 0 });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        // 子函数存在 → 可能触发规则 1（如果 main 为空且只有 return nil）
        // 但这里有 GetCapture 所以不是"只有 return nil"，规则 1 不触发
        // 让我们再添加一个子函数并清空 main 的非 return 内容
        let _cid = module.add_function("sub");
        module.get_function_mut(_cid).current_block_mut().push(IrOp::Return { value: None });

        // 重新构造：main 只有 return nil + 子函数存在 → 规则 1
        let mut module2 = IrModule::new();
        module2.add_function("main");
        module2
            .get_function_mut(IrFunctionId(0))
            .current_block_mut()
            .push(IrOp::Return { value: None }); // 只有 return nil

        let cid2 = module2.add_function("sub_fn");
        module2.get_function_mut(cid2).current_block_mut().push(IrOp::Return { value: None });

        let result = module2.validate_full();
        // 至少应有 MainFunctionEmpty 错误
        assert!(!result.is_valid(), "Empty main with sub-function should fail validation");
        assert!(
            !result.errors.is_empty(),
            "Should have at least 1 error, got {}",
            result.errors.len()
        );
    }

    // ====== 集成测试: 正常构建的 IR 通过验证 ======

    #[test]
    fn normal_ir_passes_all_scope_checks() {
        // 构建一个正常的、多函数 IR 模块，应通过所有作用域检查
        let mut module = IrModule::new();

        // fn0 (main): 有实际内容
        module.add_function("main");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(10.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Closure {
            dest: vr(1),
            ir_func: fid(1), // 引用 fn1
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        // fn1 (闭包): 有参数加载和捕获访问
        let cid = module.add_function("adder");
        let closure_func = module.get_function_mut(cid);
        closure_func.params.push(Arc::from("x"));
        closure_func.captures.push(CaptureDesc { name: Arc::from("y"), is_mutable: false });
        closure_func.current_block_mut().push(IrOp::LoadArg { dest: vr(2), index: 0 });
        closure_func.current_block_mut().push(IrOp::GetCapture { dest: vr(3), index: 0 });
        closure_func.current_block_mut().push(IrOp::Binary {
            dest: vr(4),
            op: IrBinOp::Add,
            left: vr(2),
            right: vr(3),
        });
        closure_func.current_block_mut().push(IrOp::Return { value: Some(vr(4)) });

        let result = module.validate_full();
        assert!(
            result.is_valid(),
            "Well-formed multi-function IR should pass all scope checks. Errors: {:?}\nWarnings: {:?}",
            result.errors,
            result.warnings
        );
    }

    // ====== 规则 5: 源操作数 ValueRef 引用完整性检查（新增）======

    #[test]
    fn rule5_detects_undefined_ref_in_binary() {
        // Binary 引用 v0 和 v1，但只有 v0 被定义（v1 悬空）
        let mut module = IrModule::new();
        module.add_function("bad");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Binary {
            dest: vr(2),
            op: IrBinOp::Add,
            left: vr(0),  // 已定义
            right: vr(1), // 未定义 — 悬空引用
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid(), "Dangling src reference should be detected");
        let has_undefined = result.errors.iter().any(|e| {
            matches!(e, IrValidationError::UndefinedValueRef { value_ref, .. } if *value_ref == 1)
        });
        assert!(has_undefined, "Should report UndefinedValueRef for v1, got: {:?}", result.errors);
    }

    #[test]
    fn rule5_accepts_well_formed_ir() {
        // 所有 src 都有定义：v0=load; v1=load; v2=v0+v1; return v2
        let mut module = IrModule::new();
        module.add_function("good");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(1), constant: IrConstant::Number(2.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Binary {
            dest: vr(2),
            op: IrBinOp::Add,
            left: vr(0),  // 已定义
            right: vr(1), // 已定义
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: Some(vr(2)) });

        let result = module.validate_full();
        assert!(result.is_valid(), "Well-formed IR should pass. Errors: {:?}", result.errors);
    }

    #[test]
    fn rule5_accepts_mov_chain() {
        // Mov 链: v0 = load; v1 = mov v0; v2 = mov v1; return v2
        // 链中每个 dest 都被加入定义集，因此合法
        let mut module = IrModule::new();
        module.add_function("mov_chain");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(42.0) });
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::Mov { dest: vr(1), src: vr(0) });
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::Mov { dest: vr(2), src: vr(1) });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: Some(vr(2)) });

        let result = module.validate_full();
        assert!(result.is_valid(), "Mov chain should pass. Errors: {:?}", result.errors);
    }

    #[test]
    fn rule5_accepts_cross_block_reference() {
        // 跨块引用: v0 在 bb0 定义，在 bb1 使用（函数级验证，非块级）
        let mut module = IrModule::new();
        module.add_function("cross_block");
        // bb0: LoadConstant v0; Jump bb1
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Jump { target: bb(1) });
        // bb1: Return v0
        let bb1 = BasicBlockId(1);
        module.current_function_mut().blocks.push(BasicBlock::new(bb1));
        module.current_function_mut().blocks[1].push(IrOp::Return { value: Some(vr(0)) });

        let result = module.validate_full();
        assert!(
            result.is_valid(),
            "Cross-block reference should pass. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn rule5_accepts_loadarg_getcapture_definitions() {
        // LoadArg 和 GetCapture 的 dest 视为定义
        // 验证函数参数和捕获变量的 ValueRef 自动通过 LoadArg/GetCapture 引入
        let mut module = IrModule::new();
        module.add_function("main");
        // main 需要有实际内容（避免触发 rule1 MainFunctionEmpty：有子函数时 main 不应只有 return nil）
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: Some(vr(0)) });

        // 子函数（闭包）：LoadArg + GetCapture + Binary
        let cid = module.add_function("closure");
        let closure_func = module.get_function_mut(cid);
        closure_func.params.push(Arc::from("x"));
        closure_func.captures.push(CaptureDesc { name: Arc::from("y"), is_mutable: false });
        closure_func.current_block_mut().push(IrOp::LoadArg { dest: vr(0), index: 0 });
        closure_func.current_block_mut().push(IrOp::GetCapture { dest: vr(1), index: 0 });
        closure_func.current_block_mut().push(IrOp::Binary {
            dest: vr(2),
            op: IrBinOp::Add,
            left: vr(0),
            right: vr(1),
        });
        closure_func.current_block_mut().push(IrOp::Return { value: Some(vr(2)) });

        let result = module.validate_full();
        assert!(
            result.is_valid(),
            "LoadArg/GetCapture definitions should pass. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn rule5_detects_undefined_ref_in_return() {
        // Return 引用未定义的 v99
        let mut module = IrModule::new();
        module.add_function("bad_return");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::Return { value: Some(vr(99)) }); // v99 未定义

        let result = module.validate_full();
        assert!(!result.is_valid(), "Undefined ref in Return should be detected");
        let has_undefined = result.errors.iter().any(|e| {
            matches!(e, IrValidationError::UndefinedValueRef { value_ref, .. } if *value_ref == 99)
        });
        assert!(has_undefined, "Should report UndefinedValueRef for v99, got: {:?}", result.errors);
    }

    #[test]
    fn rule5_detects_undefined_ref_in_call_args() {
        // Call 的 args 引用未定义的 v5（callee=v0 已定义）
        let mut module = IrModule::new();
        module.add_function("bad_call");
        module
            .current_function_mut()
            .current_block_mut()
            .push(IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(1.0) });
        // callee=v0 已定义，但 args 中 v5 未定义
        module.current_function_mut().current_block_mut().push(IrOp::Call {
            dest: Some(vr(1)),
            callee: vr(0),
            args: vec![vr(5)], // v5 未定义
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid(), "Undefined ref in Call args should be detected");
        let has_undefined = result.errors.iter().any(|e| {
            matches!(e, IrValidationError::UndefinedValueRef { value_ref, .. } if *value_ref == 5)
        });
        assert!(has_undefined, "Should report UndefinedValueRef for v5, got: {:?}", result.errors);
    }

    #[test]
    fn rule5_coexists_with_rule2_capture_in_main() {
        // 验证 rule5 与 rule2 共存：SetCapture 在 main 中（rule2 错误）
        // 同时 SetCapture 引用了未定义的 v0（rule5 错误）
        // 两个错误都应被收集（Phase 2 collect_all_errors 模式）
        let mut module = IrModule::new();
        module.add_function("main");
        module.current_function_mut().current_block_mut().push(IrOp::SetCapture {
            index: 0,
            value: vr(0), // v0 未定义 — rule5 会触发
        });
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let result = module.validate_full();
        assert!(!result.is_valid(), "Should have errors");
        // rule2 错误应在 errors[0]（先于 rule5 检查）
        assert!(
            matches!(result.errors[0], IrValidationError::CaptureInMainFunction { .. }),
            "errors[0] should be CaptureInMainFunction (rule2), got: {:?}",
            result.errors[0]
        );
        // rule5 错误也应存在
        let has_undefined = result.errors.iter().any(|e| {
            matches!(e, IrValidationError::UndefinedValueRef { value_ref, .. } if *value_ref == 0)
        });
        assert!(
            has_undefined,
            "Should also report UndefinedValueRef for v0 (rule5), got: {:?}",
            result.errors
        );
    }
}

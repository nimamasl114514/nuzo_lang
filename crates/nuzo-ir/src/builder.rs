//! IR Builder — 将 AST 转换为与目标无关的三地址码中间表示
//!
//! ## 设计原则
//! - SSA 风格：每个 ValueRef 只赋值一次
//! - 三地址码：每条指令最多 1 个目标 + 2 个源操作数
//! - 目标无关：不依赖具体寄存器分配或字节码格式
//!
//! ## 表达式处理策略
//! 参照 `nuzo_compiler::expressions.rs` 的语义，保持行为一致：
//! - Ident: 三级查找（局部 → 捕获 → 全局）
//! - Binary: 声明式映射 AST BinaryOp → IrBinOp
//! - Unary: Negate → Neg, Not → Not
//! - Call/Index/Field: 与编译器一致的指令发射顺序

use crate::error::IrBuildError;
use crate::module_resolver::{ModuleResolver, NullResolver, ResolveError};
use crate::types::*;
use nuzo_core::SourceLocation;
use nuzo_frontend::ast::{self, Expr, ExprVisitor, MatchPattern, Span};
use nuzo_frontend::parser::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 AST Span 转换为 SourceLocation（用于错误报告）
fn span_to_location(span: &Span) -> SourceLocation {
    SourceLocation {
        file: String::new(),
        line: span.line,
        column: span.column,
        source_line: None,
        function_name: None,
    }
}

// ============================================================================
// 循环上下文（用于 break/continue 目标解析）
// ============================================================================

/// 循环上下文 — 记录当前循环的出口和继续块
struct LoopContext {
    /// 循环出口块 ID（break 跳转目标）
    exit_block: BasicBlockId,
    /// 循环继续块 ID（continue 跳转目标，通常是条件判断块）
    continue_block: BasicBlockId,
    /// 循环结果 ValueRef（break 带值时写入此寄存器，None 表示无结果）
    result: Option<ValueRef>,
}

// ============================================================================
// IR 构建器 — 管理 IR 构建过程中的状态
// ============================================================================

/// IR 构建器 — 将 AST 表达式转换为三地址码 IR
///
/// 管理虚拟寄存器分配、基本块创建、局部变量作用域等构建状态。
/// Phase 1 聚焦表达式核心转换，语句级完整控制流在后续 Phase 补充。
pub struct IrBuilder {
    /// 正在构建的 IR 模块
    module: IrModule,
    /// 虚拟寄存器计数器（单调递增，用于分配 ValueRef）
    next_value_ref: u32,
    /// 当前作用域中的局部变量名 → ValueRef 映射（栈式作用域）
    locals: Vec<(Arc<str>, ValueRef)>,
    /// 是否正在构建函数体（用于区分顶层代码和函数内部）
    in_function: bool,
    /// 循环上下文栈（break/continue 目标基本块）
    loop_stack: Vec<LoopContext>,
    /// 当前基本块 ID（用于指令发射定位）
    current_block_id: BasicBlockId,
    /// 当前正在构建的函数 ID（用于精确定位目标函数，避免闭包编译后指令写入错误函数）
    current_function_id: IrFunctionId,
    /// 当前闭包的捕获列表（用于嵌套闭包的 Path 2 查找：外层捕获变量）
    /// 当进入闭包体时压入当前闭包的 captures，退出时弹出
    capture_stack: Vec<Vec<CaptureDesc>>,
    /// 🔧 即将注册为全局函数的名字集合（用于匿名闭包的自由变量过滤）。
    /// 当遇到 `name = fn(...) { ... }` 赋值时，在构建闭包前将 name 加入此集合，
    /// 使 build_closure_expr 中的 is_global_function 能识别这个"即将成为全局函数"的名字，
    /// 避免将其错误地当作闭包捕获变量（导致运行时 IndexOutOfBounds）。
    pending_global_fns: std::collections::HashSet<Arc<str>>,
    /// 🔧 全局函数名集合（用于前向引用/互递归解析 + 内置函数识别）。
    /// 两个来源：
    /// 1. build() 入口由调用方注入的外部全局函数名（如 VM 内置函数，来自 BuiltinRegistry）
    /// 2. pre_scan_global_fns() 预扫描收集的顶层具名函数名（`fn name(...){...}`）
    ///
    /// 使 a 引用 b（b 在 a 之后定义）时，b 能被识别为全局函数而非捕获变量。
    known_global_fns: std::collections::HashSet<Arc<str>>,
    /// SCSB 活动切片链：变量名 → SliceChain 的 ValueRef
    /// 在循环内检测到 `s = s + expr` 模式时，在循环前创建 SliceChain 并存入此表。
    /// build_assign 中检查此表，将 `s = s + expr` 替换为 SliceChainAppend。
    /// 循环结束后通过 SliceChainFinish 提取结果。
    scsb_chains: std::collections::HashMap<Arc<str>, ValueRef>,
}

/// 检查表达式中是否包含字符串字面量
///
/// 用于 SCSB 模式检测：只有当 `s = s + expr` 中 expr 包含字符串字面量时，
/// 才将其识别为字符串拼接而非数值加法。
///
/// 预留辅助函数：当前 SCSB 检测由 extract_self_concat_operands 内联完成
/// （其内部 walk 同时收集操作数与字符串标记），此函数保留供未来独立的
/// 字符串字面量预筛路径、诊断工具或 AST 重写 pass 复用，避免 API 反复变更。
#[allow(dead_code)]
fn expr_contains_string_literal(expr: &Expr) -> bool {
    match expr {
        ast::Expr::String { .. } => true,
        ast::Expr::Binary { left, right, .. } => {
            expr_contains_string_literal(left) || expr_contains_string_literal(right)
        }
        _ => false,
    }
}

/// 从 `s + a + b + ...` 链中提取非自身操作数
///
/// 如果表达式是 `s + rest` 或 `rest + s` 的 `+` 链，且包含至少一个字符串字面量，
/// 返回所有非自身的操作数列表。否则返回 None。
///
/// 例如：`s + "x" + i` → 返回 `Some(["x", i])`
///      `s + s` → 返回 `Some([])`（无非自身操作数，不触发 SCSB）
///      `s + 1` → 返回 None（无数值加法不触发 SCSB）
fn extract_self_concat_operands<'a>(expr: &'a Expr, self_name: &str) -> Option<Vec<&'a Expr>> {
    let mut operands = Vec::new();
    let mut has_string = false;
    let mut has_self = false;

    fn walk<'a>(
        expr: &'a Expr,
        self_name: &str,
        operands: &mut Vec<&'a Expr>,
        has_string: &mut bool,
        has_self: &mut bool,
    ) {
        match expr {
            ast::Expr::Binary { left, op: ast::BinaryOp::Add, right, .. } => {
                walk(left, self_name, operands, has_string, has_self);
                walk(right, self_name, operands, has_string, has_self);
            }
            ast::Expr::Ident { name, .. } if name == self_name => {
                *has_self = true;
            }
            ast::Expr::String { .. } => {
                *has_string = true;
                operands.push(expr);
            }
            _ => {
                operands.push(expr);
            }
        }
    }

    walk(expr, self_name, &mut operands, &mut has_string, &mut has_self);

    if has_self && has_string { Some(operands) } else { None }
}

impl IrBuilder {
    /// 创建新的 IR 构建器实例
    pub fn new() -> Self {
        let mut module = IrModule::new();
        // 预创建 main 函数（入口函数），保存其 ID 作为初始当前函数
        let main_func_id = module.add_function("main");
        Self {
            module,
            next_value_ref: 0,
            locals: Vec::new(),
            in_function: false,
            loop_stack: Vec::new(),
            current_block_id: BasicBlockId(0),
            current_function_id: main_func_id,
            capture_stack: Vec::new(),
            pending_global_fns: std::collections::HashSet::new(),
            known_global_fns: std::collections::HashSet::new(),
            scsb_chains: std::collections::HashMap::new(),
        }
    }

    // ========================================================================
    // 公共入口：AST Program → IR Module
    // ========================================================================

    /// 构建完整 IR 模块
    ///
    /// 将 AST 程序转换为 IR 中间表示。
    /// 创建 main 函数作为入口点，将顶层语句编译到其中。
    ///
    /// # 参数
    ///
    /// * `program` - AST 程序根节点
    /// * `global_fn_names` - 调用方提供的全局函数名列表（如 VM 内置函数），
    ///   用于闭包捕获过滤：当自由变量名命中此列表时走 GetGlobal 路径而非 GetCapture。
    ///   传 `&[]` 表示无外部全局函数（仅识别用户定义的顶层函数）。
    ///
    /// # Debug 模式验证
    /// 在 `debug_assertions` 启用时（即 debug 构建），构建完成后会自动运行
    /// 完整的 IR 合法性验证（结构性 + 函数作用域完整性检查）。
    /// 如果验证失败，返回详细的错误信息帮助定位作用域管理 bug。
    pub fn build(
        program: &ast::Program,
        global_fn_names: &[&str],
    ) -> Result<IrModule, IrBuildError> {
        // 向后兼容：使用 NullResolver，无 import 处理能力。
        // 当源码不含 import 时行为与原 build 完全一致。
        Self::build_with_resolver(program, global_fn_names, &NullResolver, None)
    }

    /// 构建完整 IR 模块（带模块解析器）
    ///
    /// 与 [`build`](Self::build) 的区别：注入 [`ModuleResolver`] 用于解析 import 语句。
    /// 当源程序包含 `import "path"` 时，会递归编译依赖模块，结果存入共享缓存。
    ///
    /// # 参数
    /// - `program`: AST 程序根节点
    /// - `global_fn_names`: 调用方提供的全局函数名列表（如 VM 内置函数）
    /// - `resolver`: 模块路径解析器（实现 [`ModuleResolver`] trait）
    /// - `current_path`: 当前模块的源文件路径（用于相对路径解析；`None` 表示顶层入口）
    ///
    /// # 循环依赖检测
    /// 使用 DFS 灰白标记法（stack 参数）检测循环 import，最大深度 100 层。
    ///
    /// # 错误
    /// - [`IrBuildError::Error`]: 包装 [`ResolveError`]（ModuleNotFound/CircularImport/...）
    /// - 其他 [`IrBuildError`] 变体：构建期错误
    pub fn build_with_resolver(
        program: &ast::Program,
        global_fn_names: &[&str],
        resolver: &dyn ModuleResolver,
        current_path: Option<&Path>,
    ) -> Result<IrModule, IrBuildError> {
        // 每个顶层入口创建独立的 cache 与 stack；
        // 递归编译通过 build_with_imports 共享这两个结构。
        let mut cache: HashMap<PathBuf, Arc<IrModule>> = HashMap::new();
        let mut stack: Vec<PathBuf> = Vec::new();
        Self::build_with_imports(
            program,
            global_fn_names,
            resolver,
            current_path,
            &mut cache,
            &mut stack,
        )
    }

    /// 递归编译 worker — 接受共享 cache 与 stack
    ///
    /// 与 [`build_with_resolver`](Self::build_with_resolver) 的区别：
    /// 此函数暴露 cache/stack 参数供 [`resolve_imports`](Self::resolve_imports) 递归调用，
    /// 确保子模块编译结果可被同一导入链中的兄弟模块复用。
    fn build_with_imports(
        program: &ast::Program,
        global_fn_names: &[&str],
        resolver: &dyn ModuleResolver,
        current_path: Option<&Path>,
        cache: &mut HashMap<PathBuf, Arc<IrModule>>,
        stack: &mut Vec<PathBuf>,
    ) -> Result<IrModule, IrBuildError> {
        let mut builder = Self::new();
        // 注入调用方提供的全局函数名（如 VM 内置函数），用于闭包捕获过滤。
        // 复用 known_global_fns 集合，与预扫描的用户函数名统一处理。
        for name in global_fn_names {
            builder.known_global_fns.insert(Arc::from(*name));
        }

        // 解析 imports（递归编译依赖模块，结果存入 cache）
        let import_records = Self::resolve_imports(program, resolver, current_path, cache, stack)?;

        // 填充 module.path / module.imports
        if let Some(path) = current_path {
            builder.module.path = Some(path.to_path_buf());
        }
        builder.module.imports = import_records.clone();

        // 🔧 预扫描：收集所有顶层具名函数定义的名字 + import 符号（含重名检测）
        builder.pre_scan_global_fns(&program.statements, &import_records)?;

        // 🔧 合并导入子模块的 IrFunction 定义到主模块
        // 子模块的 main 入口 (id=0) 被跳过；其余具名函数被合并、重新编号，
        // 并在主函数中发射 Closure + SetGlobal 使其成为可调用的全局函数。
        builder.merge_imported_functions(&import_records)?;

        // 构建所有顶层语句，同时追踪最后一个表达式的值
        // （脚本中最后一个表达式的值作为隐式返回值，类似 Ruby 的脚本语义）
        let last_val = builder.build_statements_with_last_expr(&program.statements)?;
        // 在 main 末尾添加隐式 return（使用最后一个表达式的值，而非总是 nil）
        // 这确保 `fn f() { ... }; f()` 这样的代码能正确返回 f() 的调用结果
        builder.emit_return(Some(last_val))?;

        // 🔧 Debug 模式：自动运行完整性检查，防止函数作用域管理 bug 复发
        #[cfg(debug_assertions)]
        {
            let validation_result = builder.module.validate_full();
            if !validation_result.is_valid() {
                eprintln!(
                    "[IR Validator] Found {} validation error(s) after build_with_imports():",
                    validation_result.errors.len()
                );
                for err in &validation_result.errors {
                    eprintln!("  [ERROR] {}", err);
                }
                for warn in &validation_result.warnings {
                    eprintln!("  [WARN]  {}", warn);
                }
                // 将第一个验证错误转换为 IrBuildError 以便调用者感知
                let first_err = &validation_result.errors[0];
                return Err(IrBuildError::Error {
                    message: format!("IR validation failed: {}", first_err),
                    location: SourceLocation {
                        file: String::new(),
                        line: 0,
                        column: 0,
                        source_line: None,
                        function_name: Some("IrBuilder::build_with_imports".to_string()),
                    },
                });
            }
        }

        Ok(builder.module)
    }

    /// 解析 import 语句并递归编译依赖模块（spec 3.3 节）
    ///
    /// # 算法
    /// 1. 遍历顶层 statements 中的 `Stmt::Import`
    /// 2. 调用 `resolver.resolve()` 解析路径（已规范化）
    /// 3. 调用 `resolver.check_circular()` 检测循环依赖
    /// 4. 缓存未命中 → 加载源码 → 解析 → 递归调用 `build_with_imports`
    /// 5. 收集 [`ImportRecord`]（含被导入模块的所有 fn 名）
    ///
    /// # 深度限制
    /// 100 层（超过返回 [`ResolveError::DepthExceeded`]）
    ///
    /// # 缓存语义
    /// cache key 是 `resolver.resolve()` 返回的规范化绝对路径，
    /// 同一路径不会重复编译。
    fn resolve_imports(
        program: &ast::Program,
        resolver: &dyn ModuleResolver,
        current_path: Option<&Path>,
        cache: &mut HashMap<PathBuf, Arc<IrModule>>,
        stack: &mut Vec<PathBuf>,
    ) -> Result<Vec<ImportRecord>, IrBuildError> {
        const MAX_DEPTH: usize = 100;
        let mut records = Vec::new();

        for stmt in &program.statements {
            if let ast::Stmt::Import { path, lazy, alias, span, .. } = stmt {
                let location = span_to_location(span);

                // 深度检查
                if stack.len() >= MAX_DEPTH {
                    return Err(Self::resolve_error_to_ir(ResolveError::DepthExceeded {
                        depth: stack.len(),
                        max_depth: MAX_DEPTH,
                        location,
                    }));
                }

                // 解析路径（resolver 内部已规范化）
                let resolved =
                    resolver.resolve(current_path, path).map_err(Self::resolve_error_to_ir)?;

                // 循环依赖检测（DFS 灰白标记）
                resolver.check_circular(&resolved, stack).map_err(Self::resolve_error_to_ir)?;

                // 缓存未命中 → 递归编译
                if !cache.contains_key(&resolved) {
                    stack.push(resolved.clone());

                    let source =
                        resolver.load_source(&resolved).map_err(Self::resolve_error_to_ir)?;

                    let sub_program = Parser::parse(&source).map_err(|e| IrBuildError::Error {
                        message: format!(
                            "Parse error in imported module {}: {:?}",
                            resolved.display(),
                            e
                        ),
                        location: location.clone(),
                    })?;

                    // 递归编译（共享 cache 与 stack）
                    let sub_module = Self::build_with_imports(
                        &sub_program,
                        &[],
                        resolver,
                        Some(&resolved),
                        cache,
                        stack,
                    )?;

                    stack.pop();
                    cache.insert(resolved.clone(), Arc::new(sub_module));
                }

                // 收集 ImportRecord（含被导入模块的所有 fn 名）
                let module_arc =
                    cache.get(&resolved).expect("just inserted or cached above").clone();
                records.push(ImportRecord {
                    path: resolved,
                    lazy: *lazy,
                    // Skip sub-module main entry (id=0): only named functions
                    // are exported as callable globals.
                    resolved_symbols: module_arc
                        .functions
                        .iter()
                        .filter(|f| f.id.0 != 0)
                        .map(|f| f.name.as_ref().to_string())
                        .collect(),
                    alias: alias.clone(),
                    functions: module_arc.functions.clone(),
                });
            }
        }

        Ok(records)
    }

    /// 合并导入子模块的 IrFunction 定义到主模块
    ///
    /// 对每个 import 记录中的函数（跳过子模块 main 入口 id=0）：
    /// 1. 重新分配 IrFunctionId（避免与主模块现有函数 ID 冲突）
    /// 2. 重映射函数体内 `IrOp::Closure { ir_func }` 引用到新 ID
    /// 3. 在主函数中发射 `Closure` + `SetGlobal`，使导入函数成为可调用的全局函数
    ///
    /// # ID 重映射
    /// 子模块的函数 ID 从 0 开始，直接合并会导致 ID 冲突。
    /// `id_map` 记录 old_id → new_id 映射，用于重编号和 Closure 引用修复。
    ///
    /// # 已知限制
    /// 菱形导入（A 导入 B 和 C，B 和 C 都导入 D）时，D 的函数可能被合并两次。
    /// `pre_scan_global_fns` 的重名检测会捕获同名的直接冲突，但不同别名
    /// 引用同一底层函数的情况不会报错。子模块 main 中的副作用代码（如 print）
    /// 不会被导入，因为只合并具名函数定义。
    fn merge_imported_functions(
        &mut self,
        import_records: &[ImportRecord],
    ) -> Result<(), IrBuildError> {
        for record in import_records {
            let base = self.module.functions.len() as u32;
            let mut id_map: HashMap<u32, u32> = HashMap::new();

            // Phase 1: build old_id → new_id mapping (skip sub-module main)
            for func in &record.functions {
                if func.id.0 == 0 {
                    continue;
                }
                let new_id = base + id_map.len() as u32;
                id_map.insert(func.id.0, new_id);
            }

            // Phase 2: merge with renumbered IDs and remapped Closure refs
            for func in &record.functions {
                if func.id.0 == 0 {
                    continue;
                }
                let mut cloned = func.clone();
                let new_id = id_map[&func.id.0];
                cloned.id = IrFunctionId(new_id);

                // Remap internal Closure ir_func references to new IDs
                for block in &mut cloned.blocks {
                    for op in &mut block.instructions {
                        if let IrOp::Closure { ir_func, .. } = op
                            && let Some(&mapped) = id_map.get(&ir_func.0)
                        {
                            *ir_func = IrFunctionId(mapped);
                        }
                        // ir_func.0 == 0 (ref to sub-module main) is left
                        // unmapped — rare edge case where an imported
                        // function references its parent module's main.
                    }
                }

                self.module.functions.push(cloned);

                // Emit Closure + SetGlobal in main function so codegen
                // registers the imported function as a global callable.
                let dest = self.alloc_value_ref();
                self.emit(IrOp::Closure { dest, ir_func: IrFunctionId(new_id) })?;
                self.emit(IrOp::SetGlobal { name: func.name.clone(), value: dest })?;
            }
        }

        Ok(())
    }

    /// 将 [`ResolveError`] 转换为 [`IrBuildError`]
    ///
    /// 保留位置信息便于错误报告。使用 [`IrBuildError::Error`] 变体包装，
    /// 避免新增错误变体（保持错误代码体系稳定，不破坏 IRB001-IRB010 映射）。
    fn resolve_error_to_ir(e: ResolveError) -> IrBuildError {
        let (message, location) = match &e {
            ResolveError::ModuleNotFound { path, location } => {
                (format!("Module not found: {}", path), location.clone())
            }
            ResolveError::CircularImport { chain, location } => {
                let chain_str =
                    chain.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" -> ");
                (format!("Circular import detected: {}", chain_str), location.clone())
            }
            ResolveError::DuplicateSymbol { name, first_location, second_location } => (
                format!(
                    "Duplicate symbol: '{}' (first defined at {}, redefined at {})",
                    name, first_location, second_location
                ),
                second_location.clone(),
            ),
            ResolveError::IoError { path, message, location } => {
                (format!("IO error loading module {}: {}", path, message), location.clone())
            }
            ResolveError::DepthExceeded { depth, max_depth, location } => {
                (format!("Import depth exceeded: {}/{}", depth, max_depth), location.clone())
            }
        };
        IrBuildError::Error { message, location }
    }

    // ========================================================================
    // ValueRef 分配
    // ========================================================================

    /// 分配一个新的虚拟寄存器引用
    ///
    /// 计数器单调递增，确保每个 ValueRef 唯一。
    fn alloc_value_ref(&mut self) -> ValueRef {
        let vr = ValueRef(self.next_value_ref);
        self.next_value_ref += 1;
        vr
    }

    // ========================================================================
    // 指令发射辅助方法
    // ========================================================================

    /// 向当前基本块发射一条 IR 指令
    ///
    /// 发射目标由 `current_block_id` 决定，而非"最后一个块"。
    /// 通过 `switch_to_block()` 切换发射目标，实现精确的跨块发射。
    ///
    /// # Errors (内部不变量违反)
    /// - `current_function_id` 越界: 说明作用域管理有 bug (A9 修复: 始终检查,
    ///   原仅 debug 模式检查,release 模式下越界会直接索引 panic 无消息)
    /// - `current_block_id` 越界: 说明块管理有 bug (H3 修复: 加边界检查,
    ///   原直接索引越界 panic 无消息)
    ///
    /// 这两处错误是"内部不变量违反",类似 Vec 索引越界,
    /// 不是"可能失败路径",外部输入不会触发。返回 `IrBuildError::InternalError`。
    fn emit(&mut self, op: IrOp) -> Result<(), IrBuildError> {
        // A9 修复: 移除 #[cfg(debug_assertions)],始终检查 current_function_id 边界。
        // 原本仅在 debug 模式检查,release 模式下越界会直接索引 panic(无消息)。
        let func_len = self.module.functions.len();
        if self.current_function_id.0 as usize >= func_len {
            return Err(IrBuildError::InternalError {
                what: "current_function_id out of range".to_string(),
                context: format!(
                    "fn_id={}, functions.len()={}; indicates scope management bug in build_closure_expr or build_fn_expr",
                    self.current_function_id.0, func_len
                ),
                location: SourceLocation::default(),
                hint: "Check build_closure_expr/build_fn_expr scope save/restore logic".to_string(),
            });
        }

        let block_id = self.current_block_id;
        let func = self.module.get_function_mut(self.current_function_id);
        // H3 修复: 检查 block_id 边界,防止直接索引越界 panic(无消息)。
        // block_id 由 new_block 创建后切换,理论上不会越界;
        // 若越界则说明 block 管理有 bug,返回带清晰诊断信息的错误。
        let blocks_len = func.blocks.len();
        if block_id.0 as usize >= blocks_len {
            return Err(IrBuildError::InternalError {
                what: "current_block_id out of range".to_string(),
                context: format!(
                    "block_id={}, blocks.len()={}; indicates block management bug in switch_to_block or new_block",
                    block_id.0, blocks_len
                ),
                location: SourceLocation::default(),
                hint: "Check switch_to_block/new_block block ID management".to_string(),
            });
        }
        func.blocks[block_id.0 as usize].push(op);
        Ok(())
    }

    /// 向指定基本块发射 IR 指令（精确块定位）
    ///
    /// 此方法通过 BasicBlockId 索引直接定位目标块，
    /// 用于控制流构建中需要向非当前块发射指令的场景。
    ///
    /// # 注意
    /// 此方法不修改 `current_block_id`，调用者需自行管理发射状态。
    /// 当前控制流构建已统一使用 `switch_to_block` + `emit`，此方法保留以备特殊场景。
    ///
    /// # Errors (H3 内部不变量违反)
    /// `block_id` 或 `current_function_id` 越界时返回 `IrBuildError::InternalError` 带清晰诊断信息。
    #[allow(dead_code)] // IR 构建预留 API，保留供向非当前块发射指令的特殊场景使用
    fn emit_to_block(&mut self, block_id: BasicBlockId, op: IrOp) -> Result<(), IrBuildError> {
        // H3 修复: 加边界检查,防止直接索引越界 panic(无消息)。
        let func_len = self.module.functions.len();
        if self.current_function_id.0 as usize >= func_len {
            return Err(IrBuildError::InternalError {
                what: "current_function_id out of range".to_string(),
                context: format!(
                    "fn_id={}, functions.len()={}",
                    self.current_function_id.0, func_len
                ),
                location: SourceLocation::default(),
                hint: "Check build_closure_expr/build_fn_expr scope save/restore logic".to_string(),
            });
        }
        let func = self.module.get_function_mut(self.current_function_id);
        let blocks_len = func.blocks.len();
        if block_id.0 as usize >= blocks_len {
            return Err(IrBuildError::InternalError {
                what: "block_id out of range".to_string(),
                context: format!("block_id={}, blocks.len()={}", block_id.0, blocks_len),
                location: SourceLocation::default(),
                hint: "Check block ID management in new_block/switch_to_block".to_string(),
            });
        }
        func.blocks[block_id.0 as usize].push(op);
        Ok(())
    }

    /// 发射常量加载指令，返回目标 ValueRef
    fn emit_load_constant(&mut self, constant: IrConstant) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::LoadConstant { dest, constant })?;
        Ok(dest)
    }

    /// 发射二元运算指令，返回结果 ValueRef
    fn emit_binary(
        &mut self,
        op: IrBinOp,
        left: ValueRef,
        right: ValueRef,
    ) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::Binary { dest, op, left, right })?;
        Ok(dest)
    }

    /// 发射一元运算指令，返回结果 ValueRef
    fn emit_unary(&mut self, op: IrUnaryOp, operand: ValueRef) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::Unary { dest, op, operand })?;
        Ok(dest)
    }

    /// 发射无条件跳转指令
    fn emit_jump(&mut self, target: BasicBlockId) -> Result<(), IrBuildError> {
        self.emit(IrOp::Jump { target })?;
        Ok(())
    }

    /// 发射条件跳转指令
    fn emit_jump_if(
        &mut self,
        cond: ValueRef,
        then_target: BasicBlockId,
        else_target: BasicBlockId,
    ) -> Result<(), IrBuildError> {
        self.emit(IrOp::JumpIf { cond, then_target, else_target })?;
        Ok(())
    }

    /// 发射返回指令
    fn emit_return(&mut self, value: Option<ValueRef>) -> Result<(), IrBuildError> {
        self.emit(IrOp::Return { value })?;
        Ok(())
    }

    /// 发射 GetLocal 指令，返回目标 ValueRef
    #[allow(dead_code)] // IR 构建预留 API，保留供局部变量读取场景使用
    fn emit_get_local(&mut self, name: &str) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::GetLocal { dest, name: name.into() })?;
        Ok(dest)
    }

    /// 发射 SetLocal 指令
    #[allow(dead_code)] // IR 构建预留 API，保留供局部变量写入场景使用
    fn emit_set_local(&mut self, name: &str, value: ValueRef) -> Result<(), IrBuildError> {
        self.emit(IrOp::SetLocal { name: name.into(), value })?;
        Ok(())
    }

    /// 发射 IndexGet 指令，返回目标 ValueRef
    #[allow(dead_code)] // IR 构建预留 API，保留供索引读取场景使用
    fn emit_index_get(
        &mut self,
        object: ValueRef,
        index: ValueRef,
    ) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::IndexGet { dest, object, index })?;
        Ok(dest)
    }

    /// 发射 Len 指令，返回目标 ValueRef
    #[allow(dead_code)] // IR 构建预留 API，保留供集合长度查询场景使用
    fn emit_len(&mut self, object: ValueRef) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::Len { dest, object })?;
        Ok(dest)
    }

    /// 发射 RangeNew 指令，返回目标 ValueRef
    ///
    /// 创建范围对象 dest = start..end (inclusive 控制闭/半开区间)
    fn emit_range_new(
        &mut self,
        start: ValueRef,
        end: ValueRef,
        inclusive: bool,
    ) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::RangeNew { dest, start, end, inclusive })?;
        Ok(dest)
    }

    // ========================================================================
    // 基本块管理
    // ========================================================================

    /// 创建新基本块并返回其 ID
    ///
    /// 新块自动追加到当前函数的块列表中。
    fn new_block(&mut self) -> BasicBlockId {
        let func = self.module.get_function_mut(self.current_function_id);
        let id = BasicBlockId(func.blocks.len() as u32);
        func.blocks.push(BasicBlock::new(id));
        id
    }

    /// 切换当前基本块（更新发射目标）
    ///
    /// `emit()` 方法通过 `current_block_id` 索引精确发射指令到指定块，
    /// 因此 `switch_to_block` 能正确控制后续指令的发射目标。
    /// 调用后，所有 `emit*` 方法将指令追加到新切换的块中。
    fn switch_to_block(&mut self, id: BasicBlockId) {
        self.current_block_id = id;
    }

    /// 检查当前基本块是否已以终止指令结尾（Jump/Return/JumpIf）
    ///
    /// 用于 build_if_expr 等控制流构建方法：当分支已因 break/continue/return 而终止时，
    /// 不应再发射 Mov(result) + Jump(merge) 等后续指令（否则产生死代码）。
    ///
    /// # Errors (H3 内部不变量违反)
    /// `current_function_id` 或 `current_block_id` 越界时返回 `IrBuildError::InternalError` 带清晰诊断信息。
    fn block_is_terminated(&self) -> Result<bool, IrBuildError> {
        // H3 修复: 加边界检查,防止直接索引越界 panic(无消息)。
        let func_len = self.module.functions.len();
        if self.current_function_id.0 as usize >= func_len {
            return Err(IrBuildError::InternalError {
                what: "current_function_id out of range".to_string(),
                context: format!(
                    "fn_id={}, functions.len()={}",
                    self.current_function_id.0, func_len
                ),
                location: SourceLocation::default(),
                hint: "Check build_closure_expr/build_fn_expr scope save/restore logic".to_string(),
            });
        }
        let func = &self.module.functions[self.current_function_id.0 as usize];
        let blocks_len = func.blocks.len();
        if self.current_block_id.0 as usize >= blocks_len {
            return Err(IrBuildError::InternalError {
                what: "current_block_id out of range".to_string(),
                context: format!(
                    "block_id={}, blocks.len()={}",
                    self.current_block_id.0, blocks_len
                ),
                location: SourceLocation::default(),
                hint: "Check switch_to_block/new_block block ID management".to_string(),
            });
        }
        let block = &func.blocks[self.current_block_id.0 as usize];
        Ok(block.instructions.last().is_some_and(|op| op.is_terminator()))
    }

    // ========================================================================
    // 局部变量管理
    // ========================================================================

    /// 在当前作用域定义一个局部变量
    fn define_local(&mut self, name: impl Into<Arc<str>>, value: ValueRef) {
        self.locals.push((name.into(), value));
    }

    /// 更新局部变量的 ValueRef 绑定（用于 SetLocal 后续引用能找到新值）
    fn update_local_binding(&mut self, name: &str, new_value: ValueRef) {
        for (local_name, vr) in self.locals.iter_mut().rev() {
            if local_name.as_ref() == name {
                *vr = new_value;
                return;
            }
        }
    }

    /// 查找局部变量（从内向外搜索）
    fn resolve_local(&self, name: &str) -> Option<ValueRef> {
        // 从后向前搜索（最近作用域优先）
        for (local_name, vr) in self.locals.iter().rev() {
            if local_name.as_ref() == name {
                return Some(*vr);
            }
        }
        None
    }

    /// 在 capture_stack 中查找外层闭包捕获的变量索引
    ///
    /// 从最近的捕获层开始搜索（栈顶），找到变量名匹配的捕获项，
    /// 返回其在捕获列表中的索引（用于 OuterCapture source）。
    fn find_outer_capture_index(&self, name: &str) -> Option<u16> {
        for caps in self.capture_stack.iter().rev() {
            for (i, cap) in caps.iter().enumerate() {
                if cap.name.as_ref() == name {
                    return Some(i as u16);
                }
            }
        }
        None
    }

    /// 在当前闭包的捕获列表中查找变量的捕获索引
    ///
    /// 查找 capture_stack 栈顶（当前闭包）的捕获列表，
    /// 如果变量名匹配则返回其索引（用于 GetCapture 指令）。
    fn find_capture_index(&self, name: &str) -> Option<u16> {
        self.capture_stack.last().and_then(|caps| {
            caps.iter()
                .enumerate()
                .find(|(_, cap)| cap.name.as_ref() == name)
                .map(|(i, _)| i as u16)
        })
    }

    /// 进入新作用域（记录 locals 栈深度，用于退出时回退）
    #[allow(dead_code)] // IR 作用域管理 API，保留供后续 IR 作用域嵌套使用
    fn scope_depth(&self) -> usize {
        self.locals.len()
    }

    /// 退出作用域（弹出 scope_depth 之后的所有局部变量）
    #[allow(dead_code)] // 保留为公开 API，供后续 IR 作用域管理使用
    fn pop_scope(&mut self, depth: usize) {
        self.locals.truncate(depth);
    }

    // ========================================================================
    // 表达式构建（核心方法）— AST Expr → ValueRef
    // ========================================================================

    /// 构建表达式，返回结果 ValueRef
    ///
    /// 这是所有表达式编译的统一入口点，根据 AST 表达式类型分发到具体的构建方法。
    /// 采用递归下降策略，先构建子节点再组合结果。
    pub fn build_expr(&mut self, expr: &Expr) -> Result<ValueRef, IrBuildError> {
        match expr {
            // ── 字面量（Literal Expressions）──
            Expr::Number { value, .. } => Ok(self.emit_load_constant(IrConstant::Number(*value))?),

            Expr::String { value, .. } => {
                Ok(self.emit_load_constant(IrConstant::String(value.as_str().into()))?)
            }

            Expr::Bool { value, .. } => Ok(self.emit_load_constant(IrConstant::Bool(*value))?),

            Expr::Nil { .. } => Ok(self.emit_load_constant(IrConstant::Nil)?),

            // ── 变量访问 ──
            Expr::Ident { name, span } => self.build_ident(name, span),

            // ── 运算符表达式 ──
            Expr::Binary { left, op, right, .. } => self.build_binary(left, op, right),

            Expr::Unary { op, operand, .. } => self.build_unary(op, operand),

            // ── 逻辑运算（短路求值）──
            Expr::And { left, right, .. } => self.build_and(left, right),

            Expr::Or { left, right, .. } => self.build_or(left, right),

            // ── 函数调用与成员访问 ──
            Expr::Call { callee, args, .. } => self.build_call(callee, args),

            Expr::Index { object, index, .. } => self.build_index(object, index),

            Expr::Field { object, name, .. } => self.build_field(object, name),

            // ── 控制流表达式 ──
            Expr::If { .. } => self.build_if_expr(expr),

            // 循环/跳转表达式：在表达式位置出现时，委托给语句级构建器处理
            Expr::While { condition, body, .. } => {
                self.build_while_stmt(condition, body)?;
                // 🔧 Fix: while/loop 是语句型表达式，不产生有意义的返回值。
                // 必须返回一个已定义的 ValueRef（而非 alloc_value_ref() 幽灵值），
                // 否则 codegen Phase 1 的 get_reg() 会因 reg_map 中无此条目而报 Undefined ValueRef，
                // 导致子函数注册失败 → Closure 指令找不到 prototype → 崩溃。
                Ok(self.emit_load_constant(IrConstant::Nil)?)
            }
            Expr::Loop { body, .. } => {
                // 为 loop 分配结果寄存器（支持 break 带值）
                let result = self.alloc_value_ref();
                self.build_loop_stmt(body, Some(result))?;
                Ok(result)
            }
            Expr::Break { value, span } => {
                self.build_break_stmt(value.as_deref(), span)?;
                // P2.3 文档说明（幽灵 ValueRef 模式）：
                // break 后控制流不可达，但 build_expr 签名要求返回 ValueRef。
                // 此处返回一个"幽灵" ValueRef（从未被任何 IrOp 定义或消费），
                // 而非 emit_load_constant(Nil) —— 因为 nil 会被注入到指令流，
                // 污染寄存器状态导致后续 println(i) 等读到错误值。
                //
                // 不变量：调用方不应消费 break/continue/return 表达式的返回值，
                // 因为它们出现在表达式位置仅是为了满足语法（如 `let x = if cond { break } else { 1 }`）。
                // 任何对幽灵 ValueRef 的消费都将在 codegen Phase 1 get_reg() 时
                // 报 "Undefined ValueRef" 错误，提供清晰的失败信号。
                //
                // TODO(future): 引入 `Option<ValueRef>` 返回类型或 `IrOp::Unreachable` 占位节点，
                // 在类型系统层面消除幽灵值依赖。当前保留是为了不破坏 build_expr 签名。
                Ok(self.alloc_value_ref())
            }
            Expr::Continue { span } => {
                self.build_continue_stmt(span)?;
                // 同 Break：幽灵 ValueRef 模式（详见 Break 分支注释）
                Ok(self.alloc_value_ref())
            }
            Expr::Return { value, span } => {
                self.build_return_stmt(value.as_deref(), span)?;
                // 同 Break：幽灵 ValueRef 模式（详见 Break 分支注释）
                Ok(self.alloc_value_ref())
            }

            // ── for-in 循环 ──
            Expr::ForIn { var_name, iterable, body, .. } => {
                self.build_for_in_stmt(var_name, iterable, body)?;
                // 同 Break：for-in 是语句，返回幽灵 ValueRef 占位（详见 Break 分支注释）
                Ok(self.alloc_value_ref())
            }

            // ── 函数 / 闭包表达式 ──
            Expr::Fn { name, params, body, .. } => {
                self.build_fn_expr(name.as_deref(), params, body)
            }
            Expr::Closure { params, body, .. } => self.build_closure_expr(params, body, None),

            // ── 块表达式 ──
            Expr::Block { statements, .. } => self.build_block_expr(statements),

            // ── 复合数据类型字面量 ──
            Expr::Array { elements, .. } => self.build_array_literal(elements),

            Expr::Dict { pairs, .. } => self.build_dict_literal(pairs),

            Expr::Tuple { elements, .. } => {
                // 元组编译为数组（与 compiler/functions.rs 的 compile_tuple 一致）
                self.build_array_literal(elements)
            }

            Expr::Range { start, end, inclusive, .. } => {
                let start_val = self.build_expr(start)?;
                let end_val = self.build_expr(end)?;
                Ok(self.emit_range_new(start_val, end_val, *inclusive)?)
            }

            // ── 异常处理表达式（T6 补充）──
            Expr::Try { .. } | Expr::Out { .. } => Err(IrBuildError::Error {
                message: "Exception handling not yet implemented in IR builder".to_string(),
                location: span_to_location(expr.span()),
            }),

            // ── 高级抽象：空值合并 ──
            Expr::NullCoalesce { left, right, .. } => {
                // Desugar to if-else: if (left == nil) { right } else { left }
                // But left is evaluated only once via a temporary
                let left_val = self.build_expr(left)?;
                let nil_val = self.emit_load_constant(IrConstant::Nil)?;
                let is_nil = self.emit_binary(IrBinOp::Eq, left_val, nil_val)?;

                let result = self.alloc_value_ref();
                let then_block = self.new_block(); // left is nil → evaluate right
                let else_block = self.new_block(); // left is not nil → use left
                let merge_block = self.new_block();

                self.emit_jump_if(is_nil, then_block, else_block)?;

                // Then block (left IS nil): result = right
                self.switch_to_block(then_block);
                let right_val = self.build_expr(right)?;
                self.emit(IrOp::Mov { dest: result, src: right_val })?;
                self.emit_jump(merge_block)?;

                // Else block (left is NOT nil): result = left
                self.switch_to_block(else_block);
                self.emit(IrOp::Mov { dest: result, src: left_val })?;
                self.emit_jump(merge_block)?;

                // Merge block
                self.switch_to_block(merge_block);
                Ok(result)
            }

            // ── 高级抽象：模式匹配 ──
            Expr::Match { scrutinee, arms, .. } => {
                let scrut_val = self.build_expr(scrutinee)?;
                let result = self.alloc_value_ref();

                // Phase 1: Pre-create all blocks in processing order
                // Order: check_block_0, body_block_0, check_block_1, body_block_1, ..., merge_block
                // This ensures codegen processes Mov(result, ...) definitions before the merge block uses result.
                let mut check_blocks: Vec<BasicBlockId> = Vec::new();
                let mut body_blocks: Vec<BasicBlockId> = Vec::new();
                for _ in arms.iter() {
                    check_blocks.push(self.new_block());
                    body_blocks.push(self.new_block());
                }
                let merge_block = self.new_block();

                // Entry → first check block
                self.emit_jump(check_blocks[0])?;

                // Phase 2: Emit pattern matching logic in each check block
                for (i, arm) in arms.iter().enumerate() {
                    self.switch_to_block(check_blocks[i]);
                    let next = if i + 1 < arms.len() { check_blocks[i + 1] } else { merge_block };

                    match &arm.pattern {
                        MatchPattern::Literal(lit_expr) => {
                            let lit_val = self.build_expr(lit_expr)?;
                            let eq_val = self.emit_binary(IrBinOp::Eq, scrut_val, lit_val)?;
                            self.emit_jump_if(eq_val, body_blocks[i], next)?;
                        }
                        MatchPattern::Range { start, end, inclusive } => {
                            let start_val = self.build_expr(start)?;
                            let end_val = self.build_expr(end)?;
                            let ge_val = self.emit_binary(IrBinOp::Ge, scrut_val, start_val)?;
                            // Use nested block for second check (no And in IrBinOp)
                            let check_le_block = self.new_block();
                            self.emit_jump_if(ge_val, check_le_block, next)?;
                            self.switch_to_block(check_le_block);
                            let cmp_op = if *inclusive { IrBinOp::Le } else { IrBinOp::Lt };
                            let le_val = self.emit_binary(cmp_op, scrut_val, end_val)?;
                            self.emit_jump_if(le_val, body_blocks[i], next)?;
                        }
                        MatchPattern::Variable(var_name) => {
                            self.define_local(var_name.clone(), scrut_val);
                            self.emit_jump(body_blocks[i])?;
                        }
                        MatchPattern::Wildcard => {
                            self.emit_jump(body_blocks[i])?;
                        }
                    }
                }

                // Phase 3: Emit arm bodies
                for (i, arm) in arms.iter().enumerate() {
                    self.switch_to_block(body_blocks[i]);
                    let body_val = self.build_expr(&arm.body)?;
                    self.emit(IrOp::Mov { dest: result, src: body_val })?;
                    self.emit_jump(merge_block)?;
                }

                self.switch_to_block(merge_block);
                Ok(result)
            }
        }
    }

    // ── 标识符解析（三级查找策略）──

    /// 构建标识符引用
    ///
    /// 实现三级变量解析策略（参照 nuzo_compiler::Compiler::compile_ident）：
    /// 1. 局部变量（Local）→ 直接返回已绑定的 ValueRef
    /// 2. 闭包捕获变量（Captured）→ GetCapture 指令
    /// 3. 全局变量（Global）→ GetGlobal 指令
    fn build_ident(&mut self, name: &str, _span: &Span) -> Result<ValueRef, IrBuildError> {
        // 1. 查找局部变量
        if let Some(vr) = self.resolve_local(name) {
            return Ok(vr);
        }
        // 2. 查找闭包捕获变量
        if let Some(idx) = self.find_capture_index(name) {
            let dest = self.alloc_value_ref();
            self.emit(IrOp::GetCapture { dest, index: idx })?;
            return Ok(dest);
        }
        // 3. 作为全局变量访问
        let dest = self.alloc_value_ref();
        self.emit(IrOp::GetGlobal { dest, name: name.into() })?;
        Ok(dest)
    }

    // ── 二元运算 ──

    /// 构建二元运算表达式
    ///
    /// 使用声明式映射将 AST BinaryOp 转换为 IrBinOp，
    /// 然后发射三地址码指令。
    ///
    /// 映射关系（与 nuzo_compiler::expressions.rs 的 binary_op_to_opcode 一致）：
    /// - 算术(7): Add, Sub, Mul, Div, Rem, Mod, Pow
    /// - 等值比较(2): Eq, Neq
    /// - 序比较(4): Lt, Gt, LtEq→Le, GtEq→Ge
    ///
    /// # 字符串拼接优化
    ///
    /// 当运算符为 `Add` 且操作数链长度 ≥ 3 时，将连续 `+` 链展平为
    /// `IrOp::StringBuild` 指令，使 VM 能一次性预计算总长度、单次分配、
    /// 逐段拷入，将 O(N²) 拷贝/分配降为 O(N)。
    ///
    /// 仅当操作数链长度 ≥ 3 时启用（2 个操作数时等价于普通 Add，无需优化）。
    ///
    /// ## 拼接树分析算法
    ///
    /// 递归遍历 AST 中左结合的 `+` 链，收集所有叶子操作数：
    ///
    /// ```text
    /// a + b + c + d
    ///   解析为 ((a + b) + c) + d
    ///   展平为 [a, b, c, d]
    /// ```
    ///
    /// 对于非 `+` 的子表达式（如 `a * b + c`），`a * b` 作为整体操作数保留。
    fn build_binary(
        &mut self,
        left: &Expr,
        op: &ast::BinaryOp,
        right: &Expr,
    ) -> Result<ValueRef, IrBuildError> {
        // ── 字符串拼接树检测 ──
        // 仅对 Add 运算符尝试拼接树展平。
        // 展平后如果操作数 ≥ 3 个，且至少一个操作数在编译期可判定为字符串类型，
        // 则使用 StringBuild 指令。
        //
        // 类型判定启发式：
        //   - 字符串字面量 `Expr::String` → 确定是字符串
        //   - 其他表达式 → 无法确定（可能是数字、变量等）
        // 只有当至少一个操作数是字符串字面量时才触发优化，
        // 避免将纯数字加法 `1 + 2 + 3 + 4` 误编译为字符串拼接。
        if matches!(op, ast::BinaryOp::Add) {
            let mut operands = Vec::new();
            self.flatten_add_chain(left, &mut operands);
            self.flatten_add_chain(right, &mut operands);

            if operands.len() >= 3 && operands.iter().any(Self::is_likely_string) {
                // 为每个操作数生成 IR，收集 ValueRef
                let mut value_refs = Vec::with_capacity(operands.len());
                for operand_expr in &operands {
                    let vr = self.build_expr(operand_expr)?;
                    value_refs.push(vr);
                }

                let dest = self.alloc_value_ref();
                self.emit(IrOp::StringBuild { dest, operands: value_refs })?;
                return Ok(dest);
            }
        }

        // ── 默认路径：普通二元运算 ──
        let left_val = self.build_expr(left)?;
        let right_val = self.build_expr(right)?;

        // 使用 From<BinaryOp> for IrBinOp 的单一映射源（types.rs）
        let ir_op = IrBinOp::from(*op);

        self.emit_binary(ir_op, left_val, right_val)
    }

    /// 递归展平连续 `+` 链，收集叶子表达式。
    ///
    /// 遇到 `Expr::Binary { op: Add, .. }` 时递归展平其左右子树；
    /// 遇到其他表达式时将其作为叶子节点收集。
    ///
    /// 这使得 `a + b + c + d` 展平为 `[a, b, c, d]`，
    /// 而 `a * b + c` 展平为 `[a * b, c]`（`a * b` 作为整体保留）。
    fn flatten_add_chain<'e>(&self, expr: &'e Expr, out: &mut Vec<&'e Expr>) {
        match expr {
            Expr::Binary { left, op: ast::BinaryOp::Add, right, .. } => {
                self.flatten_add_chain(left, out);
                self.flatten_add_chain(right, out);
            }
            _ => {
                out.push(expr);
            }
        }
    }

    /// 判断表达式是否在编译期可判定为字符串类型。
    ///
    /// 用于拼接树分析的启发式：只有当 `+` 链中至少一个操作数
    /// 确定是字符串时，才使用 `StringBuild` 指令。
    ///
    /// 判定规则：
    /// - `Expr::String { .. }` — 字符串字面量，确定是字符串
    /// - 其他 — 无法确定，保守返回 false
    fn is_likely_string(expr: &&Expr) -> bool {
        matches!(expr, Expr::String { .. })
    }

    // ── 一元运算 ──

    /// 构建一元运算表达式
    ///
    /// 映射关系：
    /// - Negate → IrUnaryOp::Neg（算术取负）
    /// - Not → IrUnaryOp::Not（逻辑非）
    fn build_unary(&mut self, op: &ast::UnaryOp, operand: &Expr) -> Result<ValueRef, IrBuildError> {
        let operand_val = self.build_expr(operand)?;
        let ir_op = IrUnaryOp::from(*op);
        self.emit_unary(ir_op, operand_val)
    }

    // ── 逻辑运算（短路求值）──

    /// 构建逻辑 AND 表达式（短路求值）
    ///
    /// 语义：如果 left 为假，直接返回 left；否则返回 right。
    ///
    /// 控制流结构：
    /// ```text
    ///   entry: result = mov left; JumpIf(left, eval_right, merge)
    ///   eval_right: result = mov right; Jump(merge)
    ///   merge: result 已由执行的分支设置
    /// ```
    ///
    /// 使用 Mov（degenerate Phi）在两个分支中统一写入 result ValueRef，
    /// 避免引入真正的 Phi 节点和 predecessor 追踪。
    fn build_and(&mut self, left: &Expr, right: &Expr) -> Result<ValueRef, IrBuildError> {
        let left_val = self.build_expr(left)?;
        let eval_right_block = self.new_block();
        let merge_block = self.new_block();
        let result = self.alloc_value_ref();

        // 默认结果 = left（left 为假时短路，直接使用 left 值）
        self.emit(IrOp::Mov { dest: result, src: left_val })?;
        // left 为真 → 求值 right；left 为假 → 跳到 merge（保持 result = left）
        self.emit_jump_if(left_val, eval_right_block, merge_block)?;

        // === eval_right 块：求值 right 并更新 result ===
        self.switch_to_block(eval_right_block);
        let right_val = self.build_expr(right)?;
        self.emit(IrOp::Mov { dest: result, src: right_val })?;
        self.emit_jump(merge_block)?;

        // === merge 块 ===
        self.switch_to_block(merge_block);
        Ok(result)
    }

    /// 构建逻辑 OR 表达式（短路求值）
    ///
    /// 语义：如果 left 为真，直接返回 left；否则返回 right。
    ///
    /// 控制流结构：
    /// ```text
    ///   entry: result = mov left; JumpIf(left, merge, eval_right)
    ///   eval_right: result = mov right; Jump(merge)
    ///   merge: result 已由执行的分支设置
    /// ```
    fn build_or(&mut self, left: &Expr, right: &Expr) -> Result<ValueRef, IrBuildError> {
        let left_val = self.build_expr(left)?;
        let eval_right_block = self.new_block();
        let merge_block = self.new_block();
        let result = self.alloc_value_ref();

        // 默认结果 = left（left 为真时短路，直接使用 left 值）
        self.emit(IrOp::Mov { dest: result, src: left_val })?;
        // left 为真 → 跳到 merge（保持 result = left）；left 为假 → 求值 right
        self.emit_jump_if(left_val, merge_block, eval_right_block)?;

        // === eval_right 块：求值 right 并更新 result ===
        self.switch_to_block(eval_right_block);
        let right_val = self.build_expr(right)?;
        self.emit(IrOp::Mov { dest: result, src: right_val })?;
        self.emit_jump(merge_block)?;

        // === merge 块 ===
        self.switch_to_block(merge_block);
        Ok(result)
    }

    // ── 函数调用 ──

    /// 构建函数调用表达式
    ///
    /// 编译流程（参照 nuzo_compiler::expressions.rs 的 compile_call）：
    /// 1. 编译被调用者表达式 → callee ValueRef
    /// 2. 依次编译每个实参 → arg ValueRef 列表
    /// 3. 发射 Call 指令
    fn build_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<ValueRef, IrBuildError> {
        let callee_val = self.build_expr(callee)?;

        let mut arg_vals = Vec::with_capacity(args.len());
        for arg in args {
            arg_vals.push(self.build_expr(arg)?);
        }

        let dest = Some(self.alloc_value_ref());
        self.emit(IrOp::Call { dest, callee: callee_val, args: arg_vals })?;

        dest.ok_or_else(|| IrBuildError::Error {
            message: "Call destination allocation failed".to_string(),
            location: span_to_location(callee.span()),
        })
    }

    // ── 成员访问 ──

    /// 构建索引访问表达式（object[index]）
    fn build_index(&mut self, object: &Expr, index: &Expr) -> Result<ValueRef, IrBuildError> {
        let obj_val = self.build_expr(object)?;
        let idx_val = self.build_expr(index)?;
        let dest = self.alloc_value_ref();
        self.emit(IrOp::IndexGet { dest, object: obj_val, index: idx_val })?;
        Ok(dest)
    }

    /// 构建属性访问表达式（object.field）
    fn build_field(&mut self, object: &Expr, name: &str) -> Result<ValueRef, IrBuildError> {
        let obj_val = self.build_expr(object)?;
        let dest = self.alloc_value_ref();
        self.emit(IrOp::GetField { dest, object: obj_val, field: name.into() })?;
        Ok(dest)
    }

    // ── If 表达式 ──

    /// 构建 if 表达式（可作为表达式返回值）
    ///
    /// 控制流结构：
    /// ```text
    ///   entry: JumpIf(cond, then_block, else_block)
    ///   then_block: ... ; result = mov then_val; Jump(merge)
    ///   else_block: ... ; result = mov else_val; Jump(merge)
    ///   merge: result 已由执行的分支设置
    /// ```
    ///
    /// 使用 Mov（degenerate Phi）在 then/else 分支中统一写入 result ValueRef，
    /// 实现控制流汇合点的值合并。无 else 分支时，else_block 设置 result = nil。
    fn build_if_expr(&mut self, expr: &Expr) -> Result<ValueRef, IrBuildError> {
        if let Expr::If { condition, then_branch, else_branch, .. } = expr {
            let cond_val = self.build_expr(condition)?;

            // 🔧 Fix B: 记录 if 入口前的 locals 快照（用于检测跨 BB 变量重赋值）
            let pre_if_locals: Vec<(Arc<str>, ValueRef)> = self.locals.clone();

            // 创建基本块（始终创建 else_block 以统一控制流）
            let then_block = self.new_block();
            let else_block = self.new_block();
            let merge_block = self.new_block();

            // 发射条件跳转
            self.emit_jump_if(cond_val, then_block, else_block)?;

            // 结果 ValueRef（由 then/else 分支通过 Mov 统一写入）
            let result = self.alloc_value_ref();

            // === Then 分支 ===
            self.switch_to_block(then_block);
            let then_val = self.build_block_expr(then_branch)?;
            // 快照 then 分支处理后的 locals 状态
            let post_then_locals: Vec<(Arc<str>, ValueRef)> = self.locals.clone();
            // 🔧 记录 then 分支是否被 return/break/continue 提前终止
            // （区别于被后续 Jump(merge) 终止）
            let then_early_terminated = self.block_is_terminated()?;
            if !then_early_terminated {
                self.emit(IrOp::Mov { dest: result, src: then_val })?;
                self.emit_jump(merge_block)?;
            }

            // === Else 分支 ===
            // 🔧 恢复到 if 前的 locals 状态，让 else 分支从干净的初始状态开始构建。
            self.locals = pre_if_locals.clone();

            self.switch_to_block(else_block);
            if let Some(else_expr) = else_branch {
                let else_val = self.build_expr(else_expr)?;
                if !self.block_is_terminated()? {
                    self.emit(IrOp::Mov { dest: result, src: else_val })?;
                }
            } else {
                if !self.block_is_terminated()? {
                    let nil_vr = self.emit_load_constant(IrConstant::Nil)?;
                    self.emit(IrOp::Mov { dest: result, src: nil_vr })?;
                }
            }
            // 快照 else 分支处理后的 locals 状态
            let post_else_locals: Vec<(Arc<str>, ValueRef)> = self.locals.clone();
            // 🔧 记录 else 分支是否被 return/break/continue 提前终止
            // （在发射 Jump(merge) 之前记录，避免混淆）
            let else_early_terminated = self.block_is_terminated()?;
            if !else_early_terminated {
                self.emit_jump(merge_block)?;
            }

            // === Merge 块 ===
            self.switch_to_block(merge_block);

            // 🔧 Fix B: 对在分支中被重新赋值的变量，发射 Select 指令（poor man's phi）。
            self.locals = self.merge_locals_for_if_merge(
                &pre_if_locals,
                &post_then_locals,
                &post_else_locals,
                cond_val,
            )?;

            // 🔧 Fix: 只有当两个分支都被 return/break/continue 提前终止时，
            // merge 块才是真正的不可达死代码。给它 return nil 作为 terminator，
            // 防止 build_closure_expr 追加的隐式 return 引用未定义的 ValueRef。
            // 注意：不能用 block_is_terminated() 判断，因为正常的 Jump(merge)
            // 也会使块 terminated，但那不是"提前终止"。
            if then_early_terminated && else_early_terminated && !self.block_is_terminated()? {
                let nil_vr = self.emit_load_constant(IrConstant::Nil)?;
                self.emit(IrOp::Return { value: Some(nil_vr) })?;
            }

            Ok(result)
        } else {
            // A8 修复: 用 UnexpectedExpr 错误替代 unreachable! panic,
            // 使非 If 表达式传入时能优雅返回错误而非崩溃。
            // build_if_expr 应只被 If 表达式调用,传入其他变体表示调用方有 bug。
            Err(IrBuildError::UnexpectedExpr {
                expr_kind: format!("{:?}", expr),
                context: "build_if_expr",
                location: SourceLocation::default(),
            })
        }
    }

    /// 合并 if/else 分支后的 locals 状态，对变化的变量发射 Select 指令
    ///
    /// 对比三个快照（if 前、then 后、else 后），找出在每个分支中被重新赋值的变量。
    /// 对每个变化的变量在 merge block 中发射 `dest = cond ? then_val : else_val`。
    ///
    /// # 参数
    /// - `pre`: if 入口前的 locals 快照
    /// - `post_then`: then 分支处理后的 locals 快照
    /// - `post_else`: else 分支处理后的 locals 快照
    /// - `cond_val`: if 条件表达式的 ValueRef（用于 Select 的条件）
    fn merge_locals_for_if_merge(
        &mut self,
        pre: &[(Arc<str>, ValueRef)],
        post_then: &[(Arc<str>, ValueRef)],
        post_else: &[(Arc<str>, ValueRef)],
        cond_val: ValueRef,
    ) -> Result<Vec<(Arc<str>, ValueRef)>, IrBuildError> {
        let then_map: std::collections::HashMap<&str, ValueRef> =
            post_then.iter().map(|(n, v)| (n.as_ref(), *v)).collect();
        let else_map: std::collections::HashMap<&str, ValueRef> =
            post_else.iter().map(|(n, v)| (n.as_ref(), *v)).collect();

        let mut merged = Vec::with_capacity(pre.len().max(post_then.len()).max(post_else.len()));

        // 先处理 pre 中已有的变量
        let mut processed: std::collections::HashSet<&str> =
            pre.iter().map(|(n, _)| n.as_ref()).collect();
        for (name, pre_vr) in pre {
            let then_vr = then_map.get(name.as_ref()).copied();
            let else_vr = else_map.get(name.as_ref()).copied();
            let final_vr =
                Self::merge_local_value(self, *pre_vr, then_vr, else_vr, cond_val, name.as_ref())?;
            merged.push(((*name).clone(), final_vr));
        }

        // 处理 then/else 中新增的变量（pre 中不存在的）
        for (name, vr) in post_then.iter().chain(post_else.iter()) {
            if processed.insert(name.as_ref()) {
                let then_vr = then_map.get(name.as_ref()).copied();
                let else_vr = else_map.get(name.as_ref()).copied();
                let final_vr =
                    Self::merge_local_value(self, *vr, then_vr, else_vr, cond_val, name.as_ref())?;
                merged.push(((*name).clone(), final_vr));
            }
        }

        Ok(merged)
    }

    fn merge_local_value(
        builder: &mut IrBuilder,
        pre_vr: ValueRef,
        then_vr: Option<ValueRef>,
        else_vr: Option<ValueRef>,
        cond_val: ValueRef,
        _name: &str,
    ) -> Result<ValueRef, IrBuildError> {
        let final_vr = match (then_vr, else_vr) {
            (Some(t), Some(e)) if pre_vr == t && pre_vr == e => pre_vr,
            (Some(t), Some(e)) if t != pre_vr && e == pre_vr => {
                let dest = builder.alloc_value_ref();
                builder.emit(IrOp::Select {
                    condition: cond_val,
                    then_value: t,
                    else_value: pre_vr,
                    dest,
                })?;
                dest
            }
            (Some(t), Some(e)) if t == pre_vr && e != pre_vr => {
                let dest = builder.alloc_value_ref();
                builder.emit(IrOp::Select {
                    condition: cond_val,
                    then_value: pre_vr,
                    else_value: e,
                    dest,
                })?;
                dest
            }
            (Some(t), Some(e)) => {
                let dest = builder.alloc_value_ref();
                builder.emit(IrOp::Select {
                    condition: cond_val,
                    then_value: t,
                    else_value: e,
                    dest,
                })?;
                dest
            }
            _ => then_vr.or(else_vr).unwrap_or(pre_vr),
        };
        Ok(final_vr)
    }

    // ── 块表达式 ──

    /// 构建块表达式（语句列表 → 最后一个表达式的值）
    ///
    /// Block 类型定义为 `Vec<Stmt>`（无独立的 last_expr 字段）。
    /// 按顺序执行所有语句，返回隐式的 nil 值。
    fn build_block_expr(&mut self, statements: &[ast::Stmt]) -> Result<ValueRef, IrBuildError> {
        let mut last_val = self.emit_load_constant(IrConstant::Nil)?;
        for stmt in statements {
            if let ast::Stmt::Expr(expr) = stmt {
                last_val = self.build_expr(expr)?;
            } else {
                self.build_stmt(stmt)?;
            }
        }
        Ok(last_val)
    }

    // ========================================================================
    // 函数与闭包构建（Functions & Closures）
    // ========================================================================

    /// 构建函数表达式（具名或匿名 fn）
    ///
    /// 处理流程（参照 compiler/functions.rs 的 compile_fn）：
    /// 1. 收集自由变量 → 2. 分析捕获模式 → 3. 创建嵌套 IrFunction
    /// 4. 参数/捕获变量加载 → 5. 构建函数体 → 6. 发射 Closure 指令
    fn build_fn_expr(
        &mut self,
        name: Option<&str>,
        params: &[String],
        body: &ast::Block,
    ) -> Result<ValueRef, IrBuildError> {
        self.build_closure_expr(params, body, name)
    }

    /// 构建闭包表达式（核心闭包构建逻辑）
    ///
    /// 这是 IR Builder 中最复杂的方法，实现了完整的闭包捕获语义。
    ///
    /// # 流程
    /// 1. **自由变量收集**：扫描函数体，找出引用但未在参数/局部声明的标识符
    /// 2. **捕获分析**：判断每个自由变量是只读还是可写捕获
    /// 3. **嵌套函数创建**：生成新的 IrFunction，记录 captures 和 params
    /// 4. **上下文切换**：保存/恢复 locals 栈和 in_function 状态
    /// 5. **参数加载**：发射 LoadArg 指令将形参绑定到局部 ValueRef
    /// 6. **捕获加载**：发射 GetCapture 指令将捕获变量绑定到局部 ValueRef
    /// 7. **函数体编译**：递归构建函数体内的语句和表达式
    /// 8. **隐式 return nil**：如果函数体末尾没有显式 return，追加 return nil
    /// 9. **上下文恢复**：恢复外层 locals 和 in_function
    /// 10. **Closure 发射**：在外层发射 Closure 指令，返回闭包值
    ///
    /// # 参照
    /// - compiler/functions.rs:compile_fn() — 自由变量分析和 CaptureInfo 构建
    /// - compiler/functions.rs:collect_all_identifiers() — 标识符收集器模式
    fn build_closure_expr(
        &mut self,
        params: &[String],
        body: &ast::Block,
        name: Option<&str>,
    ) -> Result<ValueRef, IrBuildError> {
        // 1. 收集自由变量（参照 compiler/functions.rs 的 IdentifierCollector 逻辑）
        //
        // 🔧 关键修复：collect_free_variables 在外层 locals 上下文中执行，
        // 此时参数名尚未注册到 locals 中，会被误判为自由变量。
        // 必须在构建 captures 前过滤掉参数名，否则参数会被错误地当作捕获变量处理，
        // 导致运行时 GetCapture 发射 IndexOutOfBounds（因为闭包实际没有这些捕获）。
        // 同样需要过滤函数自身名：递归函数体中对自身名的引用（如 fact(n-1)）
        // 不是自由变量，而是全局/局部函数名引用，不应被当作捕获变量。
        let mut free_vars = self.collect_free_variables(body);
        free_vars.retain(|var_name| {
            !params.contains(var_name) && name.is_none_or(|fn_name| fn_name != var_name)
        });

        // 2. 分析捕获模式（只读 vs 可写）
        //
        // 🔧 Fix: 同时过滤全局函数（内置 + 用户定义）。
        // free_vars 在 step 1 中已过滤参数和自身名，但还包含 len/push 等内置函数名。
        // 如果不在此处过滤，ir_func.captures 会包含这些名字 → codegen 创建 prototype 时
        // 声明了 N 个捕获槽位，但运行时 Capture 指令不会填充它们（step 6 的 true_captures
        // 已过滤掉）→ captured 数组长度 < prototype 声明的槽位数 → IndexOutOfBounds。
        let assigned_vars = collect_assigned_vars_in_ir(body);
        let effective_free_vars: Vec<&String> =
            free_vars.iter().filter(|v| !self.is_global_function(v)).collect();
        let captures: Vec<CaptureDesc> = effective_free_vars
            .iter()
            .map(|var_name| CaptureDesc {
                name: var_name.as_str().into(),
                is_mutable: assigned_vars.contains(*var_name),
            })
            .collect();

        // 3. 创建嵌套 IrFunction
        let func_name = name.unwrap_or("<anonymous>");
        let func_id = self.module.add_function(func_name);
        {
            let ir_func = self.module.get_function_mut(func_id);
            ir_func.captures = captures.clone();
            for param in params {
                ir_func.params.push(param.as_str().into());
            }
        }

        // 4. 保存/切换上下文（保存 current_function_id 和 current_block_id）
        let saved_func_id = self.current_function_id;
        let saved_block_id = self.current_block_id;
        let saved_in_function = self.in_function;
        let saved_locals = std::mem::take(&mut self.locals);
        self.in_function = true;
        self.current_function_id = func_id;
        self.current_block_id = BasicBlockId(0); // 新函数从 bb0 开始
        // 压入当前闭包的捕获列表到 capture_stack（用于嵌套闭包的 Path 2 查找）
        self.capture_stack.push(captures.clone());

        // 5. 参数加载：为每个形参分配 ValueRef 并发射 LoadArg
        for (i, param) in params.iter().enumerate() {
            let vr = self.alloc_value_ref();
            self.emit(IrOp::LoadArg { dest: vr, index: i as u16 })?;
            self.locals.push((param.as_str().into(), vr));
        }

        // 6. 捕获变量加载：仅加载真正的词法捕获，排除全局函数名引用
        //
        // 🔧 关键修复：递归/互递归场景下，自由变量中的函数名引用不应通过 GetCapture 解析。
        // 例如 `fn fact(n) { return n * fact(n-1) }` 中，`fact` 是对全局函数的引用，
        // 不是词法闭包捕获。如果错误地发射 GetCapture，运行时会 IndexOutOfBounds
        // （因为 closure_indices 中没有对应条目）。
        //
        // 过滤策略：
        // - 排除已在 module.functions 中注册的同名非 main 函数（全局函数定义）
        // - 这是一种保守的启发式：未来应做完整的 scope analysis 来区分词法捕获 vs 全局引用
        let true_captures: Vec<&CaptureDesc> =
            captures.iter().filter(|c| !self.is_global_function(&c.name)).collect();

        for (i, capture) in true_captures.iter().enumerate() {
            let vr = self.alloc_value_ref();
            self.emit(IrOp::GetCapture { dest: vr, index: i as u16 })?;
            self.locals.push((capture.name.clone(), vr));
        }

        // 7. 构建闭包体（返回最后一个表达式的值作为隐式返回值）
        let last_body_val = self.build_block_statements_for_fn(body)?;

        // 8. 隐式 return（子函数的 terminator — 在子函数 IR 中）
        //
        // 🔧 Fix: 当函数体的最后一条语句是 if/else 且两个分支都以 return 终止时，
        // build_if_expr 会将当前块切换到 merge 块（不可达死代码）。
        // 此时再发射 return 会导致 merge 块引用未定义的 ValueRef（codegen 报错）。
        // 解决方案：检查当前块是否已终止，如果已终止则不再追加 return。
        // 这在语义上是正确的 — 所有控制流路径都已显式 return。
        if !self.block_is_terminated()? {
            self.emit_return(Some(last_body_val))?;
        }

        // 9. 分配闭包目标寄存器（Capture 和 Closure 都需要引用它）
        let dest = self.alloc_value_ref();

        // 10. 恢复上下文（弹出函数局部作用域，切回父函数）
        //
        // 🔴 关键：Capture 和 Closure 指令必须发射在父函数的基本块中，不是子函数的！
        // 原因：
        //   a) Capture 读取的是父函数寄存器的值（如 adder 中的 x=v34）
        //   b) Closure 创建闭包对象并关联到父函数的作用域
        //   c) 如果在子函数上下文中发射，Capture 会出现在子函数 return 之后 → 死代码删除
        self.in_function = saved_in_function;
        self.locals = saved_locals;
        self.current_function_id = saved_func_id;
        self.current_block_id = saved_block_id;
        self.capture_stack.pop();

        // 11. 发射 Closure 指令（🔴 必须在 Capture 之前！）
        //
        // 运行时语义：Closure 先创建闭包对象（分配 captured[] 空间），
        // 然后 Capture 逐槽填充值。所以 Closure 必须先于 Capture 发射。
        //
        // 编译器约束：Capture 的 closure 参数引用 dest（即 Closure 的目标寄存器），
        // 如果 Closure 还没发射，codegen 看到 capture dest[...] 时 dest 未定义 → Undefined ValueRef 错误。
        self.emit(IrOp::Closure { dest, ir_func: func_id })?;

        // 12. 发射 Capture 指令（🔴 在父函数上下文，Closure 之后！）
        //
        // 此时 self.locals 已恢复为父函数的 locals，resolve_local 可以找到
        // 被捕获的变量（如 adder 的 x、quicksort 的 pivot 等）。
        //
        // 捕获源解析三路径：
        //   Path 1: 变量在父函数 locals 中 → Register（直接从寄存器捕获）
        //   Path 2: 变量在更外层闭包的捕获列表中 → OuterCapture（跨层转发）
        //   Path 3: 变量是全局变量 → Global（codegen 会发射 GetGlobal 再按值捕获）
        //           典型场景：quicksort 中 pivot 用 global 存储，闭包 fn(x){x <= pivot} 需要捕获它
        for (i, capture) in true_captures.iter().enumerate() {
            let source = if let Some(vr) = self.resolve_local(&capture.name) {
                CaptureSource::Register(vr)
            } else if let Some(idx) = self.find_outer_capture_index(&capture.name) {
                CaptureSource::OuterCapture(idx)
            } else {
                // Path 3: 既不在 locals 也不在外层捕获中 → 当作全局变量处理
                // 不再输出 WARNING 跳过，而是通过 Global 路径在运行时解析
                CaptureSource::Global(capture.name.clone())
            };
            self.emit(IrOp::Capture { closure: dest, index: i as u16, source })?;
        }

        // 11. 具名函数绑定到作用域（参照 compiler.rs 的 bind_fn_to_scope）
        //
        // 当编译 `fn add(a, b) { return a + b }` 时，Closure 指令只创建了闭包值，
        // 但没有将函数名绑定到作用域。需要额外发射 SetGlobal 或 SetLocal，
        // 否则后续引用 `add` 时会找不到变量。
        if let Some(fn_name) = name {
            // 检查是否是局部变量（在当前作用域中已声明）
            if self.resolve_local(fn_name).is_some() {
                self.emit(IrOp::SetLocal { name: fn_name.into(), value: dest })?;
            } else {
                self.emit(IrOp::SetGlobal { name: fn_name.into(), value: dest })?;
            }
        }

        Ok(dest)
    }

    /// 收集函数体内引用但未在当前作用域声明的标识符（自由变量）
    ///
    /// 使用 `FreeVarCollector`（实现 `ExprVisitor` trait）统一遍历逻辑。
    /// 与 `nuzo_compiler::functions::IdentifierCollector` 共享同一遍历接口，
    /// 但增加了局部变量过滤：只返回不在 locals 中的标识符。
    ///
    /// # 算法
    /// 1. 扫描 Block 中所有语句和表达式
    /// 2. 收集所有 Ident 引用的名称
    /// 3. 过滤掉已在当前 locals 中绑定的名称
    /// 4. 去重后返回自由变量列表
    ///    ⚠️ 注意：参数名称不在当前 locals 中（它们在 build_closure_expr 步骤 5 才注册），
    ///    因此调用者（build_closure_expr）必须额外过滤 params。
    fn collect_free_variables(&self, body: &ast::Block) -> Vec<String> {
        let mut collector = FreeVarCollector {
            locals: &self.locals,
            free_vars: Vec::new(),
            seen: std::collections::HashSet::new(),
            in_nested_fn: self.in_function, // 已在函数内→嵌套函数不应把外层 locals 当局部变量
        };
        collector.collect_block(body);
        collector.free_vars
    }

    /// 检查指定名字是否为已注册的全局函数（非 main）
    ///
    /// 用于 build_closure_expr step 6 的捕获过滤：如果自由变量名与某个已注册的
    /// 全局函数同名，则该引用应走 GetGlobal 路径而非 GetCapture 路径。
    ///
    /// 典型场景：递归函数 `fn fact(n) { return n * fact(n-1) }` 中，
    /// 函数体对 `fact` 的引用是对全局函数的调用，不是闭包词法捕获。
    fn is_global_function(&self, name: &str) -> bool {
        // 用户定义的非 main 函数
        if self.module.functions.iter().any(|f| f.id.0 != 0 && f.name.as_ref() == name) {
            return true;
        }
        // 🔧 即将注册为全局函数的名字（如 `fib = fn(n){...}` 中 fib 在闭包构建时还未注册）
        if self.pending_global_fns.contains(name) {
            return true;
        }
        // 🔧 预扫描的顶层具名函数名（用于互递归/前向引用解析）
        //    + 调用方通过 build() 的 global_fn_names 参数注入的全局函数名（如 VM 内置函数）。
        //    不再硬编码内置函数名列表 — 由编译器从 BuiltinRegistry 权威获取并注入。
        if self.known_global_fns.contains(name) {
            return true;
        }
        false
    }

    /// 预扫描顶层语句，收集所有具名函数定义 (`fn name(...) {...}`) 的名字，
    /// 并将 import 引入的符号也加入 `known_global_fns`。
    ///
    /// 用于解决前向引用和互递归问题：
    /// - `fn a(n) { return b(n-1) }` 中 a 引用 b，但 b 可能在 a 之后才定义
    /// - 如果不预扫描，构建 a 时 b 还未注册到 module.functions → 被错误当作捕获变量
    /// - import 引入的 fn 名也需要加入集合，使闭包捕获过滤能识别它们为全局函数
    ///
    /// # 重名检测
    /// 若同一符号在 imports 中重复出现，返回 [`IrBuildError::Error`]
    /// （包装 [`ResolveError::DuplicateSymbol`] 语义）。
    fn pre_scan_global_fns(
        &mut self,
        statements: &[ast::Stmt],
        imports: &[ImportRecord],
    ) -> Result<(), IrBuildError> {
        // 1. 扫描顶层语句中的具名函数定义
        for stmt in statements {
            if let ast::Stmt::Expr(expr) = stmt {
                self.pre_scan_expr_for_fn_names(expr);
            }
            // Assign 语句中的 `fn name(...)` 也需要扫描（如 `fib = fn(n){...}`）
            if let ast::Stmt::Assign { target, value, .. } = stmt {
                if let ast::AssignTarget::Ident { name } = target
                    && matches!(value, ast::Expr::Fn { .. } | ast::Expr::Closure { .. })
                {
                    self.known_global_fns.insert(Arc::from(name.as_str()));
                }
                // 也扫描值表达式中的嵌套函数
                self.pre_scan_expr_for_fn_names(value);
            }
        }

        // 2. 将 import 引入的符号加入 known_global_fns，并做重名检测
        //    HashSet::insert 返回 false 表示符号已存在 → 视为重复定义
        for record in imports {
            for sym in &record.resolved_symbols {
                if !self.known_global_fns.insert(Arc::from(sym.as_str())) {
                    return Err(IrBuildError::Error {
                        message: format!(
                            "Duplicate symbol: '{}' (already defined in current or imported module)",
                            sym
                        ),
                        location: SourceLocation::default(),
                    });
                }
            }
        }

        Ok(())
    }

    /// 递归扫描表达式中的具名 Fn 定义
    fn pre_scan_expr_for_fn_names(&mut self, expr: &Expr) {
        match expr {
            Expr::Fn { name: Some(fn_name), .. } => {
                self.known_global_fns.insert(Arc::from(fn_name.as_str()));
            }
            Expr::Fn { body, .. } | Expr::Closure { body, .. } => {
                // 递归扫描函数体中的嵌套函数
                for stmt in body {
                    match stmt {
                        ast::Stmt::Expr(e) => self.pre_scan_expr_for_fn_names(e),
                        ast::Stmt::Assign { value, .. } => self.pre_scan_expr_for_fn_names(value),
                        ast::Stmt::Import { .. } => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// 构建函数体语句列表（用于闭包/函数内部）
    ///
    /// 与顶层 build_statements 的区别：
    /// - 不追加隐式 return nil（由调用者负责）
    /// - 支持函数内的赋值语句（更新 locals 映射）
    /// - 返回最后一个表达式的 ValueRef（用于隐式返回值）
    ///
    /// 🔧 关键修复：使用 alloc_value_ref() 而非 emit_load_constant(Nil) 作为初始值，
    /// 避免在函数体开头注入 LoadNil 指令。对于 `fn increment() { counter = counter + 1 }`
    /// 这样的函数，旧代码会发射 `v2 = load_const nil` 然后返回 v2（nil），
    /// 而非返回赋值结果 v5（counter+1 的正确值）。
    fn build_block_statements_for_fn(
        &mut self,
        body: &ast::Block,
    ) -> Result<ValueRef, IrBuildError> {
        // 使用 alloc_value_ref() 作为占位，不发射指令（避免 nil 污染返回值）
        let mut last_val = self.alloc_value_ref();
        let mut emitted_any = false;
        // 🔧 跟踪 last_val 是否被实际指令定义过（而非仅 alloc 占位）
        // 控制流语句（while/loop/if）不返回 ValueRef，导致 last_val 可能保持为"幽灵"值。
        // codegen Phase 1 验证时会检测到未定义的 ValueRef → 子函数注册失败 → Closure 崩溃。
        let mut last_val_defined = false;

        for stmt in body {
            if let ast::Stmt::Expr(expr) = stmt {
                last_val = self.build_expr(expr)?;
                emitted_any = true;
                last_val_defined = true; // build_expr 总是返回一个已定义的 ValueRef
            } else {
                // 赋值语句也可能产生有意义的返回值（赋值的右值）
                if let Some(val_ref) = self.build_stmt_in_fn(stmt)? {
                    last_val = val_ref;
                    emitted_any = true;
                    last_val_defined = true;
                }
            }
        }
        // 仅当函数体完全为空时，或 last_val 未被定义时，才 fallback 到 return nil
        if !emitted_any || !last_val_defined {
            last_val = self.emit_load_constant(IrConstant::Nil)?;
        }
        Ok(last_val)
    }

    /// 函数内部的语句构建（扩展版 build_stmt，支持赋值语句注册局部变量）
    ///
    /// 返回 `Option<ValueRef>`：如果语句产生有意义的值（如赋值的右值）则返回 Some，
    /// 供调用者用于隐式返回值。纯效果语句（如无返回值的操作）返回 None。
    ///
    /// 🔧 关键修复：赋值目标的作用域判断逻辑：
    /// - 如果目标已在 locals 中（参数或之前的局部赋值）→ 更新局部绑定（SetLocal 语义）
    /// - 如果目标不在 locals 中 → 发射 SetGlobal（全局变量赋值）
    ///   这确保 `fn increment() { counter = counter + 1 }` 中的 `counter` 赋值
    ///   写入全局作用域，而非创建一个函数内无效的局部变量。
    fn build_stmt_in_fn(&mut self, stmt: &ast::Stmt) -> Result<Option<ValueRef>, IrBuildError> {
        match stmt {
            ast::Stmt::Expr(expr) => {
                let val = self.build_expr(expr)?;
                Ok(Some(val))
            }
            ast::Stmt::Assign { target, value, .. } => {
                // 🔧 Fix: 对于 `name = fn(...){...}` 赋值，在构建闭包前将 name 标记为
                // "即将成为全局函数"。这样 build_closure_expr 中的自由变量收集能正确
                // 识别递归引用（如 fib 引用自身）是全局函数调用而非闭包捕获。
                let is_fn_assignment = matches!(target, ast::AssignTarget::Ident { .. })
                    && matches!(value, ast::Expr::Fn { .. } | ast::Expr::Closure { .. });
                if is_fn_assignment && let ast::AssignTarget::Ident { name } = target {
                    self.pending_global_fns.insert(Arc::from(name.as_str()));
                }

                let val_ref = self.build_expr(value)?;

                // 清理 pending 标记（无论成功与否）
                if is_fn_assignment && let ast::AssignTarget::Ident { name } = target {
                    self.pending_global_fns.remove(name.as_str());
                }

                if let ast::AssignTarget::Ident { name } = target {
                    // 检查是否是已有的局部变量（参数或之前在函数体内赋值的变量）
                    if let Some(pos) = self.locals.iter().position(|(n, _)| n.as_ref() == name) {
                        // 已有局部变量 → 更新局部绑定
                        self.locals[pos].1 = val_ref;
                    } else {
                        // 非局部变量 → 发射 SetGlobal 写入全局作用域
                        // 这是关键修复：旧代码用 define_local 创建了无效的局部绑定，
                        // 导致 `counter = counter + 1` 在函数体内只修改了局部副本
                        self.emit(IrOp::SetGlobal { name: name.as_str().into(), value: val_ref })?;
                    }
                }
                // 返回赋值的右值，使调用者能将其作为函数的隐式返回值
                Ok(Some(val_ref))
            }
            // import 已在 resolve_imports pass 中处理（递归编译依赖模块）
            ast::Stmt::Import { .. } => Ok(None),
        }
    }

    // ========================================================================
    // 复合类型字面量（Compound Literals）
    // ========================================================================

    /// 构建数组字面量
    ///
    /// 编译流程（参照 compiler/functions.rs:compile_array）：
    /// 1. 依次编译每个元素表达式 → ValueRef 列表
    /// 2. 发射 ArrayNew 指令，传入元素 ValueRef 列表
    fn build_array_literal(&mut self, elements: &[Expr]) -> Result<ValueRef, IrBuildError> {
        let mut elem_vals = Vec::with_capacity(elements.len());
        for el in elements {
            elem_vals.push(self.build_expr(el)?);
        }
        let dest = self.alloc_value_ref();
        self.emit(IrOp::ArrayNew { dest, elements: elem_vals })?;
        Ok(dest)
    }

    /// 构建字典字面量
    ///
    /// 编译流程（参照 compiler/functions.rs:compile_dict）：
    /// 1. 发射 ObjectNew 创建空对象
    /// 2. 对每个键值对：编译值表达式 → SetField 设置属性
    fn build_dict_literal(&mut self, pairs: &[(String, Expr)]) -> Result<ValueRef, IrBuildError> {
        let dest = self.alloc_value_ref();
        self.emit(IrOp::ObjectNew { dest })?;
        for (key, value) in pairs {
            let val_ref = self.build_expr(value)?;
            self.emit(IrOp::SetField { object: dest, field: key.as_str().into(), value: val_ref })?;
        }
        Ok(dest)
    }

    // ========================================================================
    // 语句构建（T5 控制流完善）
    // ========================================================================

    /// 构建语句列表
    fn build_statements(&mut self, stmts: &[ast::Stmt]) -> Result<(), IrBuildError> {
        for stmt in stmts {
            self.build_stmt(stmt)?;
        }
        Ok(())
    }

    /// 构建语句列表，同时返回最后一个表达式的 ValueRef
    ///
    /// 与 `build_statements()` 的区别：此方法追踪最后一个表达式语句的值，
    /// 用于 main 函数的隐式返回值（脚本语义：最后一个表达式的值作为脚本结果）。
    ///
    /// 例如 `fn f() { 42 }; f()` 应返回 `f()` 的调用结果（而非 nil）。
    ///
    /// 🔧 关键修复：语句型表达式（While/Loop/Break/Continue/Return/ForIn）在 Nuzo 中
    /// 是控制流语句，不应有"返回值"的概念。如果对它们调用 build_expr()，会注入
    /// nil 到 last_val，覆盖之前有效表达式的值（如赋值或函数调用的结果），
    /// 导致脚本隐式返回 nil 而非正确的值（参见 test_while_break/test_while_continue）。
    /// 因此对这些语句型表达式走 build_stmt() 路径，不更新 last_val。
    fn build_statements_with_last_expr(
        &mut self,
        stmts: &[ast::Stmt],
    ) -> Result<ValueRef, IrBuildError> {
        // 使用 alloc_value_ref() 而非 emit_load_constant(Nil)，避免在 r0 注入 LoadNil 指令。
        // VM 的 run_inner() 返回 registers[0]，如果 r0 被 nil 污染则脚本总是返回 nil。
        let mut last_val = self.alloc_value_ref();
        let mut emitted_any = false;
        for stmt in stmts {
            if let ast::Stmt::Expr(expr) = stmt {
                // 🔧 检查是否为语句型表达式（控制流语句，不应产生返回值）
                // 这些构造在 AST 中是 Expr 变体，但语义上是语句（statement）
                // 对它们使用 build_stmt() 避免注入 nil 到 last_val
                match expr {
                    Expr::While { .. }
                    | Expr::Loop { .. }
                    | Expr::Break { .. }
                    | Expr::Continue { .. }
                    | Expr::Return { .. }
                    | Expr::ForIn { .. } => {
                        // 语句型表达式 → 走语句路径，不更新 last_val
                        self.build_stmt(stmt)?;
                    }
                    _ => {
                        // 普通表达式 → 正常构建，追踪其值作为 last_val
                        last_val = self.build_expr(expr)?;
                        emitted_any = true;
                    }
                }
            } else {
                // 赋值语句也更新 last_val（赋值表达式的值就是所赋的值）
                // 这确保脚本末尾的 `result = expr` 返回 expr 的值而非之前的闭包
                if let ast::Stmt::Assign { target, value, .. } = stmt {
                    last_val = self.build_assign(target, value)?;
                    emitted_any = true;
                } else {
                    self.build_stmt(stmt)?;
                }
            }
        }
        // 仅当完全没有表达式语句时，才 fallback 到 return nil
        if !emitted_any {
            last_val = self.emit_load_constant(IrConstant::Nil)?;
        }
        Ok(last_val)
    }

    /// 构建单条语句 — 完整分发实现
    ///
    /// 支持：表达式语句、赋值语句、控制流语句（if/while/loop/break/continue/return）。
    /// 由于 Nuzo AST 中 if/while/break/continue/return 都是 Expr 变体，
    /// 它们通过 Stmt::Expr 包装后进入此函数，由 build_expr_stmt() 二次分发。
    fn build_stmt(&mut self, stmt: &ast::Stmt) -> Result<(), IrBuildError> {
        match stmt {
            ast::Stmt::Expr(expr) => self.build_expr_stmt(expr),
            ast::Stmt::Assign { target, value, .. } => {
                self.build_assign(target, value)?;
                Ok(())
            }
            // import 已在 resolve_imports pass 中处理（递归编译依赖模块）
            ast::Stmt::Import { .. } => Ok(()),
        }
    }

    /// 表达式作为语句 — 对控制流表达式进行二次分发
    ///
    /// Nuzo 的 if/while/loop/break/continue/return 在 AST 中都是 Expr 变体。
    /// 当它们出现在语句位置时（通过 Stmt::Expr 包装），需要在此处分发到
    /// 专用的语句级构建方法，因为这些构造在语句位置有特殊的控制流语义
    /// （如 break/continue 需要循环上下文，return 需要函数上下文）。
    fn build_expr_stmt(&mut self, expr: &Expr) -> Result<(), IrBuildError> {
        match expr {
            // 控制流表达式 → 专用构建器（丢弃返回值）
            Expr::If { .. } => {
                self.build_if_expr(expr)?;
                Ok(())
            }
            Expr::While { condition, body, .. } => self.build_while_stmt(condition, body),
            Expr::Loop { body, .. } => self.build_loop_stmt(body, None),
            Expr::Break { value, span } => self.build_break_stmt(value.as_deref(), span),
            Expr::Continue { span } => self.build_continue_stmt(span),
            Expr::Return { value, span } => self.build_return_stmt(value.as_deref(), span),
            Expr::ForIn { var_name, iterable, body, .. } => {
                self.build_for_in_stmt(var_name, iterable, body)
            }

            // 普通表达式 → 正常构建，丢弃返回值
            _ => {
                self.build_expr(expr)?;
                Ok(())
            }
        }
    }

    // ── 赋值语句 ──

    /// 构建赋值语句
    ///
    /// 语义参照 nuzo_compiler 的赋值编译：
    /// - Ident 目标：先查局部变量表，命中则 SetLocal，否则 SetGlobal
    /// - Index/Field 目标：发射对应的 Set 指令（Phase 1 暂仅支持 Ident）
    ///
    /// # 错误
    /// - 复合赋值目标（Index/Field）在 Phase 1 返回 Error
    fn build_assign(
        &mut self,
        target: &ast::AssignTarget,
        value: &Expr,
    ) -> Result<ValueRef, IrBuildError> {
        // 🔧 Fix: 与 build_stmt_in_fn 同样的 pending_global_fns 逻辑。
        // 顶层代码（main）中的 `name = fn(...){...}` 也需要标记，否则匿名闭包
        // 的自由变量收集无法识别即将成为全局函数的名字。
        let is_fn_assignment = matches!(target, ast::AssignTarget::Ident { .. })
            && matches!(value, ast::Expr::Fn { .. } | ast::Expr::Closure { .. });
        if is_fn_assignment && let ast::AssignTarget::Ident { name } = target {
            self.pending_global_fns.insert(Arc::from(name.as_str()));
        }

        // SCSB 拦截：检测 `s = s + expr` 模式（含多操作数链如 `s = s + "x" + i`），
        // 当 s 有活动的 SliceChain 时替换为多个 SliceChainAppend
        if let ast::AssignTarget::Ident { name } = target
            && let Some(&chain_vr) = self.scsb_chains.get(name.as_str())
            && let Some(operands) = extract_self_concat_operands(value, name)
        {
            // 逐个追加非自身操作数到 SliceChain
            for operand in &operands {
                let append_val = self.build_expr(operand)?;
                self.emit(IrOp::SliceChainAppend { chain: chain_vr, src: append_val })?;
            }
            // 清理 pending 标记
            if is_fn_assignment {
                self.pending_global_fns.remove(name.as_str());
            }
            // SCSB 赋值的值是 chain 自身，返回 chain_vr
            return Ok(chain_vr);
        }

        let value_ref = self.build_expr(value)?;

        // 清理 pending 标记
        if is_fn_assignment && let ast::AssignTarget::Ident { name } = target {
            self.pending_global_fns.remove(name.as_str());
        }
        match target {
            ast::AssignTarget::Ident { name } => {
                // 三级查找：局部变量优先
                if self.resolve_local(name).is_some() {
                    self.emit(IrOp::SetLocal { name: name.as_str().into(), value: value_ref })?;
                    // 更新 locals 栈中的绑定，使后续 build_ident 返回新 ValueRef
                    self.update_local_binding(name, value_ref);
                } else {
                    self.emit(IrOp::SetGlobal { name: name.as_str().into(), value: value_ref })?;
                }
                Ok(value_ref)
            }
            ast::AssignTarget::Index { object, index, .. } => {
                // arr[idx] = value
                // 简单标识符 → IndexSetMut（原地修改，引用语义，零克隆）
                // 复杂表达式 → IndexSet（COW，安全回退）
                let is_simple_ident = matches!(object.as_ref(), ast::Expr::Ident { .. });
                let obj_ref = self.build_expr(object)?;
                let idx_ref = self.build_expr(index)?;
                if is_simple_ident {
                    self.emit(IrOp::IndexSetMut {
                        object: obj_ref,
                        index: idx_ref,
                        value: value_ref,
                    })?;
                } else {
                    self.emit(IrOp::IndexSet {
                        object: obj_ref,
                        index: idx_ref,
                        value: value_ref,
                    })?;
                }
                Ok(value_ref)
            }
            ast::AssignTarget::Field { object, name, .. } => {
                // obj.field = value  →  emit SetField
                let obj_ref = self.build_expr(object)?;
                self.emit(IrOp::SetField {
                    object: obj_ref,
                    field: name.clone().into(),
                    value: value_ref,
                })?;
                Ok(value_ref)
            }
        }
    }

    // ── While 循环 ──

    /// 构建 while 循环语句
    ///
    /// 控制流结构（参照 compiler/statements.rs::compile_while）：
    /// ```text
    ///   cond_block (条件判断)
    ///     ├── condition 为真 → JumpIf → body_block
    ///     └── condition 为假 → JumpIf → exit_block
    ///   body_block (循环体)
    ///     ├── ... 用户代码 ...
    ///     └── 末尾隐式 Jump → cond_block (回跳)
    ///     ├── break → Jump → exit_block
    ///     └── continue → Jump → cond_block
    ///   exit_block (循环出口，后续代码继续)
    /// ```
    fn build_while_stmt(
        &mut self,
        condition: &Expr,
        body: &ast::Block,
    ) -> Result<(), IrBuildError> {
        // 创建循环所需的三个基本块
        let cond_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        // SCSB: 预扫描循环体，检测 `s = s + expr` 拼接模式
        let scsb_vars = self.scsb_scan_only(body);
        // SCSB: 在循环前（当前块）初始化切片链（仅执行一次）
        self.scsb_init_chains(&scsb_vars)?;

        // 从当前块（循环前）无条件跳转到条件块
        self.emit_jump(cond_block)?;

        // === 条件块：计算条件并决定分支 ===
        self.switch_to_block(cond_block);
        let cond_val = self.build_expr(condition)?;
        self.emit_jump_if(cond_val, body_block, exit_block)?;

        // === 循环体块 ===
        self.switch_to_block(body_block);
        self.loop_stack.push(LoopContext { exit_block, continue_block: cond_block, result: None });
        self.build_statements(body)?;
        self.loop_stack.pop();
        // 循环体末尾：回跳到条件块（如果循环体没有以 break/return 终止）
        if !self.block_is_terminated()? {
            self.emit_jump(cond_block)?;
        }

        // === 出口块：后续代码在此继续 ===
        self.switch_to_block(exit_block);
        // SCSB: 循环正常退出后完成切片链
        self.scsb_finish_and_assign(scsb_vars)?;
        Ok(())
    }

    // ── 无限循环（loop {}）──

    /// 构建 loop 无限循环语句
    ///
    /// 与 while 类似但没有条件判断，只能通过 break 退出。
    /// ```text
    ///   body_block (循环体)
    ///     ├── ... 用户代码 ...
    ///     └── 末尾隐式 Jump → body_block (无限回跳)
    ///     ├── break → Jump → exit_block
    ///     └── continue → Jump → body_block
    ///   exit_block (循环出口)
    /// ```
    fn build_loop_stmt(
        &mut self,
        body: &ast::Block,
        result: Option<ValueRef>,
    ) -> Result<(), IrBuildError> {
        let body_block = self.new_block();
        let exit_block = self.new_block();

        // 从当前块跳转到循环体入口
        self.emit_jump(body_block)?;

        // === 循环体块 ===
        self.switch_to_block(body_block);
        self.loop_stack.push(LoopContext { exit_block, continue_block: body_block, result });
        self.build_statements(body)?;
        self.loop_stack.pop();
        // 循环体末尾：无条件回跳到自身（形成无限循环）
        if !self.block_is_terminated()? {
            self.emit_jump(body_block)?;
        }

        // === 出口块：后续代码在此继续 ===
        self.switch_to_block(exit_block);
        // 如果 loop 有结果寄存器但未通过 break 赋值，初始化为 nil
        if let Some(result) = result {
            let nil_vr = self.emit_load_constant(IrConstant::Nil)?;
            self.emit(IrOp::Mov { dest: result, src: nil_vr })?;
        }
        Ok(())
    }

    // ── For-In 循环 ──

    /// 构建 for-in 循环语句
    ///
    /// 控制流结构（参照 while 循环模式，增加 init/step 块）：
    /// ```text
    ///   init_block (初始化：计算 iterable, idx=0, len)
    ///     └── Jump → cond_block
    ///   cond_block (条件判断: idx < len ?)
    ///     ├── true → JumpIf → body_block
    ///     └── false → JumpIf → exit_block
    ///   body_block (循环体: var = iter[idx]; 执行用户代码)
    ///     ├── ... 用户代码 ...
    ///     ├── break → Jump → exit_block
    ///     └── continue → Jump → step_block
    ///     └── 末尾隐式 Jump → step_block
    ///   step_block (步进: idx += 1)
    ///     └── Jump → cond_block (回跳到条件判断)
    ///   exit_block (循环出口，后续代码继续)
    /// ```
    fn build_for_in_stmt(
        &mut self,
        var_name: &str,
        iterable: &Expr,
        body: &ast::Block,
    ) -> Result<(), IrBuildError> {
        // 嵌套循环安全：用循环深度生成唯一变量名，避免内外层 __iter__/__idx__/__for_len__ 冲突
        let depth = self.loop_stack.len();
        let iter_name = format!("__iter_{}__", depth);
        let idx_name = format!("__idx_{}__", depth);
        let len_name = format!("__for_len_{}__", depth);

        // 创建5个基本块: init -> cond -> body -> step -> exit
        let init_block = self.new_block();
        let cond_block = self.new_block();
        let body_block = self.new_block();
        let step_block = self.new_block();
        let exit_block = self.new_block();

        // SCSB: 预扫描循环体，检测 `s = s + expr` 拼接模式
        let scsb_vars = self.scsb_scan_only(body);

        // 从当前块跳转到初始化块
        self.emit_jump(init_block)?;

        // === 初始化块：计算 iterable, 设置 idx=0, 计算长度 ===
        self.switch_to_block(init_block);
        let iter_val = self.build_expr(iterable)?;
        self.emit_set_local(&iter_name, iter_val)?;
        let zero = self.emit_load_constant(IrConstant::Number(0.0))?;
        self.emit_set_local(&idx_name, zero)?;
        let len_val = self.emit_len(iter_val)?;
        self.emit_set_local(&len_name, len_val)?;
        // SCSB: 在循环前初始化切片链（仅执行一次）
        self.scsb_init_chains(&scsb_vars)?;
        self.emit_jump(cond_block)?;

        // === 条件块：判断 idx < len ? ===
        self.switch_to_block(cond_block);
        let idx = self.emit_get_local(&idx_name)?;
        let len = self.emit_get_local(&len_name)?;
        let cond = self.emit_binary(IrBinOp::Lt, idx, len)?;
        self.emit_jump_if(cond, body_block, exit_block)?;

        // === 循环体块：var = iter[idx]; 执行循环体 ===
        self.switch_to_block(body_block);
        let iter = self.emit_get_local(&iter_name)?;
        let idx2 = self.emit_get_local(&idx_name)?;
        let elem = self.emit_index_get(iter, idx2)?;
        self.emit_set_local(var_name, elem)?;
        // 将循环变量注册到 locals 栈，使循环体内的 build_ident 能解析为局部变量（非 GetGlobal）
        self.define_local(var_name, elem);
        let loop_var_scope_depth = self.scope_depth(); // 记录深度，退出循环时清理
        // SCSB: scsb_vars 已在 init 块前预扫描完成，此处直接使用
        // 设置循环上下文（break/continue 目标）
        self.loop_stack.push(LoopContext { exit_block, continue_block: step_block, result: None });
        self.build_statements(body)?;
        self.loop_stack.pop();
        // SCSB: 循环结束后完成切片链（在 step_block 之前的 body_block 末尾）
        // 注意：finish 应在 exit_block 中执行，因为只有正常退出循环时才需要 finish
        // 但如果循环内有 break，finish 不会执行——这是可接受的退化（正确性由 SetLocal 保证）
        // 清理循环变量作用域（防止泄漏到循环外部）
        self.pop_scope(loop_var_scope_depth);
        // 循环体末尾：跳转到步进块（如果循环体没有以 break/return 终止）
        if !self.block_is_terminated()? {
            self.emit_jump(step_block)?;
        }

        // === 步进块：idx += 1 ===
        self.switch_to_block(step_block);
        let idx3 = self.emit_get_local(&idx_name)?;
        let one = self.emit_load_constant(IrConstant::Number(1.0))?;
        let new_idx = self.emit_binary(IrBinOp::Add, idx3, one)?;
        self.emit_set_local(&idx_name, new_idx)?;
        self.emit_jump(cond_block)?;

        // === 出口块：后续代码在此继续 ===
        self.switch_to_block(exit_block);
        // SCSB: 循环正常退出后完成切片链
        self.scsb_finish_and_assign(scsb_vars)?;
        Ok(())
    }

    // ── SCSB 辅助方法 ──

    /// 扫描循环体，检测 `s = s + expr` 拼接模式（仅检测，不初始化）
    ///
    /// 返回所有符合模式的变量名列表。
    /// 调用方应在循环前调用 scsb_init_chains 初始化切片链。
    fn scsb_scan_only(&mut self, body: &ast::Block) -> Vec<Arc<str>> {
        let mut concat_vars: Vec<Arc<str>> = Vec::new();
        for stmt in body {
            if let ast::Stmt::Assign { target, value, .. } = stmt
                && let ast::AssignTarget::Ident { name } = target
                && extract_self_concat_operands(value, name).is_some()
            {
                // 使用递归提取检测 `s = s + expr + ...` 模式（含多操作数链）
                let name_arc: Arc<str> = Arc::from(name.as_str());
                if !concat_vars.contains(&name_arc) {
                    concat_vars.push(name_arc.clone());
                }
            }
        }
        concat_vars
    }

    /// 在循环前为检测到的变量创建 SliceChain
    ///
    /// 在循环初始化块中调用，确保 SliceChain 在循环体之前创建。
    fn scsb_init_chains(&mut self, concat_vars: &[Arc<str>]) -> Result<(), IrBuildError> {
        for name in concat_vars {
            let chain_vr = self.alloc_value_ref();
            self.emit(IrOp::SliceChainInit { dest: chain_vr })?;
            self.scsb_chains.insert(name.clone(), chain_vr);
        }
        Ok(())
    }

    /// 循环结束后，完成所有 SCSB 切片链并赋值回原变量
    fn scsb_finish_and_assign(&mut self, concat_vars: Vec<Arc<str>>) -> Result<(), IrBuildError> {
        for name in &concat_vars {
            if let Some(chain_vr) = self.scsb_chains.remove(name) {
                let dest_vr = self.alloc_value_ref();
                self.emit(IrOp::SliceChainFinish { dest: dest_vr, chain: chain_vr })?;
                // 将结果赋值回原变量
                if self.resolve_local(name).is_some() {
                    self.emit(IrOp::SetLocal { name: name.clone(), value: dest_vr })?;
                    self.update_local_binding(name, dest_vr);
                } else {
                    self.emit(IrOp::SetGlobal { name: name.clone(), value: dest_vr })?;
                }
            }
        }
        Ok(())
    }

    // ── Break 语句 ──

    /// 构建 break 语句
    ///
    /// 语义参照 compiler/statements.rs::compile_break：
    /// - 必须在循环体内（loop_stack 非空）
    /// - 发射无条件跳转到最近循环的 exit_block
    fn build_break_stmt(&mut self, value: Option<&Expr>, span: &Span) -> Result<(), IrBuildError> {
        // 先提取 exit_block 和 result，避免借用 self 跨越 build_expr 的可变借用
        let (exit_block, result) = if self.loop_stack.is_empty() {
            return Err(IrBuildError::BreakOutsideLoop { location: span_to_location(span) });
        } else {
            // SAFETY: 前面已验证 loop_stack 非空
            let ctx = self.loop_stack.last().expect("loop_stack non-empty checked above");
            (ctx.exit_block, ctx.result)
        };

        // break 带值：将值 Mov 到 loop 的 result 寄存器（如果存在）
        if let Some(val_expr) = value {
            let val = self.build_expr(val_expr)?;
            if let Some(result) = result {
                self.emit(IrOp::Mov { dest: result, src: val })?;
            }
        }

        self.emit_jump(exit_block)?;
        Ok(())
    }

    // ── Continue 语句 ──

    /// 构建 continue 语句
    ///
    /// 语义参照 compiler/statements.rs::compile_continue：
    /// - 必须在循环体内
    /// - 发射无条件跳转到最近循环的 continue_block（通常是条件判断块）
    fn build_continue_stmt(&mut self, span: &Span) -> Result<(), IrBuildError> {
        let continue_block =
            self.loop_stack.last().map(|ctx| ctx.continue_block).ok_or_else(|| {
                IrBuildError::ContinueOutsideLoop { location: span_to_location(span) }
            })?;

        self.emit_jump(continue_block)?;
        Ok(())
    }

    // ── Return 语句 ──

    /// 构建 return 语句
    ///
    /// 语义参照 compiler/statements.rs::compile_return：
    /// - 必须在函数体内（in_function == true）
    /// - 有值：编译表达式 → Return(value)
    /// - 无值：Return(nil)
    fn build_return_stmt(&mut self, value: Option<&Expr>, span: &Span) -> Result<(), IrBuildError> {
        if !self.in_function {
            return Err(IrBuildError::ReturnOutsideFunction { location: span_to_location(span) });
        }

        let value = match value {
            Some(expr) => Some(self.build_expr(expr)?),
            None => Some(self.emit_load_constant(IrConstant::Nil)?),
        };
        self.emit_return(value)?;
        Ok(())
    }

    // ========================================================================
    // 公共访问器
    // ========================================================================

    /// 消费构建器，返回构建完成的 IR 模块
    pub fn into_module(self) -> IrModule {
        self.module
    }
}

impl Default for IrBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 自由变量收集器（Free Variable Collector）
// ============================================================================

/// 自由变量收集器 — 扫描 AST 块，收集引用但未在当前作用域声明的标识符
///
/// 使用 `nuzo_frontend::ast::ExprVisitor` trait 统一遍历逻辑。
/// 与 `nuzo_compiler::functions::IdentifierCollector` 共享同一遍历接口，
/// 但增加了局部变量过滤：只返回不在 locals 中的自由变量。
///
/// # 设计决策
/// - 使用 `HashSet` 进行去重，避免重复收集同一变量
/// - 进入嵌套 Fn/Closure 体时继续收集（支持跨层闭包捕获分析）
/// - 局部赋值（let x = ...）不视为自由变量
struct FreeVarCollector<'a> {
    /// 当前作用域的局部变量列表（用于判断是否为自由变量）
    locals: &'a [(Arc<str>, ValueRef)],
    /// 收集到的自由变量名称（有序，去重）
    free_vars: Vec<String>,
    /// 已见过的标识符（用于去重）
    seen: std::collections::HashSet<String>,
    /// 是否在嵌套函数体内
    ///
    /// 当递归进入嵌套函数体时，外层 locals 中的变量对嵌套函数来说是
    /// 自由变量（需要被捕获），不应被过滤。设置此标志后，
    /// `collect_expr(Ident)` 不再检查 `self.locals`，只检查 `self.seen`
    /// （即嵌套函数自身的参数和局部赋值）。
    in_nested_fn: bool,
}

impl<'a> ast::ExprVisitor for FreeVarCollector<'a> {
    /// 标识符访问：判断是否为自由变量
    fn visit_ident(&mut self, name: &str, _span: &ast::Span) {
        // 判断是否为自由变量：
        // - 在嵌套函数体内：只检查 seen（嵌套函数自身的参数/局部赋值），
        //   不检查外层 locals（外层变量对嵌套函数来说是自由变量）
        // - 在当前函数体内：检查 locals + seen
        let is_local = if self.in_nested_fn {
            self.seen.contains(name)
        } else {
            self.locals.iter().any(|(n, _)| n.as_ref() == name) || self.seen.contains(name)
        };
        if !is_local {
            self.seen.insert(name.to_string());
            self.free_vars.push(name.to_string());
        }
    }

    /// 函数/闭包定义：标记函数名和参数为局部变量，不递归进入函数体
    ///
    /// 🔧 关键设计：不递归进入嵌套函数体。
    /// 嵌套函数的自由变量由其自身的 build_closure_expr → collect_free_variables 收集。
    /// 如果递归会把嵌套函数的自由变量混入当前函数的 free_vars，
    /// 导致后续 retain(!params.contains) 误删。
    fn visit_fn(
        &mut self,
        name: Option<&str>,
        params: &[String],
        _body: &ast::Block,
        _span: &ast::Span,
    ) {
        if let Some(fn_name) = name {
            // 具名函数声明标记为局部定义，防止函数名被收集为自由变量
            self.seen_local(fn_name);
        }
        // 标记嵌套函数的参数为局部变量
        for param in params {
            self.seen_local(param);
        }
    }

    /// 赋值语句：先标记赋值目标为局部变量，再遍历赋值值和目标中的子表达式
    fn visit_assign(&mut self, target: &ast::AssignTarget, value: &Expr, _span: &ast::Span) {
        // 先标记赋值目标为局部变量（防止误判为自由变量）
        if let ast::AssignTarget::Ident { name } = target {
            self.seen_local(name);
        }
        // 遍历赋值值
        self.visit_expr(value);
        // 遍历赋值目标中的子表达式（如 obj.field = expr 中的 obj）
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
}

impl<'a> FreeVarCollector<'a> {
    fn collect_block(&mut self, block: &ast::Block) {
        for stmt in block {
            self.collect_stmt(stmt);
        }
    }

    fn collect_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Expr(expr) => self.visit_expr(expr),
            ast::Stmt::Assign { target, value, span } => {
                self.visit_assign(target, value, span);
            }
            ast::Stmt::Import { .. } => {}
        }
    }

    /// 标记一个名字为"已见的局部变量"，防止被收集为自由变量
    fn seen_local(&mut self, name: &str) {
        self.seen.insert(name.to_string());
    }
}

// ============================================================================
// 辅助函数：收集被赋值的变量名（用于判断可变捕获）
// ============================================================================

/// 收集块中所有被赋值的变量名
///
/// 使用 `AssignedVarCollector`（实现 `ExprVisitor` trait）统一遍历逻辑。
/// 与 `nuzo_compiler::functions::CompilerAssignedVarCollector` 逻辑一致。
/// 用于判断闭包捕获模式：被赋值的变量需要 ByBox（可变）捕获。
fn collect_assigned_vars_in_ir(block: &ast::Block) -> std::collections::HashSet<String> {
    let mut walker = AssignedVarCollector::new();
    // 遍历语句块 — 所有赋值收集通过 walker 统一处理
    for stmt in block {
        match stmt {
            ast::Stmt::Assign { target, value, span } => {
                walker.visit_assign(target, value, span);
            }
            ast::Stmt::Expr(expr) => walker.visit_expr(expr),
            ast::Stmt::Import { .. } => {}
        }
    }
    walker.finish()
}

/// 被赋值变量收集器 — 使用 ExprVisitor trait 统一遍历逻辑
///
/// 与 FreeVarCollector 的关键区别：
/// - **会递归进入函数体**（需要发现内部赋值以判断可变捕获）
/// - **拦截 Assign 语句**收集被赋值的标识符名
struct AssignedVarCollector {
    assigned: std::collections::HashSet<String>,
}

impl AssignedVarCollector {
    fn new() -> Self {
        Self { assigned: std::collections::HashSet::new() }
    }

    fn finish(self) -> std::collections::HashSet<String> {
        self.assigned
    }
}

impl ExprVisitor for AssignedVarCollector {
    /// 拦截赋值语句：记录被赋值的标识符
    fn visit_assign(&mut self, target: &ast::AssignTarget, value: &Expr, _span: &ast::Span) {
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

    /// 函数/闭包：递归进入函数体（与 FreeVarCollector 不同）
    ///
    /// 需要发现函数内部的赋值以判断是否需要 ByBox（可变）捕获。
    fn visit_fn(
        &mut self,
        _name: Option<&str>,
        _params: &[String],
        body: &ast::Block,
        _span: &ast::Span,
    ) {
        for stmt in body {
            match stmt {
                ast::Stmt::Assign { target, .. } => {
                    if let ast::AssignTarget::Ident { name } = target {
                        self.assigned.insert(name.clone());
                    }
                }
                ast::Stmt::Expr(expr) => self.visit_expr(expr),
                ast::Stmt::Import { .. } => {}
            }
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数：从源代码字符串构建简单表达式 IR
    fn build_expr_from_source(source: &str) -> Result<IrModule, IrBuildError> {
        // 解析源代码 → AST（Parser::parse 内部完成词法 + 语法分析）
        let program = nuzo_frontend::parser::Parser::parse(source).expect("parse ok");

        // 构建 IR（测试场景不注入内置函数名，仅验证 IR 层自身逻辑）
        IrBuilder::build(&program, &[])
    }

    /// 辅助函数：提取 main 函数中的所有指令
    fn get_main_instructions(module: &IrModule) -> &Vec<IrOp> {
        &module.functions[0].blocks[0].instructions
    }

    #[test]
    fn test_number_literal() {
        let module = build_expr_from_source("42").unwrap();
        let instrs = get_main_instructions(&module);

        // 应该有 LoadConstant(Number(42)) 和 Return
        assert!(instrs.len() >= 2);
        assert!(
            matches!(&instrs[0], IrOp::LoadConstant { constant: IrConstant::Number(n), .. } if *n == 42.0)
        );
    }

    #[test]
    fn test_string_literal() {
        let module = build_expr_from_source("\"hello\"").unwrap();
        let instrs = get_main_instructions(&module);

        assert!(
            matches!(&instrs[0], IrOp::LoadConstant { constant: IrConstant::String(s), .. } if s.as_ref() == "hello")
        );
    }

    #[test]
    fn test_bool_literal() {
        let module = build_expr_from_source("true").unwrap();
        let instrs = get_main_instructions(&module);

        assert!(matches!(&instrs[0], IrOp::LoadConstant { constant: IrConstant::Bool(true), .. }));
    }

    #[test]
    fn test_nil_literal() {
        let module = build_expr_from_source("nil").unwrap();
        let instrs = get_main_instructions(&module);

        assert!(matches!(&instrs[0], IrOp::LoadConstant { constant: IrConstant::Nil, .. }));
    }

    #[test]
    fn test_binary_add() {
        let module = build_expr_from_source("1 + 2").unwrap();
        let instrs = get_main_instructions(&module);

        // 应该有 LoadConstant(1), LoadConstant(2), Binary(Add), Return(nil)
        assert!(instrs.len() >= 4);
        // 找到 Binary Add 指令
        let has_add = instrs.iter().any(|op| matches!(op, IrOp::Binary { op: IrBinOp::Add, .. }));
        assert!(has_add, "Expected Binary Add instruction");
    }

    #[test]
    fn test_binary_sub_mul_div() {
        // 测试多种算术运算符
        for (src, expected_op) in
            [("3 - 1", IrBinOp::Sub), ("2 * 3", IrBinOp::Mul), ("10 / 2", IrBinOp::Div)]
        {
            let module = build_expr_from_source(src).unwrap();
            let instrs = get_main_instructions(&module);
            let found =
                instrs.iter().any(|op| matches!(op, IrOp::Binary { op, .. } if *op == expected_op));
            assert!(found, "Expected {:?} for source '{}'", expected_op, src);
        }
    }

    #[test]
    fn test_comparison_ops() {
        // 测试比较运算符映射
        for (src, expected_op) in [
            ("1 == 2", IrBinOp::Eq),
            ("1 != 2", IrBinOp::Neq),
            ("1 < 2", IrBinOp::Lt),
            ("1 > 2", IrBinOp::Gt),
            ("1 <= 2", IrBinOp::Le),
            ("1 >= 2", IrBinOp::Ge),
        ] {
            let module = build_expr_from_source(src).unwrap();
            let instrs = get_main_instructions(&module);
            let found =
                instrs.iter().any(|op| matches!(op, IrOp::Binary { op, .. } if *op == expected_op));
            assert!(found, "Expected {:?} for source '{}'", expected_op, src);
        }
    }

    #[test]
    fn test_unary_negate() {
        let module = build_expr_from_source("-42").unwrap();
        let instrs = get_main_instructions(&module);

        let has_neg = instrs.iter().any(|op| matches!(op, IrOp::Unary { op: IrUnaryOp::Neg, .. }));
        assert!(has_neg, "Expected Unary Neg instruction");
    }

    #[test]
    fn test_unary_not() {
        let module = build_expr_from_source("!true").unwrap();
        let instrs = get_main_instructions(&module);

        let has_not = instrs.iter().any(|op| matches!(op, IrOp::Unary { op: IrUnaryOp::Not, .. }));
        assert!(has_not, "Expected Unary Not instruction");
    }

    #[test]
    fn test_global_ident() {
        // 未定义的标识符应生成 GetGlobal
        let module = build_expr_from_source("x").unwrap();
        let instrs = get_main_instructions(&module);

        let has_get_global = instrs
            .iter()
            .any(|op| matches!(op, IrOp::GetGlobal { name, .. } if name.as_ref() == "x"));
        assert!(has_get_global, "Expected GetGlobal 'x' instruction");
    }

    #[test]
    fn test_nested_binary() {
        // 测试嵌套二元运算: 1 + 2 * 3
        // Parser 应该解析为: 1 + (2 * 3)
        let module = build_expr_from_source("1 + 2 * 3").unwrap();
        let instrs = get_main_instructions(&module);

        // 应该有两个 Binary 指令（Mul 和 Add）
        let binary_count = instrs.iter().filter(|op| matches!(op, IrOp::Binary { .. })).count();
        assert_eq!(binary_count, 2, "Expected 2 Binary instructions for nested expression");
    }

    #[test]
    fn test_if_expression_basic() {
        // 基本 if 表达式应生成 JumpIf + 基本块
        let module = build_expr_from_source("if (true) { 1 }").unwrap();

        // 应至少有多个基本块（entry + then + merge）
        assert!(
            module.functions[0].blocks.len() >= 3,
            "Expected at least 3 basic blocks for if expression"
        );
    }

    #[test]
    fn test_value_ref_monotonic() {
        // ValueRef 应单调递增
        let module = build_expr_from_source("1 + 2 + 3").unwrap();
        let mut max_vr: u32 = 0;
        for op in get_main_instructions(&module) {
            if let Some(vr) = op.dest() {
                max_vr = max_vr.max(vr.0);
            }
        }
        // 至少应有几个不同的 ValueRef
        assert!(max_vr >= 3, "Expected at least 4 ValueRefs (0-3), got max={}", max_vr);
    }

    #[test]
    fn test_module_has_main_function() {
        let module = build_expr_from_source("42").unwrap();

        assert_eq!(module.functions.len(), 1, "Expected exactly 1 function (main)");
        assert_eq!(module.functions[0].name.as_ref(), "main");
    }

    #[test]
    fn test_implicit_return_nil() {
        // 即使源代码只有表达式，末尾也应有 return nil
        let module = build_expr_from_source("42").unwrap();
        let instrs = get_main_instructions(&module);

        let last_is_return = matches!(instrs.last(), Some(IrOp::Return { .. }));
        assert!(last_is_return, "Last instruction should be Return");
    }

    #[test]
    fn test_call_expression() {
        // 函数调用应生成 Call 指令
        let module = build_expr_from_source("foo()").unwrap();
        let instrs = get_main_instructions(&module);

        let has_call = instrs.iter().any(|op| matches!(op, IrOp::Call { .. }));
        assert!(has_call, "Expected Call instruction");
    }

    #[test]
    fn test_call_with_args() {
        let module = build_expr_from_source("foo(1, 2)").unwrap();
        let instrs = get_main_instructions(&module);

        // 找到 Call 指令并检查参数数量
        let call = instrs.iter().find(|op| matches!(op, IrOp::Call { .. }));
        assert!(call.is_some(), "Expected Call instruction");
        if let IrOp::Call { args, .. } = call.unwrap() {
            assert_eq!(args.len(), 2, "Expected 2 arguments");
        }
    }

    #[test]
    fn test_index_expression() {
        let module = build_expr_from_source("arr[0]").unwrap();
        let instrs = get_main_instructions(&module);

        let has_index = instrs.iter().any(|op| matches!(op, IrOp::IndexGet { .. }));
        assert!(has_index, "Expected IndexGet instruction");
    }

    #[test]
    fn test_field_expression() {
        let module = build_expr_from_source("obj.name").unwrap();
        let instrs = get_main_instructions(&module);

        let has_field = instrs
            .iter()
            .any(|op| matches!(op, IrOp::GetField { field, .. } if field.as_ref() == "name"));
        assert!(has_field, "Expected GetField 'name' instruction");
    }

    #[test]
    fn test_complex_expression() {
        // 综合测试: 一元 + 二元 + 字面量
        let module = build_expr_from_source("-1 + 2 * 3 >= 5").unwrap();
        let instrs = get_main_instructions(&module);

        // 应包含 Unary(Neg), Binary(Mul), Binary(Add), Binary(Ge)
        let unary_count = instrs.iter().filter(|op| matches!(op, IrOp::Unary { .. })).count();
        let binary_count = instrs.iter().filter(|op| matches!(op, IrOp::Binary { .. })).count();
        assert_eq!(unary_count, 1, "Expected 1 Unary instruction");
        assert_eq!(binary_count, 3, "Expected 3 Binary instructions (Mul, Add, Ge)");
    }

    #[test]
    fn test_builder_default() {
        // Default trait 应正常工作
        let builder = IrBuilder::default();
        let _module = builder.into_module();
    }

    #[test]
    fn test_multiple_statements() {
        // 多条语句都应被编译（3 个表达式，隐式返回最后一个表达式的值）
        let module = build_expr_from_source("1\n2\n3").unwrap();
        let instrs = get_main_instructions(&module);

        // 应有 3 个 LoadConstant（3 个字面量），不再额外发射 return nil
        // 因为 build_statements_with_last_expr 使用 alloc_value_ref() 作为初始值，
        // 仅在没有任何表达式语句时才 fallback 到 return nil
        let load_count = instrs.iter().filter(|op| matches!(op, IrOp::LoadConstant { .. })).count();
        assert_eq!(load_count, 3, "Expected 3 LoadConstant instructions (3 exprs, no extra nil)");
    }

    // ========================================================================
    // 控制流测试（T5: if/while/loop/break/continue/return/assign）
    // ========================================================================

    #[test]
    fn test_while_loop_basic() {
        // 基本 while 循环应生成多个基本块和 JumpIf 指令
        let module = build_expr_from_source("while (true) { 42 }").unwrap();
        let func = &module.functions[0];

        // while 循环至少产生：entry + cond + body + exit = 4 个基本块
        assert!(
            func.blocks.len() >= 4,
            "Expected at least 4 basic blocks for while loop, got {}",
            func.blocks.len()
        );

        // 应包含 JumpIf（条件跳转）
        let has_jump_if = func
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|op| matches!(op, IrOp::JumpIf { .. }));
        assert!(has_jump_if, "Expected JumpIf instruction in while loop");
    }

    #[test]
    fn test_loop_infinite_basic() {
        // 无限循环应生成 body + exit 基本块
        let module = build_expr_from_source("loop { break }").unwrap();
        let func = &module.functions[0];

        // loop 至少产生：entry + body + exit = 3 个基本块
        assert!(
            func.blocks.len() >= 3,
            "Expected at least 3 basic blocks for loop, got {}",
            func.blocks.len()
        );
    }

    #[test]
    fn test_break_in_loop() {
        // 循环中的 break 应成功构建（不报错）
        let result = build_expr_from_source("while (false) { break }");
        assert!(result.is_ok(), "break inside loop should succeed");
    }

    #[test]
    fn test_break_outside_loop_error() {
        // 循环外的 break 应报错
        let result = build_expr_from_source("break");
        assert!(result.is_err(), "break outside loop should fail");
        let err = result.unwrap_err();
        let msg = err.to_single_line();
        assert!(msg.contains("break"), "Error message should mention 'break', got: {}", msg);
    }

    #[test]
    fn test_continue_in_loop() {
        // 循环中的 continue 应成功构建
        let result = build_expr_from_source("while (true) { continue }");
        assert!(result.is_ok(), "continue inside loop should succeed");
    }

    #[test]
    fn test_continue_outside_loop_error() {
        // 循环外的 continue 应报错
        let result = build_expr_from_source("continue");
        assert!(result.is_err(), "continue outside loop should fail");
        let err = result.unwrap_err();
        let msg = err.to_single_line();
        assert!(msg.contains("continue"), "Error message should mention 'continue', got: {}", msg);
    }

    #[test]
    fn test_assign_statement() {
        // 赋值语句应生成 SetGlobal 指令（顶层作用域，非局部变量）
        let module = build_expr_from_source("x = 42").unwrap();
        let instrs = get_main_instructions(&module);

        let has_set_global = instrs
            .iter()
            .any(|op| matches!(op, IrOp::SetGlobal { name, .. } if name.as_ref() == "x"));
        assert!(has_set_global, "Expected SetGlobal 'x' instruction for assignment");
    }

    #[test]
    fn test_assign_with_expression() {
        // 赋值右侧可以是任意表达式
        let module = build_expr_from_source("y = 1 + 2").unwrap();
        let instrs = get_main_instructions(&module);

        // 应有 Binary(Add) 和 SetGlobal
        let has_binary =
            instrs.iter().any(|op| matches!(op, IrOp::Binary { op: IrBinOp::Add, .. }));
        let has_set_global = instrs
            .iter()
            .any(|op| matches!(op, IrOp::SetGlobal { name, .. } if name.as_ref() == "y"));
        assert!(has_binary, "Expected Binary Add in assignment RHS");
        assert!(has_set_global, "Expected SetGlobal 'y' instruction");
    }

    #[test]
    fn test_if_else_as_statement() {
        // if-else 作为语句（通过 Stmt::Expr 包装）应正常工作
        let module = build_expr_from_source("if (true) { 1 } else { 2 }").unwrap();
        let func = &module.functions[0];

        // if-else 至少产生：entry + then + else + merge = 4 个基本块
        assert!(
            func.blocks.len() >= 4,
            "Expected at least 4 basic blocks for if-else, got {}",
            func.blocks.len()
        );
    }

    #[test]
    fn test_nested_control_flow() {
        // 嵌套控制流：if 内含 while
        let result = build_expr_from_source("if (true) { while (false) { break } }");
        assert!(result.is_ok(), "Nested if+while+break should succeed");
    }

    #[test]
    fn test_while_with_multiple_statements() {
        // while 循环体含多条语句
        let result = build_expr_from_source("while (true) { x = 1 y = 2 }");
        assert!(result.is_ok(), "while with multiple statements should succeed");

        let module = result.unwrap();
        let instrs: Vec<_> =
            module.functions[0].blocks.iter().flat_map(|b| b.instructions.iter()).collect();

        // 应有 SetGlobal x 和 SetGlobal y
        let set_count = instrs.iter().filter(|op| matches!(op, IrOp::SetGlobal { .. })).count();
        assert!(set_count >= 2, "Expected at least 2 SetGlobal instructions, got {}", set_count);
    }

    #[test]
    fn test_complex_control_flow_blocks() {
        // 综合测试：混合使用多种控制流结构
        let source = r#"
x = 0
while (x < 10) {
    x = x + 1
    if (x > 5) { break }
}
"#;
        let result = build_expr_from_source(source);
        assert!(result.is_ok(), "Complex control flow should compile successfully");

        let module = result.unwrap();
        let func = &module.functions[0];
        // 复杂控制流应产生大量基本块
        assert!(
            func.blocks.len() >= 5,
            "Expected at least 5 basic blocks for complex control flow, got {}",
            func.blocks.len()
        );

        // 验证关键指令类型存在
        let all_instrs: Vec<_> = func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();

        let has_jump_if = all_instrs.iter().any(|op| matches!(op, IrOp::JumpIf { .. }));
        let has_jump = all_instrs.iter().any(|op| matches!(op, IrOp::Jump { .. }));
        assert!(has_jump_if, "Expected JumpIf in complex control flow");
        assert!(has_jump, "Expected Jump in complex control flow");
    }

    // 临时诊断测试：dump while_break 的完整 IR，用于调试 TypeMismatch 问题
    #[test]
    fn test_diagnostic_while_break_ir() {
        let source = "i = 0; while true { if i >= 3 { break }; i = i + 1 }";
        let module = build_expr_from_source(source).unwrap();
        // 输出完整的 IR 用于诊断
        eprintln!("=== IR DUMP (while_break) ===\n{}", module);
        assert!(!module.functions.is_empty());
    }

    // ========================================================================
    // Import 合并测试
    // ========================================================================

    /// 内存模块解析器 — 用于测试 import 功能，无需真实文件系统
    struct InMemoryResolver {
        sources: std::collections::HashMap<PathBuf, String>,
    }

    impl InMemoryResolver {
        fn new() -> Self {
            Self { sources: std::collections::HashMap::new() }
        }

        fn add_module(&mut self, path: &str, source: &str) {
            self.sources.insert(PathBuf::from(path), source.to_string());
        }
    }

    impl ModuleResolver for InMemoryResolver {
        fn resolve(
            &self,
            _current: Option<&Path>,
            import_path: &str,
        ) -> Result<PathBuf, ResolveError> {
            Ok(PathBuf::from(import_path))
        }

        fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
            self.sources.get(path).cloned().ok_or_else(|| ResolveError::ModuleNotFound {
                path: path.display().to_string(),
                location: SourceLocation::default(),
            })
        }

        fn check_circular(&self, _path: &Path, _stack: &[PathBuf]) -> Result<(), ResolveError> {
            Ok(())
        }
    }

    #[test]
    fn test_import_merges_functions() {
        let mut resolver = InMemoryResolver::new();
        resolver.add_module("math_lib", "fn add(a, b) { a + b }");

        let source = r#"import "math_lib"
add(1, 2)"#;
        let program = Parser::parse(source).expect("parse ok");
        let module = IrBuilder::build_with_resolver(&program, &[], &resolver, None)
            .expect("build with imports");

        // main + imported add
        assert!(
            module.functions.len() >= 2,
            "Expected at least 2 functions (main + imported), got {}",
            module.functions.len()
        );

        let has_add = module.functions.iter().any(|f| f.name.as_ref() == "add");
        assert!(has_add, "Expected imported function 'add'");

        // main should emit Closure + SetGlobal "add"
        let main_instrs: Vec<_> =
            module.functions[0].blocks.iter().flat_map(|b| b.instructions.iter()).collect();

        let has_setglobal_add = main_instrs
            .iter()
            .any(|op| matches!(op, IrOp::SetGlobal { name, .. } if name.as_ref() == "add"));
        assert!(has_setglobal_add, "Expected SetGlobal 'add' in main");

        let has_closure = main_instrs.iter().any(|op| matches!(op, IrOp::Closure { .. }));
        assert!(has_closure, "Expected Closure instruction in main");
    }
}

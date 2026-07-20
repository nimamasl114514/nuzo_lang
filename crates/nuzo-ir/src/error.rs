//! IR 错误类型 — 构建期错误与验证期错误
//!
//! ## 错误代码体系
//! - `IRB001`-`IRB999`: IrBuildError（构建期错误）
//! - `IRV001`-`IRV999`: IrValidationError（验证期错误）
//! - `IRW001`-`IRW999`: ValidationWarning（验证期警告）
//!
//! 每个错误变体实现 [`IrErrorCode`] trait，提供 `error_code` / `severity` / `category` / `help`
//! 四个方法，支持结构化诊断与多行 Display 输出。

use nuzo_core::SourceLocation;
use std::fmt;

// ============================================================================
// 错误基础类型：严重级别 / 分类 / 错误代码 trait
// ============================================================================
//
// 设计原则：轻量级、不依赖 nuzo_error（L4 同级依赖隔离）。
// nuzo_error 依赖 nuzo_bytecode/nuzo_values，若 nuzo_ir 复用会引入跨层依赖。
// 因此 nuzo_ir 内部定义独立的错误分类体系，命名前缀 `Ir*` 与 nuzo_error 隔离。

/// IR 错误严重级别
///
/// 轻量级版本，独立于 `nuzo_error::ErrorSeverity`（避免 L4 同级依赖）。
/// 用于错误分类与诊断展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrErrorSeverity {
    /// 错误（阻止构建/验证通过）
    Error,
    /// 警告（不阻止验证通过，但提示潜在问题）
    Warning,
    /// 信息提示（仅作记录，不影响流程）
    Info,
}

impl IrErrorSeverity {
    /// 转换为简短标识符（用于日志/Display 单行格式）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

impl fmt::Display for IrErrorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => write!(f, "错误"),
            Self::Warning => write!(f, "警告"),
            Self::Info => write!(f, "信息"),
        }
    }
}

/// IR 错误分类
///
/// 用于错误归类与诊断建议生成。独立于 `nuzo_error::ErrorCategory`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrErrorCategory {
    /// 语义错误（变量未定义、break/continue/return 位置错误等）
    Semantic,
    /// 结构错误（基本块/ValueRef/函数引用等 IR 结构非法）
    Structural,
    /// 限制错误（局部变量/参数/常量池数量超限）
    Limit,
    /// 内部错误（构建器不变量违反，原本会 panic）
    Internal,
    /// 作用域错误（函数作用域完整性检查失败）
    Scope,
    /// 其他错误（无法归类的通用错误）
    Other,
}

impl IrErrorCategory {
    /// 转换为简短标识符（用于日志/结构化输出）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Structural => "structural",
            Self::Limit => "limit",
            Self::Internal => "internal",
            Self::Scope => "scope",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for IrErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Semantic => write!(f, "语义"),
            Self::Structural => write!(f, "结构"),
            Self::Limit => write!(f, "限制"),
            Self::Internal => write!(f, "内部"),
            Self::Scope => write!(f, "作用域"),
            Self::Other => write!(f, "其他"),
        }
    }
}

/// 错误代码 trait — 所有 IR 错误类型实现此 trait 以提供结构化诊断信息
///
/// # 契约
/// - `error_code()` 返回的代码全局唯一（`IRB`/`IRV`/`IRW` 前缀 + 3 位数字）
/// - `help()` 返回 `None` 时 Display 输出不包含 `help:` 行
/// - 所有方法必须为纯函数（无副作用，可重复调用）
///
/// # 实现范围
/// - `IrBuildError`: IRB001-IRB010
/// - `IrValidationError`: IRV001-IRV011
/// - `ValidationWarning`: IRW001
pub trait IrErrorCode {
    /// 返回错误代码（如 "IRB001"），全局唯一
    fn error_code(&self) -> &'static str;

    /// 返回严重级别
    fn severity(&self) -> IrErrorSeverity;

    /// 返回错误分类
    fn category(&self) -> IrErrorCategory;

    /// 返回可选的修复建议（None 表示无建议）
    ///
    /// 返回 `String` 而非 `&str`，避免调用方需要管理字符串生命周期或
    /// 使用 `.leak()` 造成内存泄漏。由于 `help()` 仅在错误报告路径调用
    /// （编译期、频率极低），`String` 分配的开销可忽略不计。
    fn help(&self) -> Option<String>;
}

// ============================================================================
// 构建期错误（AST → IR 转换过程中产生的错误）
// ============================================================================

/// IR 构建错误
///
/// 当 AST 无法转换为合法 IR 时产生的错误。
/// 对应于编译器的语义分析阶段错误。
#[derive(Debug, Clone, PartialEq)]
pub enum IrBuildError {
    /// 引用未定义的变量
    UndefinedVariable { name: String, location: SourceLocation },

    /// break 语句在循环外使用
    BreakOutsideLoop { location: SourceLocation },

    /// continue 语句在循环外使用
    ContinueOutsideLoop { location: SourceLocation },

    /// return 语句在函数外使用
    ReturnOutsideFunction { location: SourceLocation },

    /// 局部变量数量超过上限
    TooManyLocals { count: usize, max: usize, location: SourceLocation },

    /// 函数参数数量超过上限
    TooManyArguments { count: usize, max: usize, location: SourceLocation },

    /// 常量池溢出
    ConstantPoolOverflow { location: SourceLocation },

    /// A8: build_xxx 收到非预期 AST 表达式类型
    ///
    /// 当 AST 层传入与方法语义不符的表达式变体时触发,
    /// 替代原 `unreachable!()` panic,使错误可被上层优雅处理。
    UnexpectedExpr {
        /// 实际收到的表达式类型名(从 AST Debug 提取)
        expr_kind: String,
        /// 触发错误的 build_xxx 方法名(静态字符串)
        context: &'static str,
        /// 源码位置
        location: SourceLocation,
    },

    /// 内部错误 — IR 构建器不变量违反（替代原 `panic!`）
    ///
    /// 当构建器内部状态一致性被破坏时返回，例如：
    /// - `current_function_id` 越界（作用域管理 bug）
    /// - `current_block_id` 越界（块管理 bug）
    ///
    /// 这类错误原本以 `panic!` 形式中断进程，现改为 `Result::Err` 以便上层优雅处理。
    /// **约定**：返回此错误后，构建器视为已损坏，不应继续使用。
    InternalError {
        /// 什么出错了（如 "current_function_id out of range"）
        what: String,
        /// 上下文信息（如 "fn_id=5, functions.len()=3"）
        context: String,
        /// 源码位置（best-effort，可能为 default）
        location: SourceLocation,
        /// 修复提示（指向可能的根因，如 "Check build_closure_expr scope management"）
        hint: String,
    },

    /// 通用错误（带自定义消息）
    Error { message: String, location: SourceLocation },
}

impl IrBuildError {
    /// 返回单行格式的错误字符串（与旧 Display 实现完全一致）
    ///
    /// 用于日志、单行诊断输出、紧凑错误聚合等场景。
    /// 多行 rustc 风格诊断请使用 `format!("{}", self)`（即 [`fmt::Display`]）。
    ///
    /// # 契约
    /// - 返回值与 1.0 版本的 Display 实现逐字节一致
    /// - 不含 `\n`，适合单行日志
    ///
    /// # 示例
    /// ```
    /// use nuzo_core::SourceLocation;
    /// use nuzo_ir::error::IrBuildError;
    ///
    /// let err = IrBuildError::UndefinedVariable {
    ///     name: "x".to_string(),
    ///     location: SourceLocation::default(),
    /// };
    /// assert_eq!(err.to_single_line(), "Undefined variable 'x' at <unknown>:0");
    /// ```
    pub fn to_single_line(&self) -> String {
        match self {
            Self::UndefinedVariable { name, location } => {
                format!("Undefined variable '{}' at {}", name, location)
            }
            Self::BreakOutsideLoop { location } => {
                format!("'break' outside loop at {}", location)
            }
            Self::ContinueOutsideLoop { location } => {
                format!("'continue' outside loop at {}", location)
            }
            Self::ReturnOutsideFunction { location } => {
                format!("'return' outside function at {}", location)
            }
            Self::TooManyLocals { count, max, location } => {
                format!("Too many local variables ({} > {}) at {}", count, max, location)
            }
            Self::TooManyArguments { count, max, location } => {
                format!("Too many arguments ({} > {}) at {}", count, max, location)
            }
            Self::ConstantPoolOverflow { location } => {
                format!("Constant pool overflow at {}", location)
            }
            Self::UnexpectedExpr { expr_kind, context, location } => {
                format!("Unexpected expression '{}' in {} at {}", expr_kind, context, location)
            }
            Self::InternalError { what, context, location, hint } => {
                format!(
                    "Internal error: {} (context: {}) at {}. hint: {}",
                    what, context, location, hint
                )
            }
            Self::Error { message, location } => {
                format!("{} at {}", message, location)
            }
        }
    }

    /// 返回多行 Display 第一行的消息部分（不含 severity/code 前缀，不含 location 后缀）
    ///
    /// 这是 [`to_single_line`](Self::to_single_line) 的"消息核心"——
    /// 剥离了 ` at {location}` 后缀，用于多行 Display 的第一行 `error[code]: {message}`。
    fn single_line_message(&self) -> String {
        match self {
            Self::UndefinedVariable { name, .. } => format!("Undefined variable '{}'", name),
            Self::BreakOutsideLoop { .. } => "'break' outside loop".to_string(),
            Self::ContinueOutsideLoop { .. } => "'continue' outside loop".to_string(),
            Self::ReturnOutsideFunction { .. } => "'return' outside function".to_string(),
            Self::TooManyLocals { count, max, .. } => {
                format!("Too many local variables ({} > {})", count, max)
            }
            Self::TooManyArguments { count, max, .. } => {
                format!("Too many arguments ({} > {})", count, max)
            }
            Self::ConstantPoolOverflow { .. } => "Constant pool overflow".to_string(),
            Self::UnexpectedExpr { expr_kind, context, .. } => {
                format!("Unexpected expression '{}' in {}", expr_kind, context)
            }
            Self::InternalError { what, .. } => format!("Internal error: {}", what),
            Self::Error { message, .. } => message.clone(),
        }
    }

    /// 返回错误关联的源码位置
    ///
    /// 所有 `IrBuildError` 变体均携带 `SourceLocation`，此方法返回其引用。
    fn location(&self) -> &SourceLocation {
        match self {
            Self::UndefinedVariable { location, .. } => location,
            Self::BreakOutsideLoop { location } => location,
            Self::ContinueOutsideLoop { location } => location,
            Self::ReturnOutsideFunction { location } => location,
            Self::TooManyLocals { location, .. } => location,
            Self::TooManyArguments { location, .. } => location,
            Self::ConstantPoolOverflow { location } => location,
            Self::UnexpectedExpr { location, .. } => location,
            Self::InternalError { location, .. } => location,
            Self::Error { location, .. } => location,
        }
    }
}

impl fmt::Display for IrBuildError {
    /// 多行 rustc 风格诊断输出
    ///
    /// # 格式
    /// ```text
    /// error[IRB001]: Undefined variable 'x'
    ///   --> <unknown>:0
    ///   help: 变量 'x' 未在当前作用域定义。检查拼写...
    /// ```
    ///
    /// 对于 [`IrBuildError::InternalError`] 变体，额外输出 `context:` 与 `hint:` 行：
    /// ```text
    /// error[IRB009]: Internal error: current_function_id out of range
    ///   --> <unknown>:0
    ///   context: fn_id=5, functions.len()=3
    ///   hint: Check build_closure_expr scope management
    /// ```
    ///
    /// 单行格式请使用 [`IrBuildError::to_single_line`]。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 第一行：severity[code]: message
        let code = self.error_code();
        let severity = self.severity().as_str();
        let msg = self.single_line_message();
        let mut lines: Vec<String> = Vec::with_capacity(4);
        lines.push(format!("{}[{}]: {}", severity, code, msg));

        // 第二行：--> location
        lines.push(format!("  --> {}", self.location()));

        // InternalError 特有行：context + hint
        if let Self::InternalError { context, hint, .. } = self {
            lines.push(format!("  context: {}", context));
            lines.push(format!("  hint: {}", hint));
        }

        // help 行（InternalError 的 help() 返回 hint 字段，会与 hint 行重复，故跳过）
        if !matches!(self, Self::InternalError { .. })
            && let Some(help) = self.help()
        {
            lines.push(format!("  help: {}", help));
        }

        // 行间用 \n 分隔，末尾无多余换行
        write!(f, "{}", lines.join("\n"))
    }
}

// ============================================================================
// IrErrorCode 实现：IrBuildError
// ============================================================================
//
// 错误代码映射 (IRB001-IRB010):
//   IRB001 UndefinedVariable      - Semantic - 变量未定义
//   IRB002 BreakOutsideLoop       - Semantic - break 位置错误
//   IRB003 ContinueOutsideLoop    - Semantic - continue 位置错误
//   IRB004 ReturnOutsideFunction  - Semantic - return 位置错误
//   IRB005 TooManyLocals          - Limit    - 局部变量超限
//   IRB006 TooManyArguments       - Limit    - 参数超限
//   IRB007 ConstantPoolOverflow   - Limit    - 常量池溢出
//   IRB008 UnexpectedExpr         - Internal - 非预期 AST 类型
//   IRB009 InternalError          - Internal - 构建器不变量违反
//   IRB010 Error                  - Semantic - 通用错误

impl IrErrorCode for IrBuildError {
    fn error_code(&self) -> &'static str {
        match self {
            Self::UndefinedVariable { .. } => "IRB001",
            Self::BreakOutsideLoop { .. } => "IRB002",
            Self::ContinueOutsideLoop { .. } => "IRB003",
            Self::ReturnOutsideFunction { .. } => "IRB004",
            Self::TooManyLocals { .. } => "IRB005",
            Self::TooManyArguments { .. } => "IRB006",
            Self::ConstantPoolOverflow { .. } => "IRB007",
            Self::UnexpectedExpr { .. } => "IRB008",
            Self::InternalError { .. } => "IRB009",
            Self::Error { .. } => "IRB010",
        }
    }

    fn severity(&self) -> IrErrorSeverity {
        IrErrorSeverity::Error
    }

    fn category(&self) -> IrErrorCategory {
        match self {
            Self::UndefinedVariable { .. } => IrErrorCategory::Semantic,
            Self::BreakOutsideLoop { .. } => IrErrorCategory::Semantic,
            Self::ContinueOutsideLoop { .. } => IrErrorCategory::Semantic,
            Self::ReturnOutsideFunction { .. } => IrErrorCategory::Semantic,
            Self::TooManyLocals { .. } => IrErrorCategory::Limit,
            Self::TooManyArguments { .. } => IrErrorCategory::Limit,
            Self::ConstantPoolOverflow { .. } => IrErrorCategory::Limit,
            Self::UnexpectedExpr { .. } => IrErrorCategory::Internal,
            Self::InternalError { .. } => IrErrorCategory::Internal,
            Self::Error { .. } => IrErrorCategory::Semantic,
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UndefinedVariable { name, .. } => Some(format!(
                "变量 '{}' 未在当前作用域定义。检查拼写，或使用 `let {} = ...` 声明。",
                name, name
            )),
            Self::BreakOutsideLoop { .. } => {
                Some("break 语句只能在循环体内使用。检查是否遗漏了循环结构。".to_string())
            }
            Self::ContinueOutsideLoop { .. } => {
                Some("continue 语句只能在循环体内使用。检查是否遗漏了循环结构。".to_string())
            }
            Self::ReturnOutsideFunction { .. } => {
                Some("return 语句只能在函数体内使用。顶层代码不需要 return。".to_string())
            }
            Self::TooManyLocals { count, max, .. } => Some(format!(
                "局部变量数量 {} 超过上限 {}。考虑拆分函数或使用对象封装变量。",
                count, max
            )),
            Self::TooManyArguments { count, max, .. } => {
                Some(format!("函数参数数量 {} 超过上限 {}。考虑改用结构体封装参数。", count, max))
            }
            Self::ConstantPoolOverflow { .. } => {
                Some("常量池已满。考虑复用常量或拆分模块。".to_string())
            }
            Self::UnexpectedExpr { expr_kind, context, .. } => Some(format!(
                "build_xxx 方法 {} 收到非预期的 AST 类型 {}。检查 AST 构建逻辑或 dispatch 表。",
                context, expr_kind
            )),
            Self::InternalError { hint, .. } => Some(hint.clone()),
            Self::Error { .. } => None,
        }
    }
}

impl std::error::Error for IrBuildError {}

// ============================================================================
// 验证期错误（对已构建的 IR 进行合法性检查时发现的错误）
// ============================================================================

/// IR 验证错误
///
/// 当 IrModule::validate() 检测到非法 IR 结构时返回的错误。
/// 包含结构性检查（基本块终止符、ValueRef 范围）和函数作用域完整性检查。
#[derive(Debug, Clone, PartialEq)]
pub enum IrValidationError {
    // ── 现有：结构性检查 ──
    /// 使用了未定义的 ValueRef（在使用前未赋值）
    UndefinedValueRef { value_ref: u32, context: String },

    /// 基本块缺少终止指令
    BlockMissingTerminator { block_id: u32 },

    /// 孤立的基本块（无法从入口到达，也无法到达出口）
    DisconnectedBlock { block_id: u32 },

    /// 基本块 ID 不存在
    InvalidBlockId { block_id: u32, function_id: u32 },

    /// 函数引用了不存在的 IrFunctionId
    UndefinedFunction { func_id: u32 },

    /// 通用验证错误
    Generic { message: String },

    // ── 新增：函数作用域完整性检查 ──
    /// 主函数 (fn0) 为空但存在子函数，表明顶层语句未被发射到 main
    ///
    /// 这是 `build_closure_expr` 遗漏 `current_function_id`/`current_block_id`
    /// 保存/恢复的典型症状。当闭包编译后未恢复外层上下文时，
    /// 后续的顶层代码会被错误地发射到闭包函数中，导致 main 为空。
    MainFunctionEmpty { function_index: usize, hint: String },

    /// GetCapture/SetCapture 指令出现在主函数 (fn0) 中
    ///
    /// Capture 指令只应出现在闭包函数体中（fn1+），
    /// 因为只有闭包才有捕获环境。main 函数不应包含此类指令。
    CaptureInMainFunction { instruction_index: usize, hint: String },

    /// LoadArg 指令出现在主函数 (fn0) 中
    ///
    /// Argument 加载只应出现在非 main 的函数中，
    /// 因为 main 函数不接受参数。
    ArgumentInMainFunction { instruction_index: usize, hint: String },

    /// Closure 指令引用了超出模块函数列表范围的函数索引
    ///
    /// 与现有 `UndefinedFunction` 不同，此变体携带更多上下文信息
    /// （指令位置、总函数数），便于定位问题根源。
    InvalidClosureReference {
        instruction_index: usize,
        referenced_function: u32,
        total_functions: usize,
        hint: String,
    },

    /// 基本块中的指令引用了超出该函数指令向量范围的索引
    ///
    /// 这通常表明基本块与函数之间的归属关系被破坏，
    /// 可能是跨函数操作基本块时的索引计算错误。
    InvalidBlockInstructionRef {
        function_index: usize,
        block_index: usize,
        instruction_index: u32,
        total_instructions: usize,
        hint: String,
    },
}

impl IrValidationError {
    /// 返回单行格式的错误字符串（与旧 Display 实现完全一致）
    ///
    /// 用于日志、单行诊断输出、紧凑错误聚合等场景。
    /// 多行 rustc 风格诊断请使用 `format!("{}", self)`（即 [`fmt::Display`]）。
    ///
    /// # 契约
    /// - 返回值与 1.0 版本的 Display 实现逐字节一致
    /// - 不含 `\n`，适合单行日志
    /// - `IrValidationError` 变体本身不携带 `SourceLocation`，
    ///   单行格式不含位置后缀；位置信息在多行 Display 中以 `<unknown>:0` 默认值呈现
    ///
    /// # 示例
    /// ```
    /// use nuzo_ir::error::IrValidationError;
    ///
    /// let err = IrValidationError::UndefinedValueRef {
    ///     value_ref: 42,
    ///     context: "instruction Add bb0:5".to_string(),
    /// };
    /// assert_eq!(
    ///     err.to_single_line(),
    ///     "Undefined value v42 in instruction Add bb0:5"
    /// );
    /// ```
    pub fn to_single_line(&self) -> String {
        match self {
            // ── 现有变体 ──
            Self::UndefinedValueRef { value_ref, context } => {
                format!("Undefined value v{} in {}", value_ref, context)
            }
            Self::BlockMissingTerminator { block_id } => {
                format!("Block bb{} missing terminator instruction", block_id)
            }
            Self::DisconnectedBlock { block_id } => {
                format!("Block bb{} is disconnected from CFG", block_id)
            }
            Self::InvalidBlockId { block_id, function_id } => {
                format!("Invalid block id {} in function {}", block_id, function_id)
            }
            Self::UndefinedFunction { func_id } => {
                format!("Undefined function reference fn{}", func_id)
            }
            Self::Generic { message } => message.clone(),

            // ── 新增：函数作用域完整性检查 ──
            Self::MainFunctionEmpty { function_index, hint } => {
                format!(
                    "Main function (fn{}) is empty but sub-functions exist. \
                     Top-level statements were not emitted to main function. [{}]",
                    function_index, hint
                )
            }
            Self::CaptureInMainFunction { instruction_index, hint } => {
                format!(
                    "GetCapture/SetCapture at instruction #{} in main function (fn0). \
                     Capture instructions should only appear in closure functions. [{}]",
                    instruction_index, hint
                )
            }
            Self::ArgumentInMainFunction { instruction_index, hint } => {
                format!(
                    "LoadArg at instruction #{} in main function (fn0). \
                     Argument instructions should only appear in non-main functions. [{}]",
                    instruction_index, hint
                )
            }
            Self::InvalidClosureReference {
                instruction_index,
                referenced_function,
                total_functions,
                hint,
            } => {
                format!(
                    "Closure at instruction #{} references fn{} (out of range: 0..{}). [{}]",
                    instruction_index, referenced_function, total_functions, hint
                )
            }
            Self::InvalidBlockInstructionRef {
                function_index,
                block_index,
                instruction_index,
                total_instructions,
                hint,
            } => {
                format!(
                    "Block bb{} in fn{} references instruction #{} (out of range: 0..{}). [{}]",
                    block_index, function_index, instruction_index, total_instructions, hint
                )
            }
        }
    }
}

impl fmt::Display for IrValidationError {
    /// 多行 rustc 风格诊断输出
    ///
    /// # 格式
    /// ```text
    /// error[IRV001]: Undefined value v42 in instruction Add bb0:5
    ///   --> <unknown>:0
    ///   help: ValueRef v42 在 instruction Add bb0:5 中被使用但未定义...
    /// ```
    ///
    /// `IrValidationError` 变体不携带 `SourceLocation`，位置行使用默认值 `<unknown>:0`。
    /// 单行格式请使用 [`IrValidationError::to_single_line`]。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 第一行：severity[code]: message
        // IrValidationError 的单行消息本身就是完整消息（无 location 后缀），直接复用
        let code = self.error_code();
        let severity = self.severity().as_str();
        let msg = self.to_single_line();
        let mut lines: Vec<String> = Vec::with_capacity(3);
        lines.push(format!("{}[{}]: {}", severity, code, msg));

        // 第二行：--> location（IrValidationError 无 location 字段，使用默认值）
        lines.push(format!("  --> {}", SourceLocation::default()));

        // help 行（如果有）
        if let Some(help) = self.help() {
            lines.push(format!("  help: {}", help));
        }

        // 行间用 \n 分隔，末尾无多余换行
        write!(f, "{}", lines.join("\n"))
    }
}

impl std::error::Error for IrValidationError {}

// ============================================================================
// IrErrorCode 实现：IrValidationError
// ============================================================================
//
// 错误代码映射 (IRV001-IRV011):
//   IRV001 UndefinedValueRef            - Structural - 未定义的 ValueRef
//   IRV002 BlockMissingTerminator       - Structural - 基本块缺少终止指令
//   IRV003 DisconnectedBlock            - Structural - 孤立基本块
//   IRV004 InvalidBlockId               - Structural - 无效块 ID
//   IRV005 UndefinedFunction            - Structural - 未定义函数引用
//   IRV006 Generic                      - Other      - 通用验证错误
//   IRV007 MainFunctionEmpty            - Scope      - main 函数为空
//   IRV008 CaptureInMainFunction        - Scope      - main 中出现 Capture 指令
//   IRV009 ArgumentInMainFunction       - Scope      - main 中出现 LoadArg 指令
//   IRV010 InvalidClosureReference      - Scope      - Closure 引用越界
//   IRV011 InvalidBlockInstructionRef   - Scope      - 块指令引用越界

impl IrErrorCode for IrValidationError {
    fn error_code(&self) -> &'static str {
        match self {
            Self::UndefinedValueRef { .. } => "IRV001",
            Self::BlockMissingTerminator { .. } => "IRV002",
            Self::DisconnectedBlock { .. } => "IRV003",
            Self::InvalidBlockId { .. } => "IRV004",
            Self::UndefinedFunction { .. } => "IRV005",
            Self::Generic { .. } => "IRV006",
            Self::MainFunctionEmpty { .. } => "IRV007",
            Self::CaptureInMainFunction { .. } => "IRV008",
            Self::ArgumentInMainFunction { .. } => "IRV009",
            Self::InvalidClosureReference { .. } => "IRV010",
            Self::InvalidBlockInstructionRef { .. } => "IRV011",
        }
    }

    fn severity(&self) -> IrErrorSeverity {
        // 所有验证期错误均为 Error 级别
        IrErrorSeverity::Error
    }

    fn category(&self) -> IrErrorCategory {
        match self {
            Self::UndefinedValueRef { .. } => IrErrorCategory::Structural,
            Self::BlockMissingTerminator { .. } => IrErrorCategory::Structural,
            Self::DisconnectedBlock { .. } => IrErrorCategory::Structural,
            Self::InvalidBlockId { .. } => IrErrorCategory::Structural,
            Self::UndefinedFunction { .. } => IrErrorCategory::Structural,
            Self::Generic { .. } => IrErrorCategory::Other,
            Self::MainFunctionEmpty { .. } => IrErrorCategory::Scope,
            Self::CaptureInMainFunction { .. } => IrErrorCategory::Scope,
            Self::ArgumentInMainFunction { .. } => IrErrorCategory::Scope,
            Self::InvalidClosureReference { .. } => IrErrorCategory::Scope,
            Self::InvalidBlockInstructionRef { .. } => IrErrorCategory::Scope,
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UndefinedValueRef { value_ref, context } => Some(format!(
                "ValueRef v{} 在 {} 中被使用但未定义。检查 IR 构建器是否遗漏了赋值指令。",
                value_ref, context
            )),
            Self::BlockMissingTerminator { block_id } => Some(format!(
                "基本块 bb{} 缺少终止指令(Jump/Return/JumpIf)。检查控制流构建逻辑是否遗漏了终止符发射。",
                block_id
            )),
            Self::DisconnectedBlock { block_id } => Some(format!(
                "基本块 bb{} 无法从入口到达。检查跳转目标是否遗漏了对此块的引用。",
                block_id
            )),
            Self::InvalidBlockId { block_id, function_id } => Some(format!(
                "函数 fn{} 中引用了不存在的块 bb{}。检查块 ID 分配与跳转目标的一致性。",
                function_id, block_id
            )),
            Self::UndefinedFunction { func_id } => Some(format!(
                "引用了不存在的函数 fn{}。检查函数注册顺序与 Closure 指令的目标索引。",
                func_id
            )),
            Self::Generic { message } => Some(message.clone()),
            Self::MainFunctionEmpty { hint, .. } => Some(hint.clone()),
            Self::CaptureInMainFunction { hint, .. } => Some(hint.clone()),
            Self::ArgumentInMainFunction { hint, .. } => Some(hint.clone()),
            Self::InvalidClosureReference { hint, .. } => Some(hint.clone()),
            Self::InvalidBlockInstructionRef { hint, .. } => Some(hint.clone()),
        }
    }
}

// ============================================================================
// 验证警告（非致命性问题，可能指示潜在的构建器 bug）
// ============================================================================

/// IR 验证警告
///
/// 不会阻止验证通过，但提示可能存在的作用域管理问题。
/// 用于标记"可疑但不一定是错误"的 IR 模式。
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationWarning {
    /// 非主函数中出现 Closure/Call 指令
    ///
    /// 这不一定是错误（高阶函数场景下合法），
    /// 但如果出现在不该出现的位置，可能是作用域 bug 的信号。
    SuspiciousInstructionInFunction {
        function_index: usize,
        instruction_index: usize,
        opcode: String,
        hint: String,
    },
}

impl ValidationWarning {
    /// 返回单行格式的警告字符串（与旧 Display 实现完全一致）
    ///
    /// 用于日志、单行诊断输出、紧凑错误聚合等场景。
    /// 多行 rustc 风格诊断请使用 `format!("{}", self)`（即 [`fmt::Display`]）。
    ///
    /// # 契约
    /// - 返回值与 1.0 版本的 Display 实现逐字节一致
    /// - 不含 `\n`，适合单行日志
    /// - 单行格式保留 `Warning:` 前缀与 `[hint]` 后缀以维持向后兼容
    ///
    /// # 示例
    /// ```
    /// use nuzo_ir::error::ValidationWarning;
    ///
    /// let warn = ValidationWarning::SuspiciousInstructionInFunction {
    ///     function_index: 2,
    ///     instruction_index: 7,
    ///     opcode: "Closure".to_string(),
    ///     hint: "Check scope restore".to_string(),
    /// };
    /// assert!(warn.to_single_line().contains("Warning"));
    /// assert!(warn.to_single_line().contains("Closure"));
    /// ```
    pub fn to_single_line(&self) -> String {
        match self {
            Self::SuspiciousInstructionInFunction {
                function_index,
                instruction_index,
                opcode,
                hint,
            } => {
                format!(
                    "Warning: '{}' at instruction #{} in fn{} may indicate scope error. [{}]",
                    opcode, instruction_index, function_index, hint
                )
            }
        }
    }

    /// 返回多行 Display 第一行的消息部分（不含 severity/code 前缀，不含 `Warning:` 前缀，不含 `[hint]` 后缀）
    ///
    /// 这是 [`to_single_line`](Self::to_single_line) 的"消息核心"——
    /// 剥离了 `Warning: ` 前缀与 ` [{hint}]` 后缀，
    /// 用于多行 Display 的第一行 `warning[code]: {message}`。
    fn single_line_message(&self) -> String {
        match self {
            Self::SuspiciousInstructionInFunction {
                function_index,
                instruction_index,
                opcode,
                ..
            } => {
                format!(
                    "'{}' at instruction #{} in fn{} may indicate scope error",
                    opcode, instruction_index, function_index
                )
            }
        }
    }
}

impl fmt::Display for ValidationWarning {
    /// 多行 rustc 风格诊断输出
    ///
    /// # 格式
    /// ```text
    /// warning[IRW001]: 'Closure' at instruction #7 in fn2 may indicate scope error
    ///   --> <unknown>:0
    ///   help: Check scope restore
    /// ```
    ///
    /// `ValidationWarning` 变体不携带 `SourceLocation`，位置行使用默认值 `<unknown>:0`。
    /// 单行格式请使用 [`ValidationWarning::to_single_line`]。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 第一行：severity[code]: message
        let code = self.error_code();
        let severity = self.severity().as_str();
        let msg = self.single_line_message();
        let mut lines: Vec<String> = Vec::with_capacity(3);
        lines.push(format!("{}[{}]: {}", severity, code, msg));

        // 第二行：--> location（ValidationWarning 无 location 字段，使用默认值）
        lines.push(format!("  --> {}", SourceLocation::default()));

        // help 行（如果有）
        if let Some(help) = self.help() {
            lines.push(format!("  help: {}", help));
        }

        // 行间用 \n 分隔，末尾无多余换行
        write!(f, "{}", lines.join("\n"))
    }
}

// ============================================================================
// IrErrorCode 实现：ValidationWarning
// ============================================================================
//
// 错误代码映射 (IRW001):
//   IRW001 SuspiciousInstructionInFunction - Scope - 非主函数中的可疑指令

impl IrErrorCode for ValidationWarning {
    fn error_code(&self) -> &'static str {
        match self {
            Self::SuspiciousInstructionInFunction { .. } => "IRW001",
        }
    }

    fn severity(&self) -> IrErrorSeverity {
        // 警告级别（不阻止验证通过）
        IrErrorSeverity::Warning
    }

    fn category(&self) -> IrErrorCategory {
        match self {
            Self::SuspiciousInstructionInFunction { .. } => IrErrorCategory::Scope,
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::SuspiciousInstructionInFunction { hint, .. } => Some(hint.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undefined_variable_display() {
        let loc = SourceLocation::default();
        let err = IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() };
        assert_eq!(err.to_single_line(), "Undefined variable 'x' at <unknown>:0");
    }

    #[test]
    fn break_outside_loop_display() {
        let loc = SourceLocation::default();
        let err = IrBuildError::BreakOutsideLoop { location: loc };
        assert!(err.to_single_line().contains("'break' outside loop"));
    }

    #[test]
    fn too_many_locals_display() {
        let loc = SourceLocation::default();
        let err = IrBuildError::TooManyLocals { count: 300, max: 256, location: loc };
        assert!(err.to_single_line().contains("300 > 256"));
    }

    #[test]
    fn undefined_value_ref_display() {
        let err = IrValidationError::UndefinedValueRef {
            value_ref: 42,
            context: "instruction Add bb0:5".to_string(),
        };
        assert_eq!(err.to_single_line(), "Undefined value v42 in instruction Add bb0:5");
    }

    #[test]
    fn block_missing_terminator_display() {
        let err = IrValidationError::BlockMissingTerminator { block_id: 3 };
        assert_eq!(err.to_single_line(), "Block bb3 missing terminator instruction");
    }

    #[test]
    fn disconnected_block_display() {
        let err = IrValidationError::DisconnectedBlock { block_id: 7 };
        assert!(err.to_single_line().contains("bb7"));
        assert!(err.to_single_line().contains("disconnected"));
    }

    #[test]
    fn invalid_block_id_display() {
        let err = IrValidationError::InvalidBlockId { block_id: 99, function_id: 2 };
        assert_eq!(err.to_single_line(), "Invalid block id 99 in function 2");
    }

    #[test]
    fn undefined_function_display() {
        let err = IrValidationError::UndefinedFunction { func_id: 5 };
        assert_eq!(err.to_single_line(), "Undefined function reference fn5");
    }

    #[test]
    fn generic_validation_error_display() {
        let err =
            IrValidationError::Generic { message: "SSA violation: v3 assigned twice".to_string() };
        assert_eq!(err.to_single_line(), "SSA violation: v3 assigned twice");
    }

    #[test]
    fn ir_build_error_is_std_error() {
        let err = IrBuildError::Error {
            message: "test".to_string(),
            location: SourceLocation::default(),
        };
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn ir_validation_error_is_std_error() {
        let err = IrValidationError::Generic { message: "test".to_string() };
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn equality_works() {
        let loc = SourceLocation::default();
        let a = IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() };
        let b = IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc };
        assert_eq!(a, b);
    }

    // ── 新增：函数作用域完整性错误 Display 测试 ──

    #[test]
    fn main_function_empty_display() {
        let err = IrValidationError::MainFunctionEmpty {
            function_index: 0,
            hint: "Check build_closure_expr scope management".to_string(),
        };
        let msg = err.to_single_line();
        assert!(msg.contains("fn0"), "Should mention fn0, got: {}", msg);
        assert!(msg.contains("empty"), "Should mention empty, got: {}", msg);
        assert!(
            msg.contains("build_closure_expr"),
            "Should hint at build_closure_expr, got: {}",
            msg
        );
    }

    #[test]
    fn capture_in_main_function_display() {
        let err = IrValidationError::CaptureInMainFunction {
            instruction_index: 3,
            hint: "Capture only in closures".to_string(),
        };
        let msg = err.to_single_line();
        assert!(msg.contains("GetCapture"), "Should mention GetCapture, got: {}", msg);
        assert!(msg.contains("instruction #3"), "Should show index 3, got: {}", msg);
        assert!(msg.contains("fn0"), "Should mention main function, got: {}", msg);
    }

    #[test]
    fn argument_in_main_function_display() {
        let err = IrValidationError::ArgumentInMainFunction {
            instruction_index: 0,
            hint: "LoadArg in main".to_string(),
        };
        let msg = err.to_single_line();
        assert!(msg.contains("LoadArg"), "Should mention LoadArg, got: {}", msg);
        assert!(msg.contains("fn0"), "Should mention main function, got: {}", msg);
    }

    #[test]
    fn invalid_closure_reference_display() {
        let err = IrValidationError::InvalidClosureReference {
            instruction_index: 5,
            referenced_function: 99,
            total_functions: 3,
            hint: "Scope bug suspected".to_string(),
        };
        let msg = err.to_single_line();
        assert!(msg.contains("fn99"), "Should reference fn99, got: {}", msg);
        assert!(msg.contains("0..3"), "Should show range 0..3, got: {}", msg);
        assert!(msg.contains("instruction #5"), "Should show instr #5, got: {}", msg);
    }

    #[test]
    fn invalid_block_instruction_ref_display() {
        let err = IrValidationError::InvalidBlockInstructionRef {
            function_index: 1,
            block_index: 2,
            instruction_index: 999,
            total_instructions: 5,
            hint: "Block ownership violation".to_string(),
        };
        let msg = err.to_single_line();
        assert!(msg.contains("bb2"), "Should mention bb2, got: {}", msg);
        assert!(msg.contains("fn1"), "Should mention fn1, got: {}", msg);
        assert!(msg.contains("0..5"), "Should show range 0..5, got: {}", msg);
    }

    // ── ValidationWarning 测试 ──

    #[test]
    fn suspicious_instruction_warning_display() {
        let warn = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 2,
            instruction_index: 7,
            opcode: "Closure".to_string(),
            hint: "Check scope restore".to_string(),
        };
        let msg = warn.to_single_line();
        assert!(msg.contains("Warning"), "Should start with Warning, got: {}", msg);
        assert!(msg.contains("Closure"), "Should mention opcode, got: {}", msg);
        assert!(msg.contains("fn2"), "Should mention fn2, got: {}", msg);
        assert!(msg.contains("#7"), "Should show index 7, got: {}", msg);
    }

    #[test]
    fn validation_warning_equality() {
        let a = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 1,
            instruction_index: 0,
            opcode: "Call".to_string(),
            hint: "test".to_string(),
        };
        let b = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 1,
            instruction_index: 0,
            opcode: "Call".to_string(),
            hint: "test".to_string(),
        };
        assert_eq!(a, b);
    }

    // ── 新增：多行 rustc 风格 Display 测试 ──
    //
    // 验证 Display 输出符合多行诊断格式：
    //   - 第一行：severity[code]: message
    //   - 第二行：  --> location
    //   - 后续行：  context: / hint: / help:（视变体而定）

    #[test]
    fn ir_build_error_multiline_display() {
        let err = IrBuildError::UndefinedVariable {
            name: "x".to_string(),
            location: SourceLocation::default(),
        };
        let s = format!("{}", err);
        assert!(s.contains("error[IRB001]:"), "Should contain error code, got: {}", s);
        assert!(s.contains("Undefined variable 'x'"), "Should contain message, got: {}", s);
        assert!(s.contains("-->"), "Should contain location marker, got: {}", s);
        assert!(s.contains("help:"), "Should contain help line, got: {}", s);
    }

    #[test]
    fn internal_error_multiline_display() {
        let err = IrBuildError::InternalError {
            what: "current_function_id out of range".to_string(),
            context: "fn_id=5, functions.len()=3".to_string(),
            location: SourceLocation::default(),
            hint: "Check scope management".to_string(),
        };
        let s = format!("{}", err);
        assert!(s.contains("error[IRB009]:"), "got: {}", s);
        assert!(s.contains("context:"), "Should contain context line, got: {}", s);
        assert!(s.contains("hint:"), "Should contain hint line, got: {}", s);
    }

    #[test]
    fn ir_validation_error_multiline_display() {
        let err = IrValidationError::UndefinedValueRef {
            value_ref: 42,
            context: "instruction Add bb0:5".to_string(),
        };
        let s = format!("{}", err);
        assert!(s.contains("error[IRV001]:"), "got: {}", s);
        assert!(s.contains("v42"), "Should contain value ref, got: {}", s);
    }

    #[test]
    fn validation_warning_multiline_display() {
        let warn = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 2,
            instruction_index: 7,
            opcode: "Closure".to_string(),
            hint: "Check scope restore".to_string(),
        };
        let s = format!("{}", warn);
        assert!(s.contains("warning[IRW001]:"), "got: {}", s);
        assert!(s.contains("Closure"), "Should contain opcode, got: {}", s);
    }

    // ── T8 新增：错误代码体系完整性测试 ──

    #[test]
    fn test_error_codes_unique() {
        // 收集所有 IrBuildError 变体的 error_code
        let loc = SourceLocation::default();
        let build_errors: Vec<IrBuildError> = vec![
            IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() },
            IrBuildError::BreakOutsideLoop { location: loc.clone() },
            IrBuildError::ContinueOutsideLoop { location: loc.clone() },
            IrBuildError::ReturnOutsideFunction { location: loc.clone() },
            IrBuildError::TooManyLocals { count: 1, max: 1, location: loc.clone() },
            IrBuildError::TooManyArguments { count: 1, max: 1, location: loc.clone() },
            IrBuildError::ConstantPoolOverflow { location: loc.clone() },
            IrBuildError::UnexpectedExpr {
                expr_kind: "X".to_string(),
                context: "ctx",
                location: loc.clone(),
            },
            IrBuildError::InternalError {
                what: "w".to_string(),
                context: "c".to_string(),
                location: loc.clone(),
                hint: "h".to_string(),
            },
            IrBuildError::Error { message: "m".to_string(), location: loc.clone() },
        ];
        let build_codes: Vec<&str> = build_errors.iter().map(|e| e.error_code()).collect();

        // 收集所有 IrValidationError 变体的 error_code
        let validation_errors: Vec<IrValidationError> = vec![
            IrValidationError::UndefinedValueRef { value_ref: 0, context: "c".to_string() },
            IrValidationError::BlockMissingTerminator { block_id: 0 },
            IrValidationError::DisconnectedBlock { block_id: 0 },
            IrValidationError::InvalidBlockId { block_id: 0, function_id: 0 },
            IrValidationError::UndefinedFunction { func_id: 0 },
            IrValidationError::Generic { message: "m".to_string() },
            IrValidationError::MainFunctionEmpty { function_index: 0, hint: "h".to_string() },
            IrValidationError::CaptureInMainFunction {
                instruction_index: 0,
                hint: "h".to_string(),
            },
            IrValidationError::ArgumentInMainFunction {
                instruction_index: 0,
                hint: "h".to_string(),
            },
            IrValidationError::InvalidClosureReference {
                instruction_index: 0,
                referenced_function: 0,
                total_functions: 0,
                hint: "h".to_string(),
            },
            IrValidationError::InvalidBlockInstructionRef {
                function_index: 0,
                block_index: 0,
                instruction_index: 0,
                total_instructions: 0,
                hint: "h".to_string(),
            },
        ];
        let validation_codes: Vec<&str> =
            validation_errors.iter().map(|e| e.error_code()).collect();

        // 收集 ValidationWarning 的 error_code
        let warnings: Vec<ValidationWarning> =
            vec![ValidationWarning::SuspiciousInstructionInFunction {
                function_index: 0,
                instruction_index: 0,
                opcode: "op".to_string(),
                hint: "h".to_string(),
            }];
        let warning_codes: Vec<&str> = warnings.iter().map(|w| w.error_code()).collect();

        // 合并所有 code，验证全局唯一
        let mut all_codes: Vec<&str> = Vec::new();
        all_codes.extend(build_codes.iter().copied());
        all_codes.extend(validation_codes.iter().copied());
        all_codes.extend(warning_codes.iter().copied());

        let unique_count = all_codes.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(
            unique_count,
            all_codes.len(),
            "error_codes not unique: total={}, unique={}",
            all_codes.len(),
            unique_count
        );
    }

    #[test]
    fn test_error_code_format() {
        let loc = SourceLocation::default();
        let build_err =
            IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() };
        let validation_err =
            IrValidationError::UndefinedValueRef { value_ref: 0, context: "c".to_string() };
        let warning = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 0,
            instruction_index: 0,
            opcode: "op".to_string(),
            hint: "h".to_string(),
        };

        let codes: Vec<&str> =
            vec![build_err.error_code(), validation_err.error_code(), warning.error_code()];
        for code in codes {
            assert!(
                code.starts_with("IRB") || code.starts_with("IRV") || code.starts_with("IRW"),
                "Invalid code prefix: {}",
                code
            );
            assert_eq!(code.len(), 6, "Code must be 6 chars (prefix+3 digits): {}", code);
            let digits = &code[3..];
            assert!(
                digits.chars().all(|c| c.is_ascii_digit()),
                "Code must end with 3 digits: {}",
                code
            );
        }
    }

    #[test]
    fn test_irb_codes_complete() {
        let loc = SourceLocation::default();
        let expected_codes = [
            "IRB001", "IRB002", "IRB003", "IRB004", "IRB005", "IRB006", "IRB007", "IRB008",
            "IRB009", "IRB010",
        ];
        let errors: Vec<IrBuildError> = vec![
            IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() },
            IrBuildError::BreakOutsideLoop { location: loc.clone() },
            IrBuildError::ContinueOutsideLoop { location: loc.clone() },
            IrBuildError::ReturnOutsideFunction { location: loc.clone() },
            IrBuildError::TooManyLocals { count: 1, max: 1, location: loc.clone() },
            IrBuildError::TooManyArguments { count: 1, max: 1, location: loc.clone() },
            IrBuildError::ConstantPoolOverflow { location: loc.clone() },
            IrBuildError::UnexpectedExpr {
                expr_kind: "X".to_string(),
                context: "ctx",
                location: loc.clone(),
            },
            IrBuildError::InternalError {
                what: "w".to_string(),
                context: "c".to_string(),
                location: loc.clone(),
                hint: "h".to_string(),
            },
            IrBuildError::Error { message: "m".to_string(), location: loc.clone() },
        ];
        let actual_codes: std::collections::HashSet<&str> =
            errors.iter().map(|e| e.error_code()).collect();
        for expected in &expected_codes {
            assert!(actual_codes.contains(*expected), "Missing error code: {}", expected);
        }
    }

    #[test]
    fn test_irv_codes_complete() {
        let expected_codes = [
            "IRV001", "IRV002", "IRV003", "IRV004", "IRV005", "IRV006", "IRV007", "IRV008",
            "IRV009", "IRV010", "IRV011",
        ];
        let errors: Vec<IrValidationError> = vec![
            IrValidationError::UndefinedValueRef { value_ref: 0, context: "c".to_string() },
            IrValidationError::BlockMissingTerminator { block_id: 0 },
            IrValidationError::DisconnectedBlock { block_id: 0 },
            IrValidationError::InvalidBlockId { block_id: 0, function_id: 0 },
            IrValidationError::UndefinedFunction { func_id: 0 },
            IrValidationError::Generic { message: "m".to_string() },
            IrValidationError::MainFunctionEmpty { function_index: 0, hint: "h".to_string() },
            IrValidationError::CaptureInMainFunction {
                instruction_index: 0,
                hint: "h".to_string(),
            },
            IrValidationError::ArgumentInMainFunction {
                instruction_index: 0,
                hint: "h".to_string(),
            },
            IrValidationError::InvalidClosureReference {
                instruction_index: 0,
                referenced_function: 0,
                total_functions: 0,
                hint: "h".to_string(),
            },
            IrValidationError::InvalidBlockInstructionRef {
                function_index: 0,
                block_index: 0,
                instruction_index: 0,
                total_instructions: 0,
                hint: "h".to_string(),
            },
        ];
        let actual_codes: std::collections::HashSet<&str> =
            errors.iter().map(|e| e.error_code()).collect();
        for expected in &expected_codes {
            assert!(actual_codes.contains(*expected), "Missing error code: {}", expected);
        }
    }

    #[test]
    fn test_irw_codes_complete() {
        let warn = ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 0,
            instruction_index: 0,
            opcode: "op".to_string(),
            hint: "h".to_string(),
        };
        assert_eq!(warn.error_code(), "IRW001");
    }

    #[test]
    fn test_internal_error_basic() {
        let err = IrBuildError::InternalError {
            what: "current_function_id out of range".to_string(),
            context: "fn_id=5, functions.len()=3".to_string(),
            location: SourceLocation::default(),
            hint: "Check scope management".to_string(),
        };
        assert_eq!(err.error_code(), "IRB009");
        assert_eq!(err.severity(), IrErrorSeverity::Error);
        assert_eq!(err.category(), IrErrorCategory::Internal);
        assert_eq!(err.help(), Some("Check scope management".to_string()));

        let s = format!("{}", err);
        assert!(s.contains("error[IRB009]"), "got: {}", s);
        assert!(s.contains("current_function_id"), "got: {}", s);
        assert!(s.contains("context:"), "got: {}", s);
        assert!(s.contains("hint:"), "got: {}", s);
    }

    #[test]
    fn test_ir_error_severity_display() {
        assert_eq!(format!("{}", IrErrorSeverity::Error), "错误");
        assert_eq!(format!("{}", IrErrorSeverity::Warning), "警告");
        assert_eq!(format!("{}", IrErrorSeverity::Info), "信息");
        assert_eq!(IrErrorSeverity::Error.as_str(), "error");
        assert_eq!(IrErrorSeverity::Warning.as_str(), "warning");
        assert_eq!(IrErrorSeverity::Info.as_str(), "info");
    }

    #[test]
    fn test_ir_error_category_display() {
        assert_eq!(format!("{}", IrErrorCategory::Semantic), "语义");
        assert_eq!(format!("{}", IrErrorCategory::Structural), "结构");
        assert_eq!(format!("{}", IrErrorCategory::Limit), "限制");
        assert_eq!(format!("{}", IrErrorCategory::Internal), "内部");
        assert_eq!(format!("{}", IrErrorCategory::Scope), "作用域");
        assert_eq!(format!("{}", IrErrorCategory::Other), "其他");
        assert_eq!(IrErrorCategory::Semantic.as_str(), "semantic");
        assert_eq!(IrErrorCategory::Other.as_str(), "other");
    }
}

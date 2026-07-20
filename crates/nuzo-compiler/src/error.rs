//! # 编译错误类型（Compile Error Types）
//!
//! 本模块定义了编译阶段可能遇到的所有错误情况。
//! 每个错误变体都包含位置信息（行号），便于生成友好的错误消息。
//!
//! # 错误分类
//!
//! 1. **语义错误（Semantic Errors）**：
//!    - `UnexpectedExpression`：表达式类型不匹配
//!    - `UndefinedVariable`：引用未定义的变量
//!    - `TooManyLocals`：局部变量数量超限
//!    - `DivisionByZero`：常量表达式除零
//!
//! 2. **控制流错误（Control Flow Errors）**：
//!    - `BreakOutsideLoop`：break 语句在循环外使用
//!    - `ContinueOutsideLoop`：continue 语句在循环外使用
//!    - `ReturnOutsideFunction`：return 语句在函数外使用
//!
//! 3. **参数验证错误（Argument Validation Errors）**：
//!    - `InvalidArgumentCount`：函数调用参数数量不匹配
//!
//! 4. **资源限制错误（Resource Limit Errors）**：
//!    - `JumpOffsetOverflow`：跳转偏移量超出 i16 范围
//!    - `ConstantPoolOverflow`：常量池索引超出 u16 范围
//!    - `ArrayElementOverflow`：数组元素数量超出 u16 范围
//!
//! 5. **通用错误（Generic Errors）**：
//!    - `Error`：带自定义消息的通用错误
//!    - `ParseError`：包装自解析器的语法错误

use nuzo_core::MAX_FUNCTION_LOCALS;
use nuzo_core::SourceLocation;
use nuzo_core::error::ErrorCode;
use std::fmt;

/// 编译错误的综合枚举类型
///
/// 涵盖了从词法分析到代码生成的所有可能的编译失败场景。
/// 每个变体都携带足够的上下文信息用于生成可读的错误消息。
#[derive(Debug, Clone, PartialEq, nuzo_proc::MatchSync)]
pub enum CompileError {
    /// 表达式类型不匹配
    ///
    /// 当编译器期望某种类型的表达式但得到另一种类型时触发。
    /// 例如：在需要数值的地方使用了字符串。
    UnexpectedExpression {
        /// 期望的表达式类型描述
        expected: String,
        /// 实际得到的表达式类型描述
        got: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 引用未定义的变量
    ///
    /// 当标识符在当前作用域链中无法解析时触发。
    /// 编译器会先检查局部变量，再检查闭包捕获变量，最后检查全局环境。
    UndefinedVariable {
        /// 未定义的变量名称
        name: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 局部变量数量超过上限
    ///
    /// 单个函数内的局部变量数量不能超过 `MAX_FUNCTION_LOCALS`（当前为 4096）。
    /// 此限制源于字节码编码中寄存器索引使用 u16 表示。
    TooManyLocals {
        /// 当前局部变量数量
        count: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 常量表达式除零
    ///
    /// 当编译器在编译期求值常量表达式时检测到除零操作。
    /// 注意：非常量表达式的除零会在运行时由虚拟机处理。
    DivisionByZero {
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// break 语句出现在循环外部
    ///
    /// break 只能在 while/for/loop 循环体内使用。
    /// 编译器通过维护循环栈（loop_stack）来跟踪当前是否在循环内。
    BreakOutsideLoop {
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// continue 语句出现在循环外部
    ///
    /// continue 只能在 while/for/loop 循环体内使用。
    ContinueOutsideLoop {
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// return 语句出现在函数外部
    ///
    /// return 只能在函数或闭包体内使用。
    /// 顶层代码中使用 return 是语义错误。
    ReturnOutsideFunction {
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 函数调用参数数量无效
    ///
    /// 当调用函数时提供的参数数量与函数定义的参数数量（元数 arity）不匹配时触发。
    InvalidArgumentCount {
        /// 函数期望的参数数量
        expected: usize,
        /// 实际提供的参数数量
        got: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 带自定义消息的通用错误
    ///
    /// 用于不适合归入其他类别的错误情况。
    Error {
        /// 错误消息内容
        message: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 解析错误（来自 Parser 的包装）
    ///
    /// 当编译入口函数 `compile()` 接收到解析失败的 AST 时，
    /// 将 `ParseError` 包装为此变体以统一错误接口。
    ParseError {
        /// 解析器提供的错误消息
        message: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 跳转偏移量溢出
    ///
    /// 字节码中的条件跳转和无条件跳转指令使用 i16 编码偏移量。
    /// 当跳转目标距离超过 ±32767 字节时会触发此错误。
    /// 这通常意味着函数体过大，需要拆分为更小的函数。
    JumpOffsetOverflow {
        /// 计算出的跳转偏移值
        offset: i32,
        /// 跳转指令的位置（Instruction Pointer）
        from_ip: usize,
        /// 跳转目标的位置
        to_ip: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 常量池溢出
    ///
    /// Chunk 的常量池使用 u16 索引，最大支持 65535 个常量。
    /// 当程序包含过多字面量（字符串、数字等）时会触发此错误。
    ConstantPoolOverflow {
        /// 当前常量池大小
        count: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 数组字面量元素数量溢出
    ///
    /// ArrayNew 指令的元素计数操作数使用 u16 编码。
    /// 当数组字面量的元素数量超过 65535 时会触发此错误。
    ArrayElementOverflow {
        /// 数组元素数量
        count: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 控制栈下溢（循环/块退出次数过多）
    ///
    /// 当编译器尝试从控制栈弹出循环上下文但栈为空时触发。
    /// 通常表示 break/continue 在循环外使用或编译器内部逻辑错误。
    ControlStackUnderflow {
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 闭包捕获变量数量超限
    ///
    /// 单个函数的闭包捕获变量数量不能超过 u8::MAX（255），
    /// 因为 CaptureInfo.capture_index 使用 u8 编码。
    TooManyCapturedVariables {
        /// 当前捕获变量数量
        count: usize,
        /// 最大允许数量
        max: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 无效的跳转修补目标
    ///
    /// 当尝试修补跳转指令的目标偏移量时，
    /// 目标位置超出了字节码的有效范围。
    InvalidPatchTarget {
        /// 跳转指令的位置（Instruction Pointer）
        ip: usize,
        /// 字节码总长度
        code_len: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 不支持的二元运算符
    ///
    /// 当 AST 中出现了声明式映射表未覆盖的二元运算符时触发。
    /// 理论上不应发生（宏保证 exhaustive），但作为防御性检查保留。
    UnsupportedBinaryOperator {
        /// 运算符描述
        op: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 闭包捕获变量无法解析
    ///
    /// 当编译器尝试发射 Capture 指令时，变量既不是当前作用域的局部变量，
    /// 也不在父级闭包的捕获列表中。这通常表示编译器内部逻辑错误
    /// 或作用域链分析不完整。
    UncapturableVariable {
        /// 无法捕获的变量名称
        name: String,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 无效的跳转目标地址
    ///
    /// 当 patch_jump 尝试读取跳转指令位置的字节码但该位置
    /// 超出字节码有效范围时触发。
    InvalidJumpTarget {
        /// 跳转指令的位置（Instruction Pointer）
        ip: usize,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },

    /// 无效的操作码
    ///
    /// 当 patch_jump 在跳转位置解码到无效的操作码字节时触发。
    InvalidOpcode {
        /// 跳转指令的位置（Instruction Pointer）
        ip: usize,
        /// 无效的操作码字节值
        byte: u8,
        /// 出错的源代码行号
        line: usize,
        /// 出错的源代码列号
        column: usize,
    },
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::UnexpectedExpression { expected, got, line, .. } => {
                write!(f, "[line {}] expected {}, got {}", line, expected, got)
            }
            CompileError::UndefinedVariable { name, line, .. } => {
                write!(f, "[line {}] undefined variable '{}'", line, name)
            }
            CompileError::TooManyLocals { count, line, .. } => {
                write!(
                    f,
                    "[line {}] too many local variables: {} (max {})",
                    line, count, MAX_FUNCTION_LOCALS
                )
            }
            CompileError::DivisionByZero { line, .. } => {
                write!(f, "[line {}] division by zero", line)
            }
            CompileError::BreakOutsideLoop { line, .. } => {
                write!(f, "[line {}] 'break' outside of loop", line)
            }
            CompileError::ContinueOutsideLoop { line, .. } => {
                write!(f, "[line {}] 'continue' outside of loop", line)
            }
            CompileError::ReturnOutsideFunction { line, .. } => {
                write!(f, "[line {}] 'return' outside of function", line)
            }
            CompileError::InvalidArgumentCount { expected, got, line, .. } => {
                write!(f, "[line {}] expected {} arguments, got {}", line, expected, got)
            }
            CompileError::Error { message, line, .. } => {
                write!(f, "[line {}] {}", line, message)
            }
            CompileError::ParseError { message, line, column } => {
                write!(f, "[line {}] Parse error at {}:{}: {}", line, line, column, message)
            }
            CompileError::JumpOffsetOverflow { offset, from_ip, to_ip, line, .. } => {
                write!(
                    f,
                    "[line {}] jump offset {} out of range (from ip {} to ip {})",
                    line, offset, from_ip, to_ip
                )
            }
            CompileError::ConstantPoolOverflow { count, line, .. } => {
                write!(f, "[line {}] too many constants: {} (max {})", line, count, u16::MAX)
            }
            CompileError::ArrayElementOverflow { count, line, .. } => {
                write!(f, "[line {}] too many array elements: {} (max {})", line, count, u16::MAX)
            }
            CompileError::ControlStackUnderflow { line, .. } => {
                write!(f, "[line {}] control stack underflow: too many loop/block exits", line)
            }
            CompileError::TooManyCapturedVariables { count, max, line, .. } => {
                write!(f, "[line {}] too many captured variables: {} (max {})", line, count, max)
            }
            CompileError::InvalidPatchTarget { ip, code_len, line, .. } => {
                write!(
                    f,
                    "[line {}] invalid patch target: ip {} but code length is {}",
                    line, ip, code_len
                )
            }
            CompileError::UnsupportedBinaryOperator { op, line, .. } => {
                write!(f, "[line {}] unsupported binary operator: {}", line, op)
            }
            CompileError::UncapturableVariable { name, line, .. } => {
                write!(
                    f,
                    "[line {}] cannot capture variable '{}' — not found in local scope or parent closure captures",
                    line, name
                )
            }
            CompileError::InvalidJumpTarget { ip, line, .. } => {
                write!(f, "[line {}] invalid jump target: no opcode at ip {}", line, ip)
            }
            CompileError::InvalidOpcode { ip, byte, line, .. } => {
                write!(f, "[line {}] invalid opcode byte {} at ip {} in patch_jump", line, byte, ip)
            }
        }
    }
}

impl CompileError {
    /// 返回出错源代码行号（所有变体均有此字段）。
    pub fn line(&self) -> usize {
        match self {
            CompileError::UnexpectedExpression { line, .. }
            | CompileError::UndefinedVariable { line, .. }
            | CompileError::TooManyLocals { line, .. }
            | CompileError::DivisionByZero { line, .. }
            | CompileError::BreakOutsideLoop { line, .. }
            | CompileError::ContinueOutsideLoop { line, .. }
            | CompileError::ReturnOutsideFunction { line, .. }
            | CompileError::InvalidArgumentCount { line, .. }
            | CompileError::Error { line, .. }
            | CompileError::ParseError { line, .. }
            | CompileError::JumpOffsetOverflow { line, .. }
            | CompileError::ConstantPoolOverflow { line, .. }
            | CompileError::ArrayElementOverflow { line, .. }
            | CompileError::ControlStackUnderflow { line, .. }
            | CompileError::TooManyCapturedVariables { line, .. }
            | CompileError::InvalidPatchTarget { line, .. }
            | CompileError::UnsupportedBinaryOperator { line, .. }
            | CompileError::UncapturableVariable { line, .. }
            | CompileError::InvalidJumpTarget { line, .. }
            | CompileError::InvalidOpcode { line, .. } => *line,
        }
    }

    /// 返回出错源代码列号（所有变体均有此字段；无精确列号时返回 0）。
    pub fn column(&self) -> Option<usize> {
        match self {
            CompileError::UnexpectedExpression { column, .. }
            | CompileError::UndefinedVariable { column, .. }
            | CompileError::TooManyLocals { column, .. }
            | CompileError::DivisionByZero { column, .. }
            | CompileError::BreakOutsideLoop { column, .. }
            | CompileError::ContinueOutsideLoop { column, .. }
            | CompileError::ReturnOutsideFunction { column, .. }
            | CompileError::InvalidArgumentCount { column, .. }
            | CompileError::Error { column, .. }
            | CompileError::ParseError { column, .. }
            | CompileError::JumpOffsetOverflow { column, .. }
            | CompileError::ConstantPoolOverflow { column, .. }
            | CompileError::ArrayElementOverflow { column, .. }
            | CompileError::ControlStackUnderflow { column, .. }
            | CompileError::TooManyCapturedVariables { column, .. }
            | CompileError::InvalidPatchTarget { column, .. }
            | CompileError::UnsupportedBinaryOperator { column, .. }
            | CompileError::UncapturableVariable { column, .. }
            | CompileError::InvalidJumpTarget { column, .. }
            | CompileError::InvalidOpcode { column, .. } => Some(*column),
        }
    }
}

impl From<nuzo_frontend::parser::ParseError> for CompileError {
    fn from(err: nuzo_frontend::parser::ParseError) -> Self {
        CompileError::ParseError { message: err.message, line: err.line, column: err.column }
    }
}

impl std::error::Error for CompileError {}

/// 从 `CompileError` 自动转换为 `NuzoError`。
///
/// 编译错误属于编译器层面的问题（语法错误、语义错误、资源限制等），
/// 映射为 `InternalError::CompilerBug` 以便通过 `?` 运算符统一传播。
/// 保留源码位置与编译错误码，避免降级为无位置的 C0000。
impl From<CompileError> for nuzo_values::NuzoError {
    fn from(e: CompileError) -> Self {
        let loc = SourceLocation::new(e.line()).with_column(e.column().unwrap_or(0));
        nuzo_values::NuzoError::internal(
            nuzo_values::InternalError::CompilerBug { message: e.to_string() },
            None,
        )
        .with_source_location(loc)
        .with_code(ErrorCode::CompileError)
    }
}

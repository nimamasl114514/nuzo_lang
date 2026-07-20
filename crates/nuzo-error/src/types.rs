//! 错误收集器核心类型定义
//!
//! 定义错误严重程度分类、错误类别、源码位置、执行上下文快照和调用栈帧信息等类型。

use nuzo_bytecode::Opcode;
use nuzo_core::SourceLocation;
use nuzo_core::Value;
use serde::Serialize;
use std::fmt;

// ============================================================================
// 常量：寄存器快照显示上限
// ============================================================================

/// 寄存器快照最大显示数量（超出此数量不再追加，防止输出爆炸）
const MAX_REGISTER_SNAPSHOT_DISPLAY: usize = 16;

// ============================================================================
// Error Severity Classification
// ============================================================================

/// 错误严重程度分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum ErrorSeverity {
    /// 致命错误（会导致崩溃）
    Fatal,
    /// 严重错误（逻辑错误，结果不正确）
    Error,
    /// 警告（可能有问题，但不影响运行）
    Warning,
    /// 信息提示
    Info,
}

impl fmt::Display for ErrorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorSeverity::Fatal => write!(f, "🔴 致命"),
            ErrorSeverity::Error => write!(f, "🟠 错误"),
            ErrorSeverity::Warning => write!(f, "🟡 警告"),
            ErrorSeverity::Info => write!(f, "🔵 信息"),
        }
    }
}

/// 错误类别（用于智能建议）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, nuzo_proc::MatchSync)]
pub enum ErrorCategory {
    /// 类型相关错误
    TypeMismatch,
    /// 算术运算错误
    Arithmetic,
    /// 内存/栈相关错误
    Memory,
    /// 控制流错误
    ControlFlow,
    /// 未定义行为
    UndefinedBehavior,
    /// 断言/测试失败
    Assertion,
    /// VM 内部错误（栈溢出、无效操作码等）
    Internal,
    /// 其他
    Other,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorCategory::TypeMismatch => write!(f, "类型错误"),
            ErrorCategory::Arithmetic => write!(f, "算术错误"),
            ErrorCategory::Memory => write!(f, "内存错误"),
            ErrorCategory::ControlFlow => write!(f, "控制流错误"),
            ErrorCategory::UndefinedBehavior => write!(f, "未定义行为"),
            ErrorCategory::Assertion => write!(f, "断言错误"),
            ErrorCategory::Internal => write!(f, "内部错误"),
            ErrorCategory::Other => write!(f, "其他"),
        }
    }
}

// ============================================================================
// Structured Fix Suggestion
// ============================================================================

/// 结构化修复建议
///
/// 包含用户可读的修复消息、可选的替换代码片段以及相关的源码位置。
#[derive(Debug, Clone, Serialize)]
pub struct StructuredSuggestion {
    /// 修复建议文本
    pub message: String,

    /// 建议的代码替换文本（可选）
    pub replacement: Option<String>,

    /// 相关源码位置（可选）
    pub span: Option<SourceLocation>,
}

impl StructuredSuggestion {
    /// 创建仅包含消息的结构化建议
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), replacement: None, span: None }
    }

    /// 创建带替换文本的结构化建议
    pub fn with_replacement(message: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self { message: message.into(), replacement: Some(replacement.into()), span: None }
    }
}

// ============================================================================
// Execution Context Snapshot (增强版)
// ============================================================================

/// 执行上下文快照（捕获出错时的状态）- 包含源码映射信息
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// 指令指针位置
    pub ip: usize,

    /// 当前执行的指令
    pub opcode: Option<Opcode>,

    /// 当前调用栈深度
    pub call_depth: usize,

    /// 相关寄存器的值（最多显示前16个）
    pub register_snapshot: Vec<(usize, Value)>,

    /// 操作数寄存器索引
    pub operand_registers: Vec<usize>,

    // ===== 新增：源码映射 =====
    /// 源码位置（如果可用）
    pub source_location: Option<SourceLocation>,
}

impl ExecutionContext {
    /// 创建新的执行上下文（基础版）
    pub fn new(ip: usize, opcode: Option<Opcode>, call_depth: usize) -> Self {
        ExecutionContext {
            ip,
            opcode,
            call_depth,
            register_snapshot: Vec::new(),
            operand_registers: Vec::new(),
            source_location: None,
        }
    }

    /// 创建带源码位置的执行上下文
    pub fn with_source(
        ip: usize,
        opcode: Option<Opcode>,
        call_depth: usize,
        source: SourceLocation,
    ) -> Self {
        ExecutionContext {
            ip,
            opcode,
            call_depth,
            register_snapshot: Vec::new(),
            operand_registers: Vec::new(),
            source_location: Some(source),
        }
    }

    /// 添加寄存器快照
    pub fn add_register(&mut self, idx: usize, value: Value) {
        if self.register_snapshot.len() < MAX_REGISTER_SNAPSHOT_DISPLAY {
            self.register_snapshot.push((idx, value));
        }
    }

    /// 设置操作数寄存器
    pub fn operands(&mut self, regs: Vec<usize>) {
        self.operand_registers = regs;
    }

    /// 设置源码位置
    pub fn source_location(&mut self, loc: SourceLocation) {
        self.source_location = Some(loc);
    }
}

impl fmt::Display for ExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 显示源码位置（如果有）
        if let Some(ref loc) = self.source_location {
            writeln!(f, "  📍 源码位置: {}", loc)?;
        } else {
            writeln!(f, "  📍 指令位置: IP={}", self.ip)?;
        }

        if let Some(ref op) = self.opcode {
            writeln!(f, "  📝 当前指令: {:?}", op)?;
        }
        writeln!(f, "  📚 调用深度: {} 层", self.call_depth)?;

        // 如果有源码行，显示代码上下文
        if let Some(ref loc) = self.source_location
            && let Some(ref line) = loc.source_line
        {
            writeln!(f, "  📄 源代码: {}", line.trim())?;
            if loc.column > 0 {
                writeln!(f, "     {}", " ".repeat(loc.column - 1))?;
                writeln!(f, "     ^")?;
            }
        }

        if !self.operand_registers.is_empty() {
            write!(f, "  🔢 操作数寄存器: ")?;
            for (i, reg) in self.operand_registers.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "r[{}]", reg)?;
            }
            writeln!(f)?;
        }

        if !self.register_snapshot.is_empty() {
            writeln!(f, "  💾 寄存器快照:")?;
            for (idx, val) in &self.register_snapshot {
                writeln!(f, "     r[{}] = {}", idx, val)?;
            }
        }

        Ok(())
    }
}

/// 自定义序列化实现 - 将 Value 转换为字符串表示
impl serde::Serialize for ExecutionContext {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ExecutionContext", 6)?;
        state.serialize_field("ip", &self.ip)?;
        state.serialize_field("opcode", &self.opcode.as_ref().map(|o| format!("{}", o)))?;
        state.serialize_field("call_depth", &self.call_depth)?;

        // 将 register_snapshot 中的 Value 转换为字符串表示
        let snapshot_strings: Vec<(usize, String)> =
            self.register_snapshot.iter().map(|(idx, val)| (*idx, format!("{}", val))).collect();
        state.serialize_field("register_snapshot", &snapshot_strings)?;

        state.serialize_field("operand_registers", &self.operand_registers)?;
        state.serialize_field("source_location", &self.source_location)?;
        state.end()
    }
}

// ============================================================================
// Call Stack Frame (for error tracing)
// ============================================================================

/// 调用栈帧信息（增强版 - 包含源码位置）
#[derive(Debug, Clone, Serialize)]
pub struct StackFrameInfo {
    /// 函数名或标识符
    pub function_name: String,

    /// 帧基址寄存器
    pub base_register: usize,

    /// 该帧内的指令范围
    pub ip_range: (usize, usize),

    // ===== 新增：调用栈增强信息 =====
    /// 源文件名
    pub source_file: Option<String>,

    /// 函数定义的起始行号
    pub definition_line: Option<usize>,

    /// 调用该函数的位置
    pub call_site: Option<SourceLocation>,
}

impl StackFrameInfo {
    /// 创建新的栈帧信息
    pub fn new(function_name: String, base_register: usize) -> Self {
        StackFrameInfo {
            function_name,
            base_register,
            ip_range: (0, 0),
            source_file: None,
            definition_line: None,
            call_site: None,
        }
    }

    /// 设置指令范围
    pub fn ip_range(&mut self, start: usize, end: usize) {
        self.ip_range = (start, end);
    }

    /// 设置源文件信息
    pub fn source(&mut self, file: String, line: usize) {
        self.source_file = Some(file);
        self.definition_line = Some(line);
    }

    /// 设置调用位置
    pub fn call_site(&mut self, site: SourceLocation) {
        self.call_site = Some(site);
    }
}

impl fmt::Display for StackFrameInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 显示函数名和基本信息
        write!(f, "fn {}", self.function_name)?;

        // 如果有源文件信息，显示
        if let Some(ref file) = self.source_file {
            if let Some(line) = self.definition_line {
                write!(f, " @ {}:{}", file, line)?;
            } else {
                write!(f, " @ {}", file)?;
            }
        }

        // 显示寄存器基址
        write!(f, " [base=r[{}]", self.base_register)?;

        // 如果有调用位置，显示
        if let Some(ref site) = self.call_site {
            write!(f, ", called at {}", site)?;
        }

        Ok(())
    }
}

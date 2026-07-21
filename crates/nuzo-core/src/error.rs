//! Unified error system for Nuzo — now in nuzo_core (L1).
//!
//! Merges the old `RuntimeError` and `ProgramError` into a flat [`NuzoErrorKind`] enum,
//! wrapped by [`NuzoError`] struct which carries optional source location metadata.
//! Eliminates redundant error conversion layers while keeping [`InternalError`] and
//! [`VmDiagnosis`] as structured diagnostic types.

use std::fmt;

use crate::source_location::SourceLocation;
use serde::Serialize;

// ============================================================================
// Internal Error -- VM/Compiler bugs (not user code faults)
// ============================================================================

/// Errors caused by VM or compiler bugs, not by user code.
///
/// These represent invariant violations in the runtime itself, such as
/// stack underflow, invalid opcodes, or out-of-bounds indices into
/// internal data structures.
#[derive(Debug, Clone, PartialEq, Serialize, nuzo_proc::MatchSync)]
pub enum InternalError {
    /// Call stack exceeded maximum allowed depth
    StackOverflow { depth: usize, max_depth: usize },
    /// Attempted to pop from an empty stack during an operation
    StackUnderflow { operation: String },
    /// Encountered an unrecognized opcode byte
    InvalidOpcode { opcode: u8 },
    /// Bytecode file version mismatch or opcode incompatible with the expected version.
    InvalidBytecodeVersion { expected: u32, got: u32, opcode: Option<u8> },
    /// Instruction pointer exceeded bytecode length
    BytecodeOutOfBounds { ip: usize, code_len: usize },
    /// Constant pool index out of range
    ConstantOutOfBounds { index: usize, pool_size: usize },
    /// VM executed without a loaded chunk
    NoChunkLoaded,
    /// Register index exceeds allocated register count
    RegisterOutOfBounds { reg: u16, available: usize },
    /// Jump/call target address outside valid code range
    JumpTargetOutOfBounds { target: usize, code_len: usize },
    /// Compiler generated invalid state (should never happen)
    CompilerBug { message: String },
    /// IO operation failed (file read/write, stdin, etc.)
    IoError { message: String },
    /// Lexer error (illegal character, unclosed string, etc.)
    LexerError { message: String },
    /// Parser error (syntax error, unexpected token, etc.)
    ParseError { message: String },
    /// 用户源代码编译错误（非编译器 bug）。
    ///
    /// 与 `CompilerBug` 区分：`CompilerBug` 是编译器内部不变量破坏（应报 internal error），
    /// `UserCompileError` 是用户源代码错误（应友好提示）。
    /// 例如：参数超限、变量未定义、控制流错误、资源超限等。
    UserCompileError { message: String },
    /// ISS: bytecode patch would exceed code bounds
    PatchOverflow,
    /// Benchmark harness was asked to compute statistics over an empty sample set
    EmptySamples,
    /// Global variable index or version exceeds the u16 operand limit
    /// when patching GetGlobal → GetGlobalCached (ISS).
    GlobalIndexOverflow {
        /// Resolved global variable index (must fit in u16)
        idx: usize,
        /// Current global version (must fit in u16)
        ver: u32,
    },
    /// Register file index exceeded the u16 addressable range.
    RegisterOverflow {
        /// Offending register count / index that exceeded u16::MAX
        count: usize,
    },
    /// GC object size estimate exceeded u32::MAX (would truncate on storage).
    GcObjectTooLarge {
        /// Estimated object size in bytes
        size: usize,
    },
    /// GC heap slot at the given index is empty/uninitialised when callers
    /// invoked `Gc::get` / `Gc::get_mut` (which require a valid live index).
    /// Previously this panicked via `panic_get_empty`; now it propagates as
    /// a `Result` so production code can handle it without aborting.
    HeapObjectNotFound {
        /// Heap index that did not resolve to a live `HeapObject`
        idx: u32,
    },
    /// set_global_by_name reported a new variable but resolve_global could not
    /// find it immediately afterwards (internal invariant violation).
    GlobalRegistrationFailed,
    /// A RwLock/Mutex guard was poisoned by a panic on another thread.
    LockPoisoned,
    /// Runtime lazy-import referenced a module that has not been loaded yet.
    ModuleNotLoaded {
        /// Module path that was expected but not loaded.
        path: String,
    },
}

impl fmt::Display for InternalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InternalError::StackOverflow { depth, max_depth } => {
                write!(
                    f,
                    "stack overflow: call depth {} exceeded the maximum of {}. Consider reducing recursion depth or refactoring to use loops.",
                    depth, max_depth
                )
            }
            InternalError::StackUnderflow { operation } => {
                write!(
                    f,
                    "stack underflow during '{}': not enough values on the VM stack. This indicates a bug in the Nuzo runtime.",
                    operation
                )
            }
            InternalError::InvalidOpcode { opcode } => {
                write!(
                    f,
                    "invalid opcode 0x{:02X} encountered: the bytecode may be corrupted or from an incompatible version",
                    opcode
                )
            }
            InternalError::InvalidBytecodeVersion { expected, got, opcode } => {
                if let Some(op) = opcode {
                    write!(
                        f,
                        "bytecode version mismatch: expected version {}, but got version {} (opcode 0x{:02X}). Recompile the source file to fix this.",
                        expected, got, op
                    )
                } else {
                    write!(
                        f,
                        "bytecode version mismatch: expected version {}, but got version {}. Recompile the source file to fix this.",
                        expected, got
                    )
                }
            }
            InternalError::BytecodeOutOfBounds { ip, code_len } => write!(
                f,
                "bytecode out of bounds: instruction pointer {} exceeds code length {}. This indicates a bug in the Nuzo runtime.",
                ip, code_len
            ),
            InternalError::ConstantOutOfBounds { index, pool_size } => write!(
                f,
                "constant pool index out of bounds: index {} exceeds pool size {}. This indicates a bug in the Nuzo runtime.",
                index, pool_size
            ),
            InternalError::NoChunkLoaded => write!(
                f,
                "no bytecode chunk loaded in the VM. Make sure to load a compiled program before execution."
            ),
            InternalError::RegisterOutOfBounds { reg, available } => write!(
                f,
                "register out of bounds: register {} exceeds the {} available registers. This indicates a bug in the Nuzo runtime.",
                reg, available
            ),
            InternalError::JumpTargetOutOfBounds { target, code_len } => write!(
                f,
                "jump target out of bounds: target {} exceeds code length {}. This indicates a bug in the Nuzo runtime.",
                target, code_len
            ),
            InternalError::CompilerBug { message } => {
                write!(
                    f,
                    "compiler bug: {}. Please report this issue to the Nuzo developers.",
                    message
                )
            }
            InternalError::IoError { message } => {
                write!(f, "I/O error: {}", message)
            }
            InternalError::LexerError { message } => {
                write!(f, "lexical error: {}", message)
            }
            InternalError::ParseError { message } => {
                write!(f, "syntax error: {}", message)
            }
            InternalError::UserCompileError { message } => {
                write!(f, "compile error: {}", message)
            }
            InternalError::PatchOverflow => {
                write!(
                    f,
                    "bytecode patch overflow: patching data exceeds code bounds. This indicates a bug in the Nuzo runtime."
                )
            }
            InternalError::EmptySamples => {
                write!(f, "cannot compute benchmark statistics on an empty sample set")
            }
            InternalError::GlobalIndexOverflow { idx, ver } => write!(
                f,
                "global index overflow: idx {} or version {} exceeds the maximum addressable range (65535). This indicates a bug in the Nuzo runtime.",
                idx, ver
            ),
            InternalError::RegisterOverflow { count } => {
                write!(
                    f,
                    "register overflow: {} registers exceeds the maximum of 65535. This indicates a bug in the Nuzo runtime.",
                    count
                )
            }
            InternalError::GcObjectTooLarge { size } => write!(
                f,
                "GC object too large: estimated size {} bytes exceeds the maximum allowed. This indicates a bug in the Nuzo runtime.",
                size
            ),
            InternalError::HeapObjectNotFound { idx } => write!(
                f,
                "GC heap object not found: index {} is empty or invalid. Callers of `Gc::get` / `Gc::get_mut` must guarantee a live index (use `try_get` / `get_mut_if_present` for unverified indices).",
                idx
            ),
            InternalError::GlobalRegistrationFailed => {
                write!(
                    f,
                    "global variable registration failed: a variable was not properly registered. This indicates a bug in the Nuzo runtime."
                )
            }
            InternalError::LockPoisoned => {
                write!(
                    f,
                    "internal lock poisoned: a thread panicked while holding a lock. This indicates a bug in the Nuzo runtime."
                )
            }
            InternalError::ModuleNotLoaded { path } => {
                write!(
                    f,
                    "module not loaded: '{}' has not been loaded yet. This indicates a bug in the Nuzo runtime.",
                    path
                )
            }
        }
    }
}

impl std::error::Error for InternalError {}

// ============================================================================
// ErrorCode -- Stable, user-facing error codes
// ============================================================================

/// Stable, user-facing error code attached to every [`NuzoError`].
///
/// Codes are grouped by category:
/// - `E0001`–`E0010`: program/runtime errors caused by user code.
/// - `C0000`: compilation-related errors (placeholder; detailed `C####` codes TBD).
/// - `I0000`: internal VM/compiler errors (not user code faults).
///
/// The string representation is locked by `#[serde(rename = "...")]`, so the
/// serialized form stays stable even if the Rust variant names evolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Default)]
pub enum ErrorCode {
    /// Type mismatch between expected and actual value (`E0001`).
    #[serde(rename = "E0001")]
    TypeMismatch,
    /// Collection index outside valid range (`E0002`).
    #[serde(rename = "E0002")]
    IndexOutOfBounds,
    /// Division by zero (`E0003`).
    #[serde(rename = "E0003")]
    DivisionByZero,
    /// Arithmetic operation overflowed (`E0004`).
    #[serde(rename = "E0004")]
    ArithmeticOverflow,
    /// Assertion failure (`E0005`).
    #[serde(rename = "E0005")]
    AssertFailed,
    /// Expected a numeric value but got something else (`E0006`).
    #[serde(rename = "E0006")]
    ExpectedNumber,
    /// Function called with wrong number of arguments (`E0007`).
    #[serde(rename = "E0007")]
    InvalidArgumentCount,
    /// Referenced a variable that was never defined (`E0008`).
    #[serde(rename = "E0008")]
    UndefinedVariable,
    /// Object does not support the requested operation (`E0009`).
    #[serde(rename = "E0009")]
    UnsupportedOperation,
    /// VM execution exceeded the configured time limit (`E0010`).
    #[serde(rename = "E0010")]
    ExecutionTimeout,
    /// Compilation-related error placeholder (`C0000`).
    #[serde(rename = "C0000")]
    CompileError,
    /// Import path could not be resolved: file does not exist (`C0001`).
    #[serde(rename = "C0001")]
    ModuleNotFound,
    /// Circular import detected between modules (`C0002`).
    #[serde(rename = "C0002")]
    CircularImport,
    /// Duplicate symbol definition across imported modules (`C0004`).
    ///
    /// `C0003` is reserved for `IoError`-style compile-time errors.
    #[serde(rename = "C0004")]
    DuplicateSymbol,
    /// Syntax error during lexing or parsing (`C0005`).
    ///
    /// Produced by `nuzo_frontend` when the source contains illegal characters,
    /// unterminated string literals, or grammar violations. The `NuzoError`
    /// carrying this code preserves the original `SourceLocation` (line/column)
    /// from `LexerError`/`ParseError` rather than degrading to `I0000`.
    #[serde(rename = "C0005")]
    SyntaxError,
    /// Internal VM/compiler error (`I0000`).
    #[serde(rename = "I0000")]
    #[default]
    Internal,
    /// Bytecode file version mismatch (`I0100`).
    #[serde(rename = "I0100")]
    InvalidBytecodeVersion,
}

// ============================================================================
// SourceLocation -- Defined in nuzo_core::source_location
// ============================================================================
//
// `SourceLocation` 的规范定义在 `nuzo_core::source_location` 中，
// 本模块通过 `use crate::source_location::SourceLocation` 引用，
// 保持 `nuzo_core::SourceLocation` 的唯一定义源。

// ============================================================================
// VmDiagnosis -- Rich diagnostic output for internal errors
// ============================================================================

/// Structured diagnostic report produced when an internal error occurs.
///
/// Contains disassembly around the faulting instruction, a register snapshot,
/// call stack depth, and root cause analysis. Attached optionally to
/// [`NuzoError::Internal`] variants.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VmDiagnosis {
    /// Disassembly of the current chunk's bytecode
    pub disassembly: String,
    /// Instruction pointer where the error occurred (if known)
    pub error_ip: Option<usize>,
    /// Snapshot of register values at error time: (register_index, display_string)
    pub register_snapshot: Vec<(u16, String)>,
    /// Depth of the call stack when the error occurred
    pub call_stack_depth: usize,
    /// Human-readable analysis of the root cause
    pub root_cause_analysis: String,
}

impl fmt::Display for VmDiagnosis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "╔══════════════════════════════════════════╗")?;
        writeln!(f, "║       INTERNAL ERROR DIAGNOSIS           ║")?;
        writeln!(f, "╚══════════════════════════════════════════╝")?;
        writeln!(f)?;

        if let Some(ip) = self.error_ip {
            writeln!(f, ">>> IP=0x{:04X}", ip)?;
        }
        writeln!(f)?;

        writeln!(f, "Disassembly:")?;
        for line in self.disassembly.lines() {
            let is_error_line = self
                .error_ip
                .is_some_and(|ip| line.trim_start().starts_with(&format!("{:04x}", ip)));
            if is_error_line {
                writeln!(f, "{}  >>> ERROR HERE", line)?;
            } else {
                writeln!(f, "  {}", line)?;
            }
        }
        writeln!(f)?;

        writeln!(f, "Register Snapshot:")?;
        if self.register_snapshot.is_empty() {
            writeln!(f, "  (all registers are default/zero)")?;
        } else {
            for (reg_idx, value_str) in &self.register_snapshot {
                writeln!(f, "  r{} = {}", reg_idx, value_str)?;
            }
        }
        writeln!(f)?;

        writeln!(f, "Call Stack Depth: {}", self.call_stack_depth)?;
        writeln!(f)?;

        writeln!(f, "Root Cause Analysis:")?;
        for line in self.root_cause_analysis.lines() {
            writeln!(f, "  {}", line)?;
        }

        Ok(())
    }
}

// ============================================================================
// define_errors! -- Declarative macro for NuzoErrorKind Display impl
// ============================================================================

/// Declarative macro that generates [`fmt::Display`] for [`NuzoErrorKind`].
///
/// # Supported variant formats
///
/// - **Unit variant** (no fields): `VariantName => "format string"`
/// - **Struct variant** (named fields): `VariantName { field1, field2 } => "format with {} {}"`
/// - **Custom match arm** (`; @custom` prefix): for complex Display logic that
///   cannot be expressed as a single format string. The body must evaluate to
///   a value that implements [`fmt::Display`]; the macro wraps it with
///   `write!(f, "{}", body)`.
///
/// # Design rationale
///
/// Centralizes error messages alongside variant definitions, eliminating
/// boilerplate match arms. The compiler enforces exhaustiveness -- adding a
/// new variant without a corresponding macro entry causes a compile error.
///
/// # Example
///
/// ```ignore
/// define_errors! {
///     DivisionByZero => "division by zero",
///     TypeMismatch { expected, actual } => "type mismatch: expected {}, got {}",
///     ; @custom NuzoErrorKind::Internal(err, diag) => {
///         let mut msg = format!("internal error: {}", err);
///         if let Some(ref d) = diag { msg.push_str(&format!("\n{}", d)); }
///         msg
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_errors {
    // Main rule: simple variants + optional custom match arms.
    //
    // Syntax:
    //   Variant { field1, field2 } => "fmt",   // struct variant
    //   UnitVariant => "fmt",                   // unit variant
    //   ; @custom Pattern => { body }           // custom arm (`;` separator)
    //
    // The `@custom` body must return a value implementing `fmt::Display`.
    // The macro wraps it: `write!(f, "{}", { body })`. This avoids macro
    // hygiene issues -- the body never references `f` from the macro definition.
    (
        $( $variant:ident $( { $($field:ident),* $(,)? } )? => $fmt:literal ),* $(,)?
        $( ; @custom $pattern:pat => { $($body:tt)* } )*
    ) => {
        impl fmt::Display for NuzoErrorKind {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $(
                        NuzoErrorKind::$variant $( { $($field),* } )? => write!(f, $fmt $(, $($field),*)?)
                    ),*
                    $(
                        , $pattern => write!(f, "{}", { $($body)* })
                    )*
                }
            }
        }
    };
}

// ============================================================================
// NuzoErrorKind -- Error classification enum
// ============================================================================

/// Error classification enum — the "what went wrong" without location metadata.
///
/// Wrapped by [`NuzoError`] which adds optional [`SourceLocation`].
/// Pattern-match on `error.kind` to inspect the error category.
#[derive(Debug, Clone, PartialEq, Serialize, nuzo_proc::MatchSync)]
pub enum NuzoErrorKind {
    // --- Program errors (user code) ---
    /// Type mismatch between expected and actual value
    TypeMismatch { expected: String, actual: String },
    /// Collection index outside valid range
    IndexOutOfBounds { index: String, length: String },
    /// Division by zero
    DivisionByZero,
    /// Arithmetic operation overflowed
    ArithmeticOverflow,
    /// Assertion failure with message
    AssertFailed { message: String },
    /// Expected a numeric value but got something else
    ExpectedNumber { got: String },
    /// Function called with wrong number of arguments
    InvalidArgumentCount { expected: usize, got: usize },
    /// Referenced a variable that was never defined
    UndefinedVariable { name: String },
    /// Object does not support the requested operation
    UnsupportedOperation { operation: String, type_name: String },
    /// VM execution exceeded the configured time limit
    ExecutionTimeout {
        /// Timeout in milliseconds
        limit_ms: u64,
    },

    // --- Internal errors (VM/compiler bugs) ---
    /// Internal VM/compiler error with optional diagnostic report
    //
    // TODO: `#[allow(dead_code)]` 是历史遗留。
    // 实际上 `Option<VmDiagnosis>` 字段被读取：
    // - 本文件 `define_errors!` 宏的 `@custom` 分支通过 `if let Some(diag) = diagnosis` 读取（见下方 Display impl）
    // - `nuzo-error/src/diagnostic.rs` 通过 `DiagnosticError::diagnosis()` 方法读取
    // - `nuzo-vm/src/dispatch.rs` 多处构造 `Some(VmDiagnosis { ... })`
    // 但因为 `nuzo_proc::MatchSync` 派生宏生成的代码可能未读取该字段，
    // 编译器仍会报 dead_code warning。改进方向：让 MatchSync 派生宏
    // 显式标记所有字段为已使用，或者重构为 `Box<VmDiagnosis>` 减少 enum 体积。
    // 暂保留 allow 以避免 warning，未来在 MatchSync 宏升级后可移除。
    Internal(InternalError, #[allow(dead_code)] Option<VmDiagnosis>), // VmDiagnosis 保留供诊断模式输出

    // --- Import / module errors (compile-time) ---
    /// Import path could not be resolved: the file does not exist.
    ModuleNotFound {
        /// Module path that was requested but not found.
        path: String,
    },
    /// Circular import detected between modules.
    CircularImport {
        /// Ordered chain of module paths forming the cycle.
        chain: Vec<String>,
    },
    /// A symbol was defined more than once across imported modules.
    DuplicateSymbol {
        /// Name of the conflicting symbol.
        name: String,
        /// Source location of the first definition, if known.
        first_location: Option<SourceLocation>,
        /// Source location of the second (conflicting) definition, if known.
        second_location: Option<SourceLocation>,
    },
}

define_errors! {
    TypeMismatch { expected, actual } => "type mismatch: expected a {} value, but got a {} value",
    IndexOutOfBounds { index, length } => "index out of bounds: index {} is beyond the valid range 0..{}",
    DivisionByZero => "division by zero: the divisor must not be zero",
    ArithmeticOverflow => "arithmetic overflow: the result exceeds the representable range of the number type",
    AssertFailed { message } => "assertion failed: {}",
    ExpectedNumber { got } => "expected a number, but got a {}",
    InvalidArgumentCount { expected, got } => "wrong number of arguments: this function expects {} argument(s), but {} were provided",
    UndefinedVariable { name } => "undefined variable: '{}' has not been declared or assigned before use",
    UnsupportedOperation { operation, type_name } => "unsupported operation: cannot perform '{}' on a {} value",
    ExecutionTimeout { limit_ms } => "execution timeout: the program ran longer than the {} ms limit, possibly due to an infinite loop",
    ModuleNotFound { path } => "module not found: '{}'. Check that the file path is correct",
    ; @custom NuzoErrorKind::CircularImport { chain } => {
        format!("circular import detected: {}. Check that module imports do not form a cycle.", chain.join(" -> "))
    }
    ; @custom NuzoErrorKind::DuplicateSymbol { name, first_location, second_location } => {
        let mut msg = format!("duplicate symbol '{}' was defined more than once", name);
        if let Some(loc) = first_location {
            msg.push_str(&format!("\n  first defined at: {}", loc));
        }
        if let Some(loc) = second_location {
            msg.push_str(&format!("\n  redefined at: {}", loc));
        }
        msg
    }
    ; @custom NuzoErrorKind::Internal(err, diagnosis) => {
        // 去掉 err 末尾的句号，避免与上层拼接产生双句号
        let err_str = err.to_string().trim_end_matches('.').to_string();
        // UserCompileError 是用户源代码错误，不是运行时 bug，避免误导用户上报。
        // InternalError::UserCompileError Display 已输出 "compile error: <msg>"，
        // 直接采用，不再叠加前缀。
        let mut msg = match err {
            crate::error::InternalError::UserCompileError { .. } => err_str,
            _ => {
                format!("internal error: {}. This is a bug in the Nuzo runtime, not in your code.", err_str)
            }
        };
        if let Some(diag) = diagnosis {
            msg.push_str(&format!("\n{}", diag));
        }
        msg
    }
}

// ============================================================================
// Bilingual message support -- Chinese translations for NuzoErrorKind
// ============================================================================

/// Returns a Chinese translation of the error message for the given kind.
///
/// Returns `Some(String)` with the Chinese message (including dynamic content)
/// if a translation is available, or `None` if the kind is untranslated
/// (caller should fall back to the English message).
///
/// # Design rationale
///
/// Uses `Option<String>` (not `Option<&str>`) because most variants carry
/// dynamic payloads (e.g. `expected`/`actual` type names) that must be
/// interpolated into the Chinese template.
fn kind_message_zh(kind: &NuzoErrorKind) -> Option<String> {
    match kind {
        NuzoErrorKind::TypeMismatch { expected, actual } => {
            Some(format!("类型不匹配：此处需要 {} 类型的值，但得到的是 {} 类型", expected, actual))
        }
        NuzoErrorKind::IndexOutOfBounds { index, length } => {
            Some(format!("索引越界：索引 {} 超出范围，有效索引为 0..{}", index, length))
        }
        NuzoErrorKind::DivisionByZero => {
            Some("除零错误：除数不能为零，请检查除法表达式的分母".to_string())
        }
        NuzoErrorKind::ArithmeticOverflow => {
            Some("算术溢出：计算结果超出了数值类型的表示范围".to_string())
        }
        NuzoErrorKind::AssertFailed { message } => Some(format!("断言失败：{}", message)),
        NuzoErrorKind::ExpectedNumber { got } => {
            Some(format!("期望数字类型，但得到的是 {} 类型", got))
        }
        NuzoErrorKind::InvalidArgumentCount { expected, got } => {
            Some(format!("参数数量不匹配：该函数需要 {} 个参数，但传入了 {} 个", expected, got))
        }
        NuzoErrorKind::UndefinedVariable { name } => {
            Some(format!("未定义的变量：'{}' 在使用前未被声明或赋值", name))
        }
        NuzoErrorKind::UnsupportedOperation { operation, type_name } => {
            Some(format!("不支持的操作：无法对 {} 类型的值执行 '{}' 操作", type_name, operation))
        }
        NuzoErrorKind::ExecutionTimeout { limit_ms } => {
            Some(format!("执行超时：程序运行时间超过了 {} 毫秒的限制，可能存在死循环", limit_ms))
        }
        NuzoErrorKind::ModuleNotFound { path } => {
            Some(format!("找不到模块：'{}'，请检查文件路径是否正确", path))
        }
        NuzoErrorKind::CircularImport { chain } => Some(format!(
            "检测到循环导入：{}。请检查模块之间的导入关系是否形成了循环。",
            chain.join(" -> ")
        )),
        NuzoErrorKind::DuplicateSymbol { name, first_location, second_location } => {
            let mut msg = format!("重复定义的符号 '{}'", name);
            if let Some(loc) = first_location {
                msg.push_str(&format!("\n  首次定义于：{}", loc));
            }
            if let Some(loc) = second_location {
                msg.push_str(&format!("\n  重复定义于：{}", loc));
            }
            Some(msg)
        }
        NuzoErrorKind::Internal(err, diagnosis) => {
            // 去掉 err 末尾的句号，避免与上层拼接产生双句号
            let err_str = err.to_string().trim_end_matches('.').to_string();
            // UserCompileError 是用户源代码错误，不是运行时 bug，避免误导用户上报。
            // 中文版本只提取 message 字段（InternalError Display 是英文），避免叠加前缀。
            let mut msg = match err {
                crate::error::InternalError::UserCompileError { message } => {
                    format!("编译错误：{}", message.trim_end_matches('.'))
                }
                _ => {
                    format!("内部错误：{}。这是 Nuzo 运行时的 bug，不是你的代码问题。", err_str)
                }
            };
            if let Some(diag) = diagnosis {
                msg.push_str(&format!("\n{}", diag));
            }
            Some(msg)
        }
    }
}

// ============================================================================
// Unified NuzoError -- Struct wrapping kind + optional source location
// ============================================================================

/// Unified error type with optional source code location.
///
/// Wraps a [`NuzoErrorKind`] (the error category) together with an optional
/// [`SourceLocation`] indicating where in the source code the error occurred.
///
/// # Construction
///
/// Use the convenience constructors for each variant, then optionally chain
/// [`with_source_location`](NuzoError::with_source_location):
///
/// ```
/// use nuzo_core::{NuzoError, SourceLocation};
///
/// let err = NuzoError::division_by_zero()
///     .with_source_location(SourceLocation::new(42).with_function("main"));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NuzoError {
    /// The error category and its payload.
    pub kind: NuzoErrorKind,
    /// Optional source code location where the error originated.
    pub source_location: Option<SourceLocation>,
    /// Stable error code for machine-readable identification.
    #[serde(default)]
    pub code: ErrorCode,
}

impl NuzoError {
    // -----------------------------------------------------------------------
    // Convenience constructors
    // -----------------------------------------------------------------------

    /// Type mismatch error.
    pub fn type_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::TypeMismatch { expected: expected.into(), actual: actual.into() },
            source_location: None,
            code: ErrorCode::TypeMismatch,
        }
    }

    /// Index out of bounds error.
    pub fn index_out_of_bounds(index: impl Into<String>, length: impl Into<String>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::IndexOutOfBounds { index: index.into(), length: length.into() },
            source_location: None,
            code: ErrorCode::IndexOutOfBounds,
        }
    }

    /// Division by zero error.
    pub fn division_by_zero() -> Self {
        NuzoError {
            kind: NuzoErrorKind::DivisionByZero,
            source_location: None,
            code: ErrorCode::DivisionByZero,
        }
    }

    /// Arithmetic overflow error.
    pub fn arithmetic_overflow() -> Self {
        NuzoError {
            kind: NuzoErrorKind::ArithmeticOverflow,
            source_location: None,
            code: ErrorCode::ArithmeticOverflow,
        }
    }

    /// Assertion failure error.
    pub fn assert_failed(message: impl Into<String>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::AssertFailed { message: message.into() },
            source_location: None,
            code: ErrorCode::AssertFailed,
        }
    }

    /// Expected number but got something else.
    pub fn expected_number(got: impl Into<String>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::ExpectedNumber { got: got.into() },
            source_location: None,
            code: ErrorCode::ExpectedNumber,
        }
    }

    /// Wrong number of arguments.
    pub fn invalid_argument_count(expected: usize, got: usize) -> Self {
        NuzoError {
            kind: NuzoErrorKind::InvalidArgumentCount { expected, got },
            source_location: None,
            code: ErrorCode::InvalidArgumentCount,
        }
    }

    /// Undefined variable reference.
    pub fn undefined_variable(name: impl Into<String>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::UndefinedVariable { name: name.into() },
            source_location: None,
            code: ErrorCode::UndefinedVariable,
        }
    }

    /// Object does not support the requested operation.
    pub fn unsupported_operation(
        operation: impl Into<String>,
        type_name: impl Into<String>,
    ) -> Self {
        NuzoError {
            kind: NuzoErrorKind::UnsupportedOperation {
                operation: operation.into(),
                type_name: type_name.into(),
            },
            source_location: None,
            code: ErrorCode::UnsupportedOperation,
        }
    }

    /// Internal VM/compiler error with optional diagnosis.
    pub fn internal(err: InternalError, diagnosis: Option<VmDiagnosis>) -> Self {
        NuzoError {
            kind: NuzoErrorKind::Internal(err, diagnosis),
            source_location: None,
            code: ErrorCode::Internal,
        }
    }

    /// Execution timeout exceeded.
    pub fn execution_timeout(limit_ms: u64) -> Self {
        NuzoError {
            kind: NuzoErrorKind::ExecutionTimeout { limit_ms },
            source_location: None,
            code: ErrorCode::ExecutionTimeout,
        }
    }

    // -----------------------------------------------------------------------
    // Source location chaining
    // -----------------------------------------------------------------------

    /// Attach a source location to this error, consuming and returning it.
    ///
    /// Enables a fluent builder pattern:
    /// ```
    /// use nuzo_core::{NuzoError, SourceLocation};
    ///
    /// let err = NuzoError::division_by_zero()
    ///     .with_source_location(SourceLocation::new(10));
    /// assert!(err.source_location.is_some());
    /// ```
    pub fn with_source_location(mut self, loc: SourceLocation) -> Self {
        self.source_location = Some(loc);
        self
    }

    // -----------------------------------------------------------------------
    // Error code chaining
    // -----------------------------------------------------------------------

    /// Attach a stable error code to this error, consuming and returning it.
    pub fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = code;
        self
    }

    /// Return the stable error code attached to this error.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    // -----------------------------------------------------------------------
    // Bilingual formatting
    // -----------------------------------------------------------------------

    /// Format the error **message** (without source location prefix) in the
    /// specified language mode.
    ///
    /// - `LangMode::En` → English (uses [`NuzoErrorKind`]'s [`Display`](fmt::Display))
    /// - `LangMode::Zh` → Chinese (falls back to English if no translation)
    /// - `LangMode::Both` → `"中文 / English"` (falls back to English-only)
    ///
    /// This method does **not** include the `at <location>:` prefix; use
    /// [`to_string_with_lang`](Self::to_string_with_lang) for the full output.
    pub fn format_with_lang(&self, lang: LangMode) -> String {
        let en_msg = self.kind.to_string();
        match lang {
            LangMode::En => en_msg,
            LangMode::Zh => kind_message_zh(&self.kind).unwrap_or(en_msg),
            LangMode::Both => match kind_message_zh(&self.kind) {
                Some(zh) => format!("{} / {}", zh, en_msg),
                None => en_msg,
            },
        }
    }

    /// Format the **full** error output (including `at <location>:` prefix
    /// when present) in the specified language mode.
    ///
    /// This is the language-aware equivalent of [`Display`](fmt::Display).
    /// Useful in tests to avoid depending on the `NUZO_LANG` environment
    /// variable (which would cause race conditions under parallel test
    /// execution).
    pub fn to_string_with_lang(&self, lang: LangMode) -> String {
        let message = self.format_with_lang(lang);
        if let Some(ref loc) = self.source_location {
            format!("at {}: {}", loc, message)
        } else {
            message
        }
    }
}

impl fmt::Display for NuzoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lang = LangMode::from_env();
        let message = self.format_with_lang(lang);
        if let Some(ref loc) = self.source_location {
            write!(f, "at {}: {}", loc, message)
        } else {
            write!(f, "{}", message)
        }
    }
}

impl std::error::Error for NuzoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            NuzoErrorKind::Internal(err, _) => Some(err),
            _ => None,
        }
    }
}

// ============================================================================
// Convenience From implementations
// ============================================================================

impl From<InternalError> for NuzoError {
    fn from(e: InternalError) -> Self {
        NuzoError::internal(e, None)
    }
}

// ============================================================================
// LangMode -- Language selection mode (driven by NUZO_LANG env var)
// ============================================================================

/// 语言模式（从 NUZO_LANG 环境变量读取）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LangMode {
    /// 仅中文
    Zh,
    /// 仅英文
    En,
    /// 双语（默认）
    Both,
}

impl LangMode {
    /// 从 `NUZO_LANG` 环境变量读取语言模式。
    ///
    /// - `zh` → [`LangMode::Zh`]
    /// - `en` → [`LangMode::En`]
    /// - 未设置 / 无法读取 / 未知值 → [`LangMode::Both`]
    pub fn from_env() -> Self {
        match std::env::var("NUZO_LANG").as_deref() {
            Ok("zh") => LangMode::Zh,
            Ok("en") => LangMode::En,
            _ => LangMode::Both,
        }
    }

    /// 根据语言模式选择消息：传入 (中文, 英文)，返回对应的消息
    pub fn select(self, zh: &str, en: &str) -> String {
        match self {
            LangMode::Zh => zh.to_string(),
            LangMode::En => en.to_string(),
            LangMode::Both => format!("{} / {}", zh, en),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_error_display() {
        assert_eq!(
            format!("{}", InternalError::StackOverflow { depth: 256, max_depth: 255 }),
            "stack overflow: call depth 256 exceeded the maximum of 255. Consider reducing recursion depth or refactoring to use loops."
        );
        assert_eq!(
            format!("{}", InternalError::StackUnderflow { operation: "ADD".to_string() }),
            "stack underflow during 'ADD': not enough values on the VM stack. This indicates a bug in the Nuzo runtime."
        );
        assert_eq!(
            format!("{}", InternalError::InvalidOpcode { opcode: 0xFE }),
            "invalid opcode 0xFE encountered: the bytecode may be corrupted or from an incompatible version"
        );
        assert_eq!(
            format!(
                "{}",
                InternalError::InvalidBytecodeVersion { expected: 1, got: 999, opcode: None }
            ),
            "bytecode version mismatch: expected version 1, but got version 999. Recompile the source file to fix this."
        );
        assert_eq!(
            format!(
                "{}",
                InternalError::InvalidBytecodeVersion { expected: 1, got: 2, opcode: Some(0xAB) }
            ),
            "bytecode version mismatch: expected version 1, but got version 2 (opcode 0xAB). Recompile the source file to fix this."
        );
        assert_eq!(
            format!("{}", InternalError::BytecodeOutOfBounds { ip: 100, code_len: 50 }),
            "bytecode out of bounds: instruction pointer 100 exceeds code length 50. This indicates a bug in the Nuzo runtime."
        );
        assert_eq!(
            format!("{}", InternalError::ConstantOutOfBounds { index: 10, pool_size: 5 }),
            "constant pool index out of bounds: index 10 exceeds pool size 5. This indicates a bug in the Nuzo runtime."
        );
        assert_eq!(
            format!("{}", InternalError::NoChunkLoaded),
            "no bytecode chunk loaded in the VM. Make sure to load a compiled program before execution."
        );
        assert_eq!(
            format!("{}", InternalError::RegisterOutOfBounds { reg: 16, available: 8 }),
            "register out of bounds: register 16 exceeds the 8 available registers. This indicates a bug in the Nuzo runtime."
        );
        assert_eq!(
            format!("{}", InternalError::JumpTargetOutOfBounds { target: 200, code_len: 100 }),
            "jump target out of bounds: target 200 exceeds code length 100. This indicates a bug in the Nuzo runtime."
        );
        assert_eq!(
            format!("{}", InternalError::CompilerBug { message: "bad AST node".to_string() }),
            "compiler bug: bad AST node. Please report this issue to the Nuzo developers."
        );
    }

    #[test]
    fn test_internal_error_is_std_error() {
        let err: &dyn std::error::Error = &InternalError::NoChunkLoaded;
        assert!(err.source().is_none());
    }

    #[test]
    fn test_internal_error_clone_eq() {
        let e1 = InternalError::InvalidOpcode { opcode: 0xFF };
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }

    #[test]
    fn test_nuzo_error_display_program_variants() {
        assert_eq!(
            NuzoError::division_by_zero().to_string_with_lang(LangMode::En),
            "division by zero: the divisor must not be zero"
        );
        assert_eq!(
            NuzoError::arithmetic_overflow().to_string_with_lang(LangMode::En),
            "arithmetic overflow: the result exceeds the representable range of the number type"
        );
        assert_eq!(
            NuzoError::type_mismatch("number", "string").to_string_with_lang(LangMode::En),
            "type mismatch: expected a number value, but got a string value"
        );
        assert_eq!(
            NuzoError::index_out_of_bounds("10", "5").to_string_with_lang(LangMode::En),
            "index out of bounds: index 10 is beyond the valid range 0..5"
        );
        assert_eq!(
            NuzoError::assert_failed("value should be positive").to_string_with_lang(LangMode::En),
            "assertion failed: value should be positive"
        );
        assert_eq!(
            NuzoError::expected_number("nil").to_string_with_lang(LangMode::En),
            "expected a number, but got a nil"
        );
    }

    #[test]
    fn test_nuzo_error_display_internal() {
        let internal = NuzoError::internal(InternalError::NoChunkLoaded, None);
        assert_eq!(
            internal.to_string_with_lang(LangMode::En),
            "internal error: no bytecode chunk loaded in the VM. Make sure to load a compiled program before execution. This is a bug in the Nuzo runtime, not in your code."
        );
    }

    #[test]
    fn test_from_internal_to_nuzo() {
        let ie = InternalError::NoChunkLoaded;
        let ne: NuzoError = ie.into();
        match ne.kind {
            NuzoErrorKind::Internal(err, None) => assert_eq!(err, InternalError::NoChunkLoaded),
            other => panic!("expected Internal(None), got {:?}", other),
        }
    }

    #[test]
    fn test_nuzo_error_clone() {
        let e1 = NuzoError::division_by_zero();
        let e2 = e1.clone();
        assert_eq!(e1, e2);

        let e3 = NuzoError::internal(InternalError::NoChunkLoaded, None);
        let e4 = e3.clone();
        assert_eq!(e3, e4);
    }

    #[test]
    fn test_nuzo_error_serde_roundtrip() {
        let ne = NuzoError::type_mismatch("number", "nil");
        let json = serde_json::to_string(&ne).expect("NuzoError should serialize");
        assert!(json.contains("TypeMismatch"));

        let ie = InternalError::StackOverflow { depth: 100, max_depth: 50 };
        let json = serde_json::to_string(&ie).expect("InternalError should serialize");
        assert!(json.contains("StackOverflow"));
    }

    #[test]
    fn test_vm_diagnosis_display() {
        let diag = VmDiagnosis {
            disassembly: "0000  OP_NIL\n0001  OP_RETURN".to_string(),
            error_ip: Some(0),
            register_snapshot: vec![(0, "nil".to_string())],
            call_stack_depth: 1,
            root_cause_analysis: "Empty chunk executed.".to_string(),
        };
        let formatted = format!("{}", diag);
        assert!(formatted.contains("INTERNAL ERROR DIAGNOSIS"));
        assert!(formatted.contains(">>> ERROR HERE"));
    }

    #[test]
    fn test_source_location_display() {
        let loc = SourceLocation::new(10);
        assert_eq!(format!("{}", loc), "<unknown>:10");

        let loc_with_col = SourceLocation::new(10).with_column(5);
        assert_eq!(format!("{}", loc_with_col), "<unknown>:10:5");

        let loc_full = SourceLocation::new(10).with_column(5).with_function("main");
        assert_eq!(format!("{}", loc_full), "<unknown>:10:5 (in function main)");

        let loc_no_col = SourceLocation::new(10).with_function("foo");
        assert_eq!(format!("{}", loc_no_col), "<unknown>:10 (in function foo)");
    }

    #[test]
    fn test_nuzo_error_with_source_location() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation::new(42));
        assert!(err.source_location.is_some());
        let loc = err.source_location.as_ref().unwrap();
        assert_eq!(loc.line, 42);
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "at <unknown>:42: division by zero: the divisor must not be zero"
        );
    }

    #[test]
    fn test_nuzo_error_with_full_source_location() {
        let err = NuzoError::type_mismatch("number", "string").with_source_location(
            SourceLocation::new(10).with_column(5).with_function("calculate"),
        );
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "at <unknown>:10:5 (in function calculate): type mismatch: expected a number value, but got a string value"
        );
    }

    #[test]
    fn test_nuzo_error_without_source_location() {
        let err = NuzoError::division_by_zero();
        assert!(err.source_location.is_none());
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "division by zero: the divisor must not be zero"
        );
    }

    #[test]
    fn test_invalid_argument_count_display() {
        let err = NuzoError::invalid_argument_count(3, 1);
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "wrong number of arguments: this function expects 3 argument(s), but 1 were provided"
        );
        match &err.kind {
            NuzoErrorKind::InvalidArgumentCount { expected, got } => {
                assert_eq!(*expected, 3);
                assert_eq!(*got, 1);
            }
            other => panic!("expected InvalidArgumentCount, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_argument_count_zero_expected() {
        let err = NuzoError::invalid_argument_count(0, 5);
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "wrong number of arguments: this function expects 0 argument(s), but 5 were provided"
        );
    }

    #[test]
    fn test_unsupported_operation_display() {
        let err = NuzoError::unsupported_operation("index read", "nil");
        assert_eq!(
            err.to_string_with_lang(LangMode::En),
            "unsupported operation: cannot perform 'index read' on a nil value"
        );
        match &err.kind {
            NuzoErrorKind::UnsupportedOperation { operation, type_name } => {
                assert_eq!(operation, "index read");
                assert_eq!(type_name, "nil");
            }
            other => panic!("expected UnsupportedOperation, got {:?}", other),
        }
    }

    #[test]
    fn test_unsupported_operation_clone_eq() {
        let e1 = NuzoError::unsupported_operation("foo", "bar");
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }

    #[test]
    fn test_invalid_argument_count_clone_eq() {
        let e1 = NuzoError::invalid_argument_count(2, 3);
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }

    // -----------------------------------------------------------------------
    // Bilingual (Zh / Both) mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_nuzo_error_zh_division_by_zero() {
        let err = NuzoError::division_by_zero();
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "除零错误：除数不能为零，请检查除法表达式的分母"
        );
    }

    #[test]
    fn test_nuzo_error_zh_arithmetic_overflow() {
        let err = NuzoError::arithmetic_overflow();
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "算术溢出：计算结果超出了数值类型的表示范围"
        );
    }

    #[test]
    fn test_nuzo_error_zh_type_mismatch() {
        let err = NuzoError::type_mismatch("number", "string");
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "类型不匹配：此处需要 number 类型的值，但得到的是 string 类型"
        );
    }

    #[test]
    fn test_nuzo_error_zh_index_out_of_bounds() {
        let err = NuzoError::index_out_of_bounds("10", "5");
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "索引越界：索引 10 超出范围，有效索引为 0..5"
        );
    }

    #[test]
    fn test_nuzo_error_zh_expected_number() {
        let err = NuzoError::expected_number("nil");
        assert_eq!(err.to_string_with_lang(LangMode::Zh), "期望数字类型，但得到的是 nil 类型");
    }

    #[test]
    fn test_nuzo_error_zh_invalid_argument_count() {
        let err = NuzoError::invalid_argument_count(3, 1);
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "参数数量不匹配：该函数需要 3 个参数，但传入了 1 个"
        );
    }

    #[test]
    fn test_nuzo_error_zh_undefined_variable() {
        let err = NuzoError::undefined_variable("foo");
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "未定义的变量：'foo' 在使用前未被声明或赋值"
        );
    }

    #[test]
    fn test_nuzo_error_zh_unsupported_operation() {
        let err = NuzoError::unsupported_operation("index read", "nil");
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "不支持的操作：无法对 nil 类型的值执行 'index read' 操作"
        );
    }

    #[test]
    fn test_nuzo_error_zh_execution_timeout() {
        let err = NuzoError::execution_timeout(5000);
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "执行超时：程序运行时间超过了 5000 毫秒的限制，可能存在死循环"
        );
    }

    #[test]
    fn test_nuzo_error_zh_internal_error() {
        let err = NuzoError::internal(InternalError::NoChunkLoaded, None);
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "内部错误：no bytecode chunk loaded in the VM. Make sure to load a compiled program before execution。这是 Nuzo 运行时的 bug，不是你的代码问题。"
        );
    }

    #[test]
    fn test_nuzo_error_zh_with_source_location() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation::new(42));
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "at <unknown>:42: 除零错误：除数不能为零，请检查除法表达式的分母"
        );
    }

    #[test]
    fn test_nuzo_error_zh_module_not_found() {
        let err = NuzoError {
            kind: NuzoErrorKind::ModuleNotFound { path: "foo.nuzo".to_string() },
            source_location: None,
            code: ErrorCode::ModuleNotFound,
        };
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "找不到模块：'foo.nuzo'，请检查文件路径是否正确"
        );
    }

    #[test]
    fn test_nuzo_error_zh_circular_import() {
        let err = NuzoError {
            kind: NuzoErrorKind::CircularImport {
                chain: vec!["a".to_string(), "b".to_string(), "a".to_string()],
            },
            source_location: None,
            code: ErrorCode::CircularImport,
        };
        assert_eq!(
            err.to_string_with_lang(LangMode::Zh),
            "检测到循环导入：a -> b -> a。请检查模块之间的导入关系是否形成了循环。"
        );
    }

    #[test]
    fn test_nuzo_error_both_division_by_zero() {
        let err = NuzoError::division_by_zero();
        assert_eq!(
            err.to_string_with_lang(LangMode::Both),
            "除零错误：除数不能为零，请检查除法表达式的分母 / division by zero: the divisor must not be zero"
        );
    }

    #[test]
    fn test_nuzo_error_both_type_mismatch() {
        let err = NuzoError::type_mismatch("number", "string");
        assert_eq!(
            err.to_string_with_lang(LangMode::Both),
            "类型不匹配：此处需要 number 类型的值，但得到的是 string 类型 / type mismatch: expected a number value, but got a string value"
        );
    }

    #[test]
    fn test_nuzo_error_both_with_source_location() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation::new(42));
        assert_eq!(
            err.to_string_with_lang(LangMode::Both),
            "at <unknown>:42: 除零错误：除数不能为零，请检查除法表达式的分母 / division by zero: the divisor must not be zero"
        );
    }

    #[test]
    fn test_nuzo_error_both_invalid_argument_count() {
        let err = NuzoError::invalid_argument_count(3, 1);
        assert_eq!(
            err.to_string_with_lang(LangMode::Both),
            "参数数量不匹配：该函数需要 3 个参数，但传入了 1 个 / wrong number of arguments: this function expects 3 argument(s), but 1 were provided"
        );
    }

    #[test]
    fn test_kind_message_zh_covers_all_variants() {
        // Ensures every NuzoErrorKind variant has a Chinese translation.
        // If a new variant is added without a translation, this test fails.
        let variants: Vec<NuzoErrorKind> = vec![
            NuzoErrorKind::TypeMismatch { expected: "x".into(), actual: "y".into() },
            NuzoErrorKind::IndexOutOfBounds { index: "0".into(), length: "1".into() },
            NuzoErrorKind::DivisionByZero,
            NuzoErrorKind::ArithmeticOverflow,
            NuzoErrorKind::AssertFailed { message: "msg".into() },
            NuzoErrorKind::ExpectedNumber { got: "nil".into() },
            NuzoErrorKind::InvalidArgumentCount { expected: 1, got: 2 },
            NuzoErrorKind::UndefinedVariable { name: "v".into() },
            NuzoErrorKind::UnsupportedOperation { operation: "op".into(), type_name: "t".into() },
            NuzoErrorKind::ExecutionTimeout { limit_ms: 100 },
            NuzoErrorKind::Internal(InternalError::NoChunkLoaded, None),
            NuzoErrorKind::ModuleNotFound { path: "p".into() },
            NuzoErrorKind::CircularImport { chain: vec!["a".into()] },
            NuzoErrorKind::DuplicateSymbol {
                name: "s".into(),
                first_location: None,
                second_location: None,
            },
        ];
        for v in &variants {
            assert!(
                kind_message_zh(v).is_some(),
                "Missing Chinese translation for variant: {:?}",
                v
            );
        }
    }
}

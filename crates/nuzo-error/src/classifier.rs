//! Nuzo Error Classifier - Automatic Error Categorization and Fix Suggestions
//!
//! This module provides the [`ErrorClassifier`] which automatically categorizes
//! errors by severity and category, generates root cause analysis for internal
//! errors, and produces fix suggestions for program errors.
//!
//! # Usage
//!
//! ```rust,ignore
//! use nuzo::error_classifier::ErrorClassifier;
//! use nuzo::nuzo_values::{NuzoError, InternalError};
//! use nuzo::error_collector::{ErrorSeverity, ErrorCategory};
//!
//! let error = NuzoError::DivisionByZero);
//!
//! // Classify the error
//! let (severity, category) = ErrorClassifier::classify(&error);
//!
//! // Get fix suggestion for program errors
//!     let suggestion = ErrorClassifier::fix_suggestion(&error);
//!     println!("Fix: {}", suggestion);
//! }
//!
//! // Get root cause for internal errors
//! if let NuzoError::Internal(ie, _) = &error {
//!     let root_cause = ErrorClassifier::root_cause(ie);
//!     println!("Root cause: {}", root_cause);
//! }
//! ```

use crate::types::{ErrorCategory, ErrorSeverity, StructuredSuggestion};
use nuzo_core::{InternalError, LangMode, NuzoError, NuzoErrorKind};

/// Automatic error classifier for the Nuzo runtime.
///
/// Provides three core capabilities:
/// 1. **classify()** - Maps any `NuzoError` to a severity/category pair
/// 2. **root_cause()** - Generates detailed root cause analysis for `InternalError`
/// 3. **fix_suggestion()** - Generates actionable fix suggestions for `ProgramError`
///
/// All methods are associated functions (no state), making this a zero-cost
/// utility that can be called directly via `ErrorClassifier::method()`.
pub struct ErrorClassifier;

impl ErrorClassifier {
    // ========================================================================
    // Error Classification
    // ========================================================================

    /// Classify a [`NuzoError`] into its severity and category.
    ///
    /// # Classification Rules
    ///
    /// | Error | Severity | Category |
    /// |-------|----------|----------|
    /// | `NuzoError::DivisionByZero` | Error | Arithmetic |
    /// | `NuzoError::ArithmeticOverflow` | Warning | Arithmetic |
    /// | `NuzoError::TypeMismatch` | Error | TypeMismatch |
    /// | `NuzoError::IndexOutOfBounds` | Error | TypeMismatch |
    /// | `NuzoError::AssertFailed` | Error | Assertion |
    /// | `NuzoError::ExpectedNumber` | Error | TypeMismatch |
    /// | `InternalError` (any) | Fatal | UndefinedBehavior |
    ///
    /// # Arguments
    ///
    /// * `error` - The error to classify
    ///
    /// # Returns
    ///
    /// A tuple of `(ErrorSeverity, ErrorCategory)`
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (severity, category) = ErrorClassifier::classify(&nuzo_error);
    /// assert_eq!(severity, ErrorSeverity::Error);
    /// assert_eq!(category, ErrorCategory::Arithmetic);
    /// ```
    pub fn classify(error: &NuzoError) -> (ErrorSeverity, ErrorCategory) {
        match &error.kind {
            NuzoErrorKind::Internal(_, _) => (ErrorSeverity::Fatal, ErrorCategory::Internal),
            _ => Self::classify_program_error(error),
        }
    }

    /// Classify a program error by severity and category.
    fn classify_program_error(error: &NuzoError) -> (ErrorSeverity, ErrorCategory) {
        match &error.kind {
            NuzoErrorKind::DivisionByZero => (ErrorSeverity::Error, ErrorCategory::Arithmetic),
            NuzoErrorKind::ArithmeticOverflow => (ErrorSeverity::Error, ErrorCategory::Arithmetic),
            NuzoErrorKind::TypeMismatch { .. } => {
                (ErrorSeverity::Error, ErrorCategory::TypeMismatch)
            }
            NuzoErrorKind::IndexOutOfBounds { .. } => {
                (ErrorSeverity::Error, ErrorCategory::TypeMismatch)
            }
            NuzoErrorKind::AssertFailed { .. } => (ErrorSeverity::Error, ErrorCategory::Assertion),
            NuzoErrorKind::ExpectedNumber { .. } => {
                (ErrorSeverity::Error, ErrorCategory::TypeMismatch)
            }
            NuzoErrorKind::InvalidArgumentCount { .. } => {
                (ErrorSeverity::Error, ErrorCategory::TypeMismatch)
            }
            NuzoErrorKind::UndefinedVariable { .. } => {
                (ErrorSeverity::Error, ErrorCategory::TypeMismatch)
            }
            _ => (ErrorSeverity::Error, ErrorCategory::TypeMismatch),
        }
    }

    // ========================================================================
    // Root Cause Analysis for Internal Errors
    // ========================================================================

    /// Generate a detailed root cause analysis for an [`InternalError`].
    ///
    /// Each variant produces a human-readable explanation that:
    /// - Describes what went wrong
    /// - Identifies which component is at fault (compiler, VM, or user code)
    /// - Suggests where to look for the bug
    ///
    /// # Arguments
    ///
    /// * `error` - The internal error to analyze
    ///
    /// # Returns
    ///
    /// A string containing the root cause analysis
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ie = InternalError::InvalidOpcode { opcode: 0xFF };
    /// let cause = ErrorClassifier::root_cause(&ie);
    /// assert!(cause.contains("compiler bug"));
    /// ```
    pub fn root_cause(error: &InternalError) -> String {
        match error {
            InternalError::StackOverflow { depth, max_depth } => {
                format!(
                    "Stack overflow at depth {}/{}. This is likely caused by \
                    deeply nested function calls or infinite recursion in user code. \
                    If the recursion depth seems reasonable, this may be a VM bug.",
                    depth, max_depth
                )
            }
            InternalError::StackUnderflow { operation } => {
                format!(
                    "Stack underflow during '{}'. This indicates a VM bug: \
                    an internal stack operation attempted to pop from an empty stack.",
                    operation
                )
            }
            InternalError::InvalidOpcode { opcode } => {
                format!(
                    "Invalid opcode {} (0x{:02X}) found in bytecode. \
                    This indicates a compiler bug: the bytecode stream contains an invalid instruction. \
                    Possible causes:\n  \
                    - Compiler generated an opcode that doesn't exist in the instruction set\n  \
                    - Bytecode corruption during compilation or loading\n  \
                    - Opcode enum mismatch between compiler and VM versions\n\n\
                    Suggestion: Check the compiler's code generation for the source line corresponding to this instruction.",
                    opcode, opcode
                )
            }
            InternalError::BytecodeOutOfBounds { ip, code_len } => {
                format!(
                    "Instruction pointer {} exceeds bytecode length {}. \
                    This indicates a compiler bug: the instruction pointer went past \
                    the end of the bytecode.\n\n\
                    Suggestion: Recompile the source file to regenerate valid bytecode.",
                    ip, code_len
                )
            }
            InternalError::ConstantOutOfBounds { index, pool_size } => {
                format!(
                    "Constant pool index {} out of bounds (pool size={}). \
                    This indicates a compiler bug: the bytecode references a constant \
                    that doesn't exist.\n\n\
                    Suggestion: Check the compiler's constant pool management.",
                    index, pool_size
                )
            }
            InternalError::NoChunkLoaded => {
                "No chunk loaded into VM. This indicates a usage error: \
                ensure compile() was called before run()."
                    .to_string()
            }
            InternalError::RegisterOutOfBounds { reg, available } => {
                format!(
                    "Register r{} out of bounds (available={}). This indicates a \
                    compiler bug: the bytecode references a register that was never allocated.",
                    reg, available
                )
            }
            InternalError::JumpTargetOutOfBounds { target, code_len } => {
                format!(
                    "Jump target {} out of bounds (code length={}). This indicates \
                    a compiler bug: a jump instruction points outside the bytecode.",
                    target, code_len
                )
            }
            InternalError::CompilerBug { message } => {
                format!("Compiler bug: {}. This should never happen - report as a bug.", message)
            }
            InternalError::IoError { message } => {
                format!(
                    "I/O error: {}. This indicates a runtime I/O failure \
                    (file read/write, stdin/stdout, etc.).\n\n\
                    Suggestion: Check file paths, permissions, and disk space.",
                    message
                )
            }
            InternalError::LexerError { message } => {
                format!(
                    "Lexer error: {}. This indicates the source code contains \
                    invalid characters or unclosed string literals.\n\n\
                    Suggestion: Check the source for illegal characters or \
                    missing closing quotes.",
                    message
                )
            }
            InternalError::ParseError { message } => {
                format!(
                    "Parse error: {}. This indicates the source code has a \
                    syntax error (unexpected token, missing delimiter, etc.).\n\n\
                    Suggestion: Review the syntax around the reported location.",
                    message
                )
            }
            InternalError::PatchOverflow => {
                "Bytecode patch overflow: the patch data would exceed code bounds. \
                 This indicates an ISS (Instruction Self-Specialization) internal error \
                 where a specialized instruction's operands are larger than the original \
                 generic instruction."
                    .to_string()
            }
            InternalError::EmptySamples => {
                "Benchmark statistics requested over an empty sample set. \
                 This indicates the benchmark harness was asked to compute statistics \
                 without any measured iterations (e.g. iterations = 0)."
                    .to_string()
            }
            InternalError::GlobalIndexOverflow { idx, ver } => {
                format!(
                    "Global index overflow during ISS patch: resolved global index {} or \
                     version {} exceeds the u16 operand range. This indicates more than \
                     65535 globals or an extremely long-lived version counter.",
                    idx, ver
                )
            }
            InternalError::RegisterOverflow { count } => {
                format!(
                    "Register index overflow: {} exceeds u16::MAX (65535). \
                     This indicates the register file grew beyond the addressable range, \
                     likely due to an extremely large frame or array/builtin argument count.",
                    count
                )
            }
            InternalError::GcObjectTooLarge { size } => {
                format!(
                    "GC object size estimate {} bytes exceeds u32::MAX. \
                     A single heap object cannot exceed 4 GiB; this indicates a runaway \
                     allocation (e.g. an unbounded array).",
                    size
                )
            }
            InternalError::HeapObjectNotFound { idx } => {
                format!(
                    "GC heap object not found at index {}. \
                     The slot is empty or the index is invalid. \
                     This indicates a VM bug: `Gc::get` / `Gc::get_mut` was called with \
                     an unverified index; use `try_get` / `get_mut_if_present` for \
                     unverified indices.",
                    idx
                )
            }
            InternalError::GlobalRegistrationFailed => {
                "Global registration failed: set_global_by_name reported creating a new \
                 variable, but resolve_global could not find it immediately afterwards. \
                 This is an internal invariant violation in the global table."
                    .to_string()
            }
            InternalError::LockPoisoned => {
                "A synchronization primitive (RwLock/Mutex) was poisoned by a panic on \
                 another thread. This indicates a prior panic while holding the lock."
                    .to_string()
            }
            InternalError::ModuleNotLoaded { path } => {
                format!(
                    "Module '{}' was referenced via a lazy import but has not been loaded \
                     yet. This indicates the import resolver did not register the module \
                     before it was needed at runtime.",
                    path
                )
            }
            InternalError::InvalidBytecodeVersion { expected, got, opcode } => {
                let op_str = opcode.map(|op| format!(" (opcode 0x{:02X})", op)).unwrap_or_default();
                format!(
                    "Bytecode version mismatch: expected {}, got {}{}. \
                     The bytecode file was produced by a different version of Nuzo \
                     and cannot be loaded safely. \n\n\
                     Suggestion: recompile the source with the current compiler, \
                     or load the file with the matching VM version.",
                    expected, got, op_str
                )
            }
        }
    }

    // ========================================================================
    // Root Cause Analysis for NuzoError (Unified Interface)
    // ========================================================================

    /// Generate root cause analysis for any [`NuzoError`].
    ///
    /// This is a unified interface that dispatches to the appropriate handler
    /// based on whether the error is a program error or internal error.
    ///
    /// # Arguments
    ///
    /// * `error` - The error to analyze
    ///
    /// # Returns
    ///
    /// A string containing the root cause analysis
    pub fn generate_root_cause(error: &NuzoError) -> String {
        match &error.kind {
            NuzoErrorKind::Internal(ie, _) => Self::root_cause(ie),
            _ => Self::generate_program_root_cause(error),
        }
    }

    /// Generate root cause analysis for program errors.
    fn generate_program_root_cause(err: &NuzoError) -> String {
        format!("Program logic error: {}", err)
    }

    // ========================================================================
    // Fix Suggestions for Program Errors
    // ========================================================================

    /// Generate an actionable fix suggestion for a [`ProgramError`].
    ///
    /// Each variant produces a specific, practical suggestion that the
    /// developer can follow to fix the error in their code.
    ///
    /// # Arguments
    ///
    /// * `error` - The program error to generate a suggestion for
    ///
    /// # Returns
    ///
    /// A string containing the fix suggestion
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let pe = NuzoError::DivisionByZero;
    /// let suggestion = ErrorClassifier::fix_suggestion(&pe);
    /// assert!(suggestion.contains("zero divisors"));
    /// ```
    pub fn fix_suggestion(error: &NuzoError) -> String {
        match &error.kind {
            NuzoErrorKind::DivisionByZero => {
                "Check for zero divisors before division. Use an if-statement to guard: \
                if divisor != 0 { result = a / b }"
                    .to_string()
            }
            NuzoErrorKind::ArithmeticOverflow => {
                "The result exceeds the integer range. Consider using floating-point \
                arithmetic or breaking the calculation into smaller steps."
                    .to_string()
            }
            NuzoErrorKind::TypeMismatch { expected, actual } => {
                format!(
                    "Expected {} but got {}. Check the type of the value before the operation.",
                    expected, actual
                )
            }
            NuzoErrorKind::IndexOutOfBounds { index, length } => {
                format!(
                    "Index {} is out of bounds (length={}). Check the index before accessing: \
                    if idx < len(arr) {{ arr[idx] }}",
                    index, length
                )
            }
            NuzoErrorKind::AssertFailed { message } => {
                format!("Assertion failed: {}. Review the condition being asserted.", message)
            }
            NuzoErrorKind::ExpectedNumber { got } => {
                format!(
                    "Expected a number but got {}. Ensure the value is numeric before \
                    performing arithmetic.",
                    got
                )
            }
            NuzoErrorKind::InvalidArgumentCount { expected, got } => {
                format!(
                    "Function expects {} arguments but got {}. Check the function call.",
                    expected, got
                )
            }
            NuzoErrorKind::UndefinedVariable { name } => {
                format!("Define variable '{}' before use or check for typos", name)
            }
            NuzoErrorKind::Internal(ie, _) => {
                format!(
                    "Internal error: {}. This is likely a VM/compiler bug. Please report it.",
                    ie
                )
            }
            NuzoErrorKind::UnsupportedOperation { operation, type_name } => {
                format!(
                    "{} does not support '{}'. Check the type documentation for supported operations.",
                    type_name, operation
                )
            }
            NuzoErrorKind::ExecutionTimeout { limit_ms } => {
                format!(
                    "Execution exceeded {} ms timeout. Optimize algorithm or increase the timeout limit.",
                    limit_ms
                )
            }
            NuzoErrorKind::ModuleNotFound { path } => {
                format!(
                    "Module '{}' could not be found. Check the import path for typos and ensure the file exists on disk.",
                    path
                )
            }
            NuzoErrorKind::CircularImport { chain } => {
                format!(
                    "Circular import detected: {}. Break the cycle by removing or deferring one import in the chain.",
                    chain.join(" -> ")
                )
            }
            NuzoErrorKind::DuplicateSymbol { name, .. } => {
                format!(
                    "Symbol '{}' is defined more than once. Rename one of the definitions or remove the duplicate.",
                    name
                )
            }
        }
    }

    // ========================================================================
    // Fix Suggestions for NuzoError (Unified Interface - returns Vec<String>)
    // ========================================================================

    /// Generate multiple fix suggestions for any [`NuzoError`].
    ///
    /// Returns a vector of suggestions, allowing for multiple possible fixes.
    /// This is more comprehensive than [`fix_suggestion()`] and should be used
    /// when displaying diagnostic information to users.
    ///
    /// # Arguments
    ///
    /// * `error` - The error to generate suggestions for
    ///
    /// # Returns
    ///
    /// A vector of fix suggestion strings
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let error = NuzoError::DivisionByZero);
    /// let suggestions = ErrorClassifier::generate_fix_suggestion(&error);
    /// for (i, suggestion) in suggestions.iter().enumerate() {
    ///     println!("{}. {}", i + 1, suggestion);
    /// }
    /// ```
    pub fn generate_fix_suggestion(error: &NuzoError) -> Vec<String> {
        match &error.kind {
            NuzoErrorKind::Internal(ie, _) => Self::generate_internal_fix_suggestion(ie),
            _ => Self::generate_program_fix_suggestion(error),
        }
    }

    /// Generate fix suggestions for program errors.
    fn generate_program_fix_suggestion(err: &NuzoError) -> Vec<String> {
        match &err.kind {
            NuzoErrorKind::DivisionByZero => vec![
                "Check if divisor could be zero before division".to_string(),
                "Add a guard: if denominator != 0 { result = a / b }".to_string(),
            ],
            NuzoErrorKind::TypeMismatch { expected, actual } => vec![
                format!("Expected type '{}', but got '{}'", expected, actual),
                "Verify the variable types in your expression".to_string(),
            ],
            NuzoErrorKind::IndexOutOfBounds { index, length } => vec![
                format!("Index {} is out of bounds for collection of length {}", index, length),
                "Use len() to check bounds before indexing".to_string(),
            ],
            NuzoErrorKind::AssertFailed { message } => vec![
                format!("Assertion failed: {}", message),
                "Check your assumptions about program state".to_string(),
            ],
            _ => vec!["Review the operation that caused this error".to_string()],
        }
    }

    /// Generate fix suggestions for internal errors.
    fn generate_internal_fix_suggestion(err: &InternalError) -> Vec<String> {
        match err {
            InternalError::StackOverflow { max_depth, .. } => vec![
                format!("Stack overflow: exceeded maximum depth of {}", max_depth),
                "This may indicate infinite recursion in user code".to_string(),
                "Or a compiler bug generating deeply nested calls".to_string(),
            ],
            InternalError::InvalidOpcode { opcode } => vec![
                format!("Invalid opcode {} (0x{:02X}) found in bytecode", opcode, opcode),
                "The compiler generated an unrecognized instruction".to_string(),
                "Report this as a compiler bug".to_string(),
            ],
            InternalError::BytecodeOutOfBounds { ip, code_len } => vec![
                format!("Instruction pointer {} exceeds bytecode length {}", ip, code_len),
                "The bytecode stream is corrupted or truncated".to_string(),
                "Recompile the source file".to_string(),
            ],
            InternalError::NoChunkLoaded => vec![
                "No bytecode chunk loaded into VM".to_string(),
                "Ensure compile() was called before run()".to_string(),
            ],
            InternalError::CompilerBug { message } => vec![
                format!("Compiler bug: {}", message),
                "This should never happen - report as a bug".to_string(),
            ],
            _ => vec![
                format!("Internal error: {}", err),
                "This indicates a bug in the runtime itself".to_string(),
                "Please report this issue with reproduction steps".to_string(),
            ],
        }
    }

    // ========================================================================
    // Structured Fix Suggestions for Program Errors
    // ========================================================================

    /// Generate structured fix suggestions for any [`NuzoError`].
    ///
    /// Returns a vector of [`StructuredSuggestion`] values, each containing a
    /// human-readable message plus optional replacement code and source span.
    /// This preserves the existing string-based helpers while adding a machine-
    /// readable layer for IDEs, formatters and external tooling.
    ///
    /// # Arguments
    ///
    /// * `error` - The error to generate suggestions for
    ///
    /// # Returns
    ///
    /// A vector of structured suggestions
    pub fn generate_structured_suggestions(error: &NuzoError) -> Vec<StructuredSuggestion> {
        match &error.kind {
            NuzoErrorKind::Internal(ie, _) => Self::generate_internal_structured_suggestions(ie),
            _ => Self::generate_program_structured_suggestions(error),
        }
    }

    /// Generate structured suggestions for program errors.
    fn generate_program_structured_suggestions(err: &NuzoError) -> Vec<StructuredSuggestion> {
        match &err.kind {
            NuzoErrorKind::DivisionByZero => vec![
                StructuredSuggestion::new("ensure the divisor is not zero before dividing"),
                StructuredSuggestion::with_replacement(
                    "add a guard around the division",
                    "if divisor != 0 { result = a / b }",
                ),
            ],
            NuzoErrorKind::ArithmeticOverflow => vec![
                StructuredSuggestion::new(
                    "the result exceeds the integer range; consider using floating-point arithmetic",
                ),
                StructuredSuggestion::new(
                    "break the calculation into smaller steps to avoid overflow",
                ),
            ],
            NuzoErrorKind::TypeMismatch { expected, actual } => {
                vec![StructuredSuggestion::new(format!(
                    "check the value type or add a conversion (expected {}, got {})",
                    expected, actual
                ))]
            }
            NuzoErrorKind::IndexOutOfBounds { index, length } => vec![
                StructuredSuggestion::new(format!(
                    "check the index before accessing (index={}, length={})",
                    index, length
                )),
                StructuredSuggestion::with_replacement(
                    "use a bounds guard",
                    "if idx < len(arr) { arr[idx] }",
                ),
            ],
            NuzoErrorKind::AssertFailed { message } => vec![StructuredSuggestion::new(format!(
                "review the condition being asserted: {}",
                message
            ))],
            NuzoErrorKind::ExpectedNumber { got } => vec![StructuredSuggestion::new(format!(
                "ensure the value is numeric before performing arithmetic (got {})",
                got
            ))],
            NuzoErrorKind::InvalidArgumentCount { expected, got } => {
                vec![StructuredSuggestion::new(format!(
                    "verify the function signature and argument count (expected {}, got {})",
                    expected, got
                ))]
            }
            NuzoErrorKind::UndefinedVariable { name } => vec![StructuredSuggestion::new(format!(
                "define the variable '{}' before use or check for typos",
                name
            ))],
            NuzoErrorKind::UnsupportedOperation { operation, type_name } => {
                vec![StructuredSuggestion::new(format!(
                    "{} does not support '{}'; check the type documentation for supported operations",
                    type_name, operation
                ))]
            }
            NuzoErrorKind::ExecutionTimeout { limit_ms } => {
                vec![StructuredSuggestion::new(format!(
                    "optimize the algorithm or increase the timeout limit (current: {} ms)",
                    limit_ms
                ))]
            }
            NuzoErrorKind::ModuleNotFound { path } => vec![StructuredSuggestion::new(format!(
                "verify the import path '{}' for typos and ensure the module file exists",
                path
            ))],
            NuzoErrorKind::CircularImport { chain } => vec![StructuredSuggestion::new(format!(
                "break the import cycle: {}",
                chain.join(" -> ")
            ))],
            NuzoErrorKind::DuplicateSymbol { name, .. } => {
                vec![StructuredSuggestion::new(format!(
                    "rename or remove the duplicate definition of symbol '{}'",
                    name
                ))]
            }
            NuzoErrorKind::Internal(_, _) => unreachable!("internal errors handled above"),
        }
    }

    /// Generate structured suggestions for internal errors.
    fn generate_internal_structured_suggestions(err: &InternalError) -> Vec<StructuredSuggestion> {
        vec![StructuredSuggestion::new(format!(
            "internal error: {}. This is likely a VM/compiler bug. Please report it with reproduction steps.",
            err
        ))]
    }

    // ========================================================================
    // Language-aware variants
    // ========================================================================

    /// Language-aware version of [`fix_suggestion`](Self::fix_suggestion).
    ///
    /// Returns the single best suggestion in the language specified by `lang`:
    /// - `LangMode::En` → English (same as `fix_suggestion`)
    /// - `LangMode::Zh` → Chinese
    /// - `LangMode::Both` → "中文 / English"
    pub fn fix_suggestion_with_lang(error: &NuzoError, lang: LangMode) -> String {
        match &error.kind {
            NuzoErrorKind::DivisionByZero => lang.select(
                "除法前确保除数不为零，可用 if 语句保护：if divisor != 0 { result = a / b }",
                "Check for zero divisors before division. Use an if-statement to guard: \
                if divisor != 0 { result = a / b }",
            ),
            NuzoErrorKind::ArithmeticOverflow => lang.select(
                "计算结果超出整数范围，考虑使用浮点运算或将计算拆分为更小的步骤",
                "The result exceeds the integer range. Consider using floating-point \
                arithmetic or breaking the calculation into smaller steps.",
            ),
            NuzoErrorKind::TypeMismatch { expected, actual } => lang.select(
                &format!("期望 {} 但得到 {}，操作前请检查值的类型", expected, actual),
                &format!(
                    "Expected {} but got {}. Check the type of the value before the operation.",
                    expected, actual
                ),
            ),
            NuzoErrorKind::IndexOutOfBounds { index, length } => lang.select(
                &format!(
                    "索引 {} 越界（长度 {}），访问前请检查：if idx < len(arr) {{ arr[idx] }}",
                    index, length
                ),
                &format!(
                    "Index {} is out of bounds (length={}). Check the index before accessing: \
                    if idx < len(arr) {{ arr[idx] }}",
                    index, length
                ),
            ),
            NuzoErrorKind::AssertFailed { message } => lang.select(
                &format!("断言失败：{}，请复查断言的条件", message),
                &format!("Assertion failed: {}. Review the condition being asserted.", message),
            ),
            NuzoErrorKind::ExpectedNumber { got } => lang.select(
                &format!("期望数字但得到 {}，执行算术前请确保值是数字类型", got),
                &format!(
                    "Expected a number but got {}. Ensure the value is numeric before performing arithmetic.",
                    got
                ),
            ),
            NuzoErrorKind::InvalidArgumentCount { expected, got } => lang.select(
                &format!("函数期望 {} 个参数但传入 {} 个，请核对函数调用", expected, got),
                &format!(
                    "Function expects {} arguments but got {}. Check the function call.",
                    expected, got
                ),
            ),
            NuzoErrorKind::UndefinedVariable { name } => lang.select(
                &format!("使用前请定义变量 '{}' 或检查拼写", name),
                &format!("Define variable '{}' before use or check for typos", name),
            ),
            NuzoErrorKind::Internal(ie, _) => lang.select(
                &format!("内部错误：{}，可能是 VM/编译器 bug，请附带复现步骤上报", ie),
                &format!(
                    "Internal error: {}. This is likely a VM/compiler bug. Please report it.",
                    ie
                ),
            ),
            NuzoErrorKind::UnsupportedOperation { operation, type_name } => lang.select(
                &format!("{} 不支持 '{}' 操作，请查阅类型文档确认支持的操作", type_name, operation),
                &format!(
                    "{} does not support '{}'. Check the type documentation for supported operations.",
                    type_name, operation
                ),
            ),
            NuzoErrorKind::ExecutionTimeout { limit_ms } => lang.select(
                &format!("执行超过 {} 毫秒超时，请优化算法或调高超时限制", limit_ms),
                &format!(
                    "Execution exceeded {} ms timeout. Optimize algorithm or increase the timeout limit.",
                    limit_ms
                ),
            ),
            NuzoErrorKind::ModuleNotFound { path } => lang.select(
                &format!("找不到模块 '{}'，请检查导入路径拼写并确认文件存在", path),
                &format!(
                    "Module '{}' could not be found. Check the import path for typos and ensure the file exists on disk.",
                    path
                ),
            ),
            NuzoErrorKind::CircularImport { chain } => lang.select(
                &format!("检测到循环导入：{}，请移除或延后链中某个导入以打破循环", chain.join(" -> ")),
                &format!(
                    "Circular import detected: {}. Break the cycle by removing or deferring one import in the chain.",
                    chain.join(" -> ")
                ),
            ),
            NuzoErrorKind::DuplicateSymbol { name, .. } => lang.select(
                &format!("符号 '{}' 重复定义，请重命名或删除其中一个定义", name),
                &format!(
                    "Symbol '{}' is defined more than once. Rename one of the definitions or remove the duplicate.",
                    name
                ),
            ),
        }
    }

    /// Language-aware version of [`generate_fix_suggestion`](Self::generate_fix_suggestion).
    pub fn generate_fix_suggestion_with_lang(error: &NuzoError, lang: LangMode) -> Vec<String> {
        match &error.kind {
            NuzoErrorKind::Internal(ie, _) => {
                Self::generate_internal_fix_suggestion_with_lang(ie, lang)
            }
            _ => Self::generate_program_fix_suggestion_with_lang(error, lang),
        }
    }

    /// Language-aware helper: program-error fix suggestions.
    fn generate_program_fix_suggestion_with_lang(err: &NuzoError, lang: LangMode) -> Vec<String> {
        match &err.kind {
            NuzoErrorKind::DivisionByZero => match lang {
                LangMode::Zh => vec![
                    "除法前检查除数是否为零".to_string(),
                    "添加保护：if denominator != 0 { result = a / b }".to_string(),
                ],
                LangMode::En => vec![
                    "Check if divisor could be zero before division".to_string(),
                    "Add a guard: if denominator != 0 { result = a / b }".to_string(),
                ],
                LangMode::Both => vec![
                    "除法前检查除数是否为零 / Check if divisor could be zero before division"
                        .to_string(),
                    "添加保护：if denominator != 0 { result = a / b } / Add a guard: if denominator != 0 { result = a / b }".to_string(),
                ],
            },
            NuzoErrorKind::TypeMismatch { expected, actual } => match lang {
                LangMode::Zh => vec![
                    format!("期望类型 '{}'，实际得到 '{}'", expected, actual),
                    "请检查表达式中的变量类型".to_string(),
                ],
                LangMode::En => vec![
                    format!("Expected type '{}', but got '{}'", expected, actual),
                    "Verify the variable types in your expression".to_string(),
                ],
                LangMode::Both => vec![lang.select(
                    &format!("期望类型 '{}'，实际得到 '{}'", expected, actual),
                    &format!("Expected type '{}', but got '{}'", expected, actual),
                )],
            },
            NuzoErrorKind::IndexOutOfBounds { index, length } => match lang {
                LangMode::Zh => vec![
                    format!("索引 {} 超出长度为 {} 的集合范围", index, length),
                    "访问前用 len() 检查边界".to_string(),
                ],
                LangMode::En => vec![
                    format!("Index {} is out of bounds for collection of length {}", index, length),
                    "Use len() to check bounds before indexing".to_string(),
                ],
                LangMode::Both => vec![lang.select(
                    &format!("索引 {} 超出长度为 {} 的集合范围", index, length),
                    &format!(
                        "Index {} is out of bounds for collection of length {}",
                        index, length
                    ),
                )],
            },
            NuzoErrorKind::AssertFailed { message } => match lang {
                LangMode::Zh => vec![
                    format!("断言失败：{}", message),
                    "请检查对程序状态的假设".to_string(),
                ],
                LangMode::En => vec![
                    format!("Assertion failed: {}", message),
                    "Check your assumptions about program state".to_string(),
                ],
                LangMode::Both => vec![lang.select(
                    &format!("断言失败：{}", message),
                    &format!("Assertion failed: {}", message),
                )],
            },
            _ => match lang {
                LangMode::Zh => vec!["请检查导致此错误的操作".to_string()],
                LangMode::En => vec!["Review the operation that caused this error".to_string()],
                LangMode::Both => vec![
                    "请检查导致此错误的操作 / Review the operation that caused this error"
                        .to_string(),
                ],
            },
        }
    }

    /// Language-aware helper: internal-error fix suggestions.
    fn generate_internal_fix_suggestion_with_lang(
        err: &InternalError,
        lang: LangMode,
    ) -> Vec<String> {
        match err {
            InternalError::StackOverflow { max_depth, .. } => match lang {
                LangMode::Zh => vec![
                    format!("栈溢出：超过最大深度 {}", max_depth),
                    "可能是用户代码中的无限递归".to_string(),
                    "或编译器生成了过深的调用".to_string(),
                ],
                LangMode::En => vec![
                    format!("Stack overflow: exceeded maximum depth of {}", max_depth),
                    "This may indicate infinite recursion in user code".to_string(),
                    "Or a compiler bug generating deeply nested calls".to_string(),
                ],
                LangMode::Both => vec![lang.select(
                    &format!("栈溢出：超过最大深度 {}", max_depth),
                    &format!("Stack overflow: exceeded maximum depth of {}", max_depth),
                )],
            },
            InternalError::InvalidOpcode { opcode } => match lang {
                LangMode::Zh => vec![
                    format!("字节码中发现无效操作码 {}（0x{:02X}）", opcode, opcode),
                    "编译器生成了无法识别的指令".to_string(),
                    "请作为编译器 bug 上报".to_string(),
                ],
                LangMode::En => vec![
                    format!("Invalid opcode {} (0x{:02X}) found in bytecode", opcode, opcode),
                    "The compiler generated an unrecognized instruction".to_string(),
                    "Report this as a compiler bug".to_string(),
                ],
                LangMode::Both => vec![lang.select(
                    &format!("字节码中发现无效操作码 {}（0x{:02X}）", opcode, opcode),
                    &format!("Invalid opcode {} (0x{:02X}) found in bytecode", opcode, opcode),
                )],
            },
            _ => match lang {
                LangMode::Zh => vec![
                    format!("内部错误：{}", err),
                    "这表明运行时本身存在 bug".to_string(),
                    "请附带复现步骤上报此问题".to_string(),
                ],
                LangMode::En => vec![
                    format!("Internal error: {}", err),
                    "This indicates a bug in the runtime itself".to_string(),
                    "Please report this issue with reproduction steps".to_string(),
                ],
                LangMode::Both => vec![
                    lang.select(&format!("内部错误：{}", err), &format!("Internal error: {}", err)),
                ],
            },
        }
    }

    /// Language-aware version of [`generate_structured_suggestions`](Self::generate_structured_suggestions).
    pub fn generate_structured_suggestions_with_lang(
        error: &NuzoError,
        lang: LangMode,
    ) -> Vec<StructuredSuggestion> {
        match &error.kind {
            NuzoErrorKind::Internal(ie, _) => {
                Self::generate_internal_structured_suggestions_with_lang(ie, lang)
            }
            _ => Self::generate_program_structured_suggestions_with_lang(error, lang),
        }
    }

    /// Language-aware helper: program-error structured suggestions.
    fn generate_program_structured_suggestions_with_lang(
        err: &NuzoError,
        lang: LangMode,
    ) -> Vec<StructuredSuggestion> {
        match &err.kind {
            NuzoErrorKind::DivisionByZero => match lang {
                LangMode::Zh => vec![
                    StructuredSuggestion::new("除法前确保除数不为零"),
                    StructuredSuggestion::with_replacement(
                        "为除法添加保护",
                        "if divisor != 0 { result = a / b }",
                    ),
                ],
                LangMode::En => vec![
                    StructuredSuggestion::new("ensure the divisor is not zero before dividing"),
                    StructuredSuggestion::with_replacement(
                        "add a guard around the division",
                        "if divisor != 0 { result = a / b }",
                    ),
                ],
                LangMode::Both => vec![StructuredSuggestion::new(lang.select(
                    "除法前确保除数不为零",
                    "ensure the divisor is not zero before dividing",
                ))],
            },
            NuzoErrorKind::ArithmeticOverflow => match lang {
                LangMode::Zh => vec![
                    StructuredSuggestion::new("结果超出整数范围，考虑使用浮点运算"),
                    StructuredSuggestion::new("将计算拆分为更小的步骤以避免溢出"),
                ],
                LangMode::En => vec![
                    StructuredSuggestion::new(
                        "the result exceeds the integer range; consider using floating-point arithmetic",
                    ),
                    StructuredSuggestion::new(
                        "break the calculation into smaller steps to avoid overflow",
                    ),
                ],
                LangMode::Both => vec![StructuredSuggestion::new(lang.select(
                    "结果超出整数范围，考虑使用浮点运算",
                    "the result exceeds the integer range; consider using floating-point arithmetic",
                ))],
            },
            NuzoErrorKind::TypeMismatch { expected, actual } => vec![
                StructuredSuggestion::new(lang.select(
                    &format!("检查值类型或添加类型转换（期望 {}，实际 {}）", expected, actual),
                    &format!("check the value type or add a conversion (expected {}, got {})", expected, actual),
                )),
            ],
            NuzoErrorKind::IndexOutOfBounds { index, length } => match lang {
                LangMode::Zh => vec![
                    StructuredSuggestion::new(format!(
                        "访问前检查索引（index={}, length={}）",
                        index, length
                    )),
                    StructuredSuggestion::with_replacement("使用边界保护", "if idx < len(arr) { arr[idx] }"),
                ],
                LangMode::En => vec![
                    StructuredSuggestion::new(format!(
                        "check the index before accessing (index={}, length={})",
                        index, length
                    )),
                    StructuredSuggestion::with_replacement(
                        "use a bounds guard",
                        "if idx < len(arr) { arr[idx] }",
                    ),
                ],
                LangMode::Both => vec![StructuredSuggestion::new(lang.select(
                    &format!("访问前检查索引（index={}, length={}）", index, length),
                    &format!("check the index before accessing (index={}, length={})", index, length),
                ))],
            },
            NuzoErrorKind::AssertFailed { message } => vec![StructuredSuggestion::new(
                lang.select(
                    &format!("复查被断言的条件：{}", message),
                    &format!("review the condition being asserted: {}", message),
                ),
            )],
            NuzoErrorKind::ExpectedNumber { got } => vec![StructuredSuggestion::new(lang.select(
                &format!("执行算术前确保值是数字类型（实际 {}）", got),
                &format!("ensure the value is numeric before performing arithmetic (got {})", got),
            ))],
            NuzoErrorKind::InvalidArgumentCount { expected, got } => vec![
                StructuredSuggestion::new(lang.select(
                    &format!("核对函数签名和参数个数（期望 {}，实际 {}）", expected, got),
                    &format!("verify the function signature and argument count (expected {}, got {})", expected, got),
                )),
            ],
            NuzoErrorKind::UndefinedVariable { name } => vec![StructuredSuggestion::new(lang.select(
                &format!("使用前定义变量 '{}' 或检查拼写", name),
                &format!("define the variable '{}' before use or check for typos", name),
            ))],
            NuzoErrorKind::UnsupportedOperation { operation, type_name } => vec![
                StructuredSuggestion::new(lang.select(
                    &format!("{} 不支持 '{}'，请查阅类型文档确认支持的操作", type_name, operation),
                    &format!("{} does not support '{}'; check the type documentation for supported operations", type_name, operation),
                )),
            ],
            NuzoErrorKind::ExecutionTimeout { limit_ms } => vec![StructuredSuggestion::new(
                lang.select(
                    &format!("优化算法或调高超时限制（当前 {} 毫秒）", limit_ms),
                    &format!("optimize the algorithm or increase the timeout limit (current: {} ms)", limit_ms),
                ),
            )],
            NuzoErrorKind::ModuleNotFound { path } => vec![StructuredSuggestion::new(lang.select(
                &format!("核对导入路径 '{}' 拼写，并确认模块文件存在", path),
                &format!("verify the import path '{}' for typos and ensure the module file exists", path),
            ))],
            NuzoErrorKind::CircularImport { chain } => vec![StructuredSuggestion::new(lang.select(
                &format!("打破导入循环：{}", chain.join(" -> ")),
                &format!("break the import cycle: {}", chain.join(" -> ")),
            ))],
            NuzoErrorKind::DuplicateSymbol { name, .. } => vec![StructuredSuggestion::new(
                lang.select(
                    &format!("重命名或删除符号 '{}' 的重复定义", name),
                    &format!("rename or remove the duplicate definition of symbol '{}'", name),
                ),
            )],
            NuzoErrorKind::Internal(_, _) => unreachable!("internal errors handled above"),
        }
    }

    /// Language-aware helper: internal-error structured suggestions.
    fn generate_internal_structured_suggestions_with_lang(
        err: &InternalError,
        lang: LangMode,
    ) -> Vec<StructuredSuggestion> {
        vec![StructuredSuggestion::new(lang.select(
            &format!("内部错误：{}，可能是 VM/编译器 bug，请附带复现步骤上报", err),
            &format!("internal error: {}. This is likely a VM/compiler bug. Please report it with reproduction steps.", err),
        ))]
    }

    /// 带候选变量列表的语言感知结构化建议生成。
    ///
    /// 在 [`generate_structured_suggestions_with_lang`] 基础上，当错误为
    /// [`NuzoErrorKind::UndefinedVariable`] 且 `candidates` 非空时，调用
    /// [`crate::similar::suggest_similar`] 按编辑距离筛选相似变量名，
    /// 追加 "Did you mean 'X'?" 风格的纠错建议。
    ///
    /// 候选列表由调用方（如 REPL、IDE 插件、CLI 的 `--suggest-vars` 选项）
    /// 从当前作用域的变量名集合中提供。未提供时退回到基础建议，保持向后兼容。
    ///
    /// # 参数
    ///
    /// - `error` — 待生成建议的错误
    /// - `lang` — 语言模式
    /// - `candidates` — 当前作用域可用的变量名列表（可为空）
    ///
    /// # 返回
    ///
    /// 结构化建议向量； UndefinedVariable 错误下会在基础建议后追加
    /// "Did you mean X?" 形式的纠错建议（最多 3 条，按距离升序排列）。
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// use nuzo_core::{LangMode, NuzoError};
    /// use nuzo_error::ErrorClassifier;
    ///
    /// let err = NuzoError::undefined_variable("conut");
    /// let candidates = vec!["count".to_string(), "counter".to_string()];
    /// let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
    ///     &err,
    ///     LangMode::En,
    ///     &candidates,
    /// );
    /// assert!(suggestions.iter().any(|s| s.message.contains("Did you mean 'count'?")));
    /// ```
    pub fn generate_structured_suggestions_with_candidates(
        error: &NuzoError,
        lang: LangMode,
        candidates: &[String],
    ) -> Vec<StructuredSuggestion> {
        let mut suggestions = Self::generate_structured_suggestions_with_lang(error, lang);

        if let NuzoErrorKind::UndefinedVariable { name } = &error.kind
            && !candidates.is_empty()
            && !name.is_empty()
        {
            // 拼写纠错场景下，短标识符（如 "conut"→"count" 距离 2）也需要被识别，
            // 因此阈值至少为 2，避免 `default_max_distance` 对短字符串过严。
            let max_distance = crate::similar::default_max_distance(name).max(2);
            let similar = crate::similar::suggest_similar(name, candidates, 3, max_distance);
            for candidate in similar {
                let message = lang.select(
                    &format!("你是否想用 '{}'？", candidate),
                    &format!("Did you mean '{}'?", candidate),
                );
                suggestions.push(StructuredSuggestion::with_replacement(message, candidate));
            }
        }

        suggestions
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Test classify()
    // ========================================================================

    #[test]
    fn test_classify_division_by_zero() {
        let error = NuzoError::division_by_zero();
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::Arithmetic);
    }

    #[test]
    fn test_classify_arithmetic_overflow() {
        let error = NuzoError::arithmetic_overflow();
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::Arithmetic);
    }

    #[test]
    fn test_classify_type_mismatch() {
        let error = NuzoError::type_mismatch("number", "string");
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::TypeMismatch);
    }

    #[test]
    fn test_classify_index_out_of_bounds() {
        let error = NuzoError::index_out_of_bounds("10", "5");
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::TypeMismatch);
    }

    #[test]
    fn test_classify_assert_failed() {
        let error = NuzoError::assert_failed("value should be positive");
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::Assertion);
    }

    #[test]
    fn test_classify_expected_number() {
        let error = NuzoError::expected_number("nil");
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Error);
        assert_eq!(category, ErrorCategory::TypeMismatch);
    }

    #[test]
    fn test_classify_internal_error() {
        let error = NuzoError::internal(InternalError::NoChunkLoaded, None);
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Fatal);
        assert_eq!(category, ErrorCategory::Internal);
    }

    #[test]
    fn test_classify_internal_error_with_diagnosis() {
        let error = NuzoError::internal(InternalError::InvalidOpcode { opcode: 0xFF }, None);
        let (severity, category) = ErrorClassifier::classify(&error);
        assert_eq!(severity, ErrorSeverity::Fatal);
        assert_eq!(category, ErrorCategory::Internal);
    }

    // ========================================================================
    // Test root_cause()
    // ========================================================================

    #[test]
    fn test_root_cause_stack_overflow() {
        let ie = InternalError::StackOverflow { depth: 256, max_depth: 255 };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("256/255"));
        assert!(cause.contains("Stack overflow"));
        assert!(cause.contains("infinite recursion"));
    }

    #[test]
    fn test_root_cause_stack_underflow() {
        let ie = InternalError::StackUnderflow { operation: "ADD".to_string() };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("ADD"));
        assert!(cause.contains("VM bug"));
        assert!(cause.contains("empty stack"));
    }

    #[test]
    fn test_root_cause_invalid_opcode() {
        let ie = InternalError::InvalidOpcode { opcode: 0xFF };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("0xFF"));
        assert!(cause.contains("compiler bug"));
        assert!(cause.contains("invalid instruction"));
    }

    #[test]
    fn test_root_cause_bytecode_out_of_bounds() {
        let ie = InternalError::BytecodeOutOfBounds { ip: 100, code_len: 50 };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("100")); // Instruction pointer 100
        assert!(cause.contains("50")); // bytecode length 50
        assert!(cause.contains("compiler bug"));
    }

    #[test]
    fn test_root_cause_constant_out_of_bounds() {
        let ie = InternalError::ConstantOutOfBounds { index: 10, pool_size: 5 };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("index 10"));
        assert!(cause.contains("pool size=5"));
        assert!(cause.contains("compiler bug"));
    }

    #[test]
    fn test_root_cause_no_chunk_loaded() {
        let ie = InternalError::NoChunkLoaded;
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("No chunk loaded"));
        assert!(cause.contains("usage error")); // Changed from "VM bug" to "usage error"
    }

    #[test]
    fn test_root_cause_register_out_of_bounds() {
        let ie = InternalError::RegisterOutOfBounds { reg: 16, available: 8 };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("r16"));
        assert!(cause.contains("available=8"));
        assert!(cause.contains("compiler bug"));
    }

    #[test]
    fn test_root_cause_jump_target_out_of_bounds() {
        let ie = InternalError::JumpTargetOutOfBounds { target: 200, code_len: 100 };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("target 200"));
        assert!(cause.contains("code length=100"));
        assert!(cause.contains("compiler bug"));
    }

    #[test]
    fn test_root_cause_compiler_bug() {
        let ie = InternalError::CompilerBug { message: "bad AST node".to_string() };
        let cause = ErrorClassifier::root_cause(&ie);
        assert!(cause.contains("bad AST node"));
    }

    // ========================================================================
    // Test fix_suggestion()
    // ========================================================================

    #[test]
    fn test_fix_suggestion_division_by_zero() {
        let pe = NuzoError::division_by_zero();
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("zero divisors"));
        assert!(suggestion.contains("if divisor != 0"));
    }

    #[test]
    fn test_fix_suggestion_arithmetic_overflow() {
        let pe = NuzoError::arithmetic_overflow();
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("integer range"));
        assert!(suggestion.contains("floating-point"));
    }

    #[test]
    fn test_fix_suggestion_type_mismatch() {
        let pe = NuzoError::type_mismatch("number", "string");
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("Expected number"));
        assert!(suggestion.contains("got string"));
    }

    #[test]
    fn test_fix_suggestion_index_out_of_bounds() {
        let pe = NuzoError::index_out_of_bounds("10", "5");
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("Index 10"));
        assert!(suggestion.contains("length=5"));
        assert!(suggestion.contains("if idx < len(arr)"));
    }

    #[test]
    fn test_fix_suggestion_assert_failed() {
        let pe = NuzoError::assert_failed("value should be positive");
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("value should be positive"));
        assert!(suggestion.contains("Assertion failed"));
    }

    #[test]
    fn test_fix_suggestion_expected_number() {
        let pe = NuzoError::expected_number("nil");
        let suggestion = ErrorClassifier::fix_suggestion(&pe);
        assert!(suggestion.contains("Expected a number"));
        assert!(suggestion.contains("got nil"));
    }

    // ========================================================================
    // Test language-aware variants (_with_lang)
    // ========================================================================

    #[test]
    fn test_fix_suggestion_with_lang_zh_division_by_zero() {
        let pe = NuzoError::division_by_zero();
        let suggestion = ErrorClassifier::fix_suggestion_with_lang(&pe, LangMode::Zh);
        assert!(
            suggestion.contains("除法前确保除数不为零"),
            "Zh 建议应包含中文关键词: {}",
            suggestion
        );
    }

    #[test]
    fn test_fix_suggestion_with_lang_zh_type_mismatch() {
        let pe = NuzoError::type_mismatch("number", "string");
        let suggestion = ErrorClassifier::fix_suggestion_with_lang(&pe, LangMode::Zh);
        assert!(suggestion.contains("期望 number"), "Zh 建议应包含插值: {}", suggestion);
        assert!(suggestion.contains("但得到 string"), "Zh 建议应包含插值: {}", suggestion);
    }

    #[test]
    fn test_fix_suggestion_with_lang_en_matches_old() {
        // En 模式应与旧 fix_suggestion 行为一致（向后兼容）
        let pe = NuzoError::division_by_zero();
        let old = ErrorClassifier::fix_suggestion(&pe);
        let new = ErrorClassifier::fix_suggestion_with_lang(&pe, LangMode::En);
        assert_eq!(old, new, "En 模式应与旧函数返回相同结果");
    }

    #[test]
    fn test_generate_structured_suggestions_with_lang_zh() {
        let pe = NuzoError::index_out_of_bounds("10", "5");
        let suggestions =
            ErrorClassifier::generate_structured_suggestions_with_lang(&pe, LangMode::Zh);
        assert!(!suggestions.is_empty(), "应至少返回一条建议");
        assert!(
            suggestions[0].message.contains("访问前检查索引"),
            "首条建议应为中文: {}",
            suggestions[0].message
        );
    }

    #[test]
    fn test_generate_structured_suggestions_with_lang_zh_has_replacement() {
        let pe = NuzoError::division_by_zero();
        let suggestions =
            ErrorClassifier::generate_structured_suggestions_with_lang(&pe, LangMode::Zh);
        let has_replacement = suggestions.iter().any(|s| s.replacement.is_some());
        assert!(has_replacement, "应至少有一条带 replacement 的建议");
    }

    #[test]
    fn test_generate_structured_suggestions_with_lang_both_bilingual() {
        let pe = NuzoError::undefined_variable("foo");
        let suggestions =
            ErrorClassifier::generate_structured_suggestions_with_lang(&pe, LangMode::Both);
        assert!(!suggestions.is_empty());
        assert!(
            suggestions[0].message.contains("使用前定义变量"),
            "Both 模式应包含中文: {}",
            suggestions[0].message
        );
        assert!(
            suggestions[0].message.contains("define the variable"),
            "Both 模式应包含英文: {}",
            suggestions[0].message
        );
    }

    // ========================================================================
    // Test generate_structured_suggestions_with_candidates (拼写纠错)
    // ========================================================================

    #[test]
    fn test_candidates_no_candidates_falls_back_to_base() {
        // 无候选列表时退回到基础建议，不应包含 "Did you mean"
        let err = NuzoError::undefined_variable("conut");
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::En,
            &[],
        );
        assert!(!suggestions.is_empty(), "应至少有基础建议");
        assert!(
            !suggestions.iter().any(|s| s.message.contains("Did you mean")),
            "无候选列表不应生成拼写纠错建议"
        );
    }

    #[test]
    fn test_candidates_undefined_variable_typo_en() {
        // 经典拼写错误："conut" → "count"
        let err = NuzoError::undefined_variable("conut");
        let candidates = vec!["count".to_string(), "counter".to_string(), "total".to_string()];
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::En,
            &candidates,
        );
        let did_you_mean: Vec<_> =
            suggestions.iter().filter(|s| s.message.contains("Did you mean")).collect();
        assert!(!did_you_mean.is_empty(), "应至少有一条 'Did you mean' 建议");
        // 第一条应是距离最近的 "count"
        assert!(
            did_you_mean[0].message.contains("count"),
            "首条建议应包含 'count': {}",
            did_you_mean[0].message
        );
        assert_eq!(
            did_you_mean[0].replacement.as_deref(),
            Some("count"),
            "replacement 字段应为 'count'"
        );
    }

    #[test]
    fn test_candidates_undefined_variable_typo_zh() {
        // 中文模式下应输出 "你是否想用"
        let err = NuzoError::undefined_variable("conut");
        let candidates = vec!["count".to_string()];
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::Zh,
            &candidates,
        );
        assert!(
            suggestions.iter().any(|s| s.message.contains("你是否想用 'count'")),
            "Zh 模式应包含中文纠错建议"
        );
    }

    #[test]
    fn test_candidates_non_undefined_variable_ignored() {
        // 非 UndefinedVariable 错误即使有候选列表也不应生成拼写纠错建议
        let err = NuzoError::division_by_zero();
        let candidates = vec!["count".to_string()];
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::En,
            &candidates,
        );
        assert!(
            !suggestions.iter().any(|s| s.message.contains("Did you mean")),
            "非 UndefinedVariable 错误不应生成拼写纠错建议"
        );
    }

    #[test]
    fn test_candidates_max_distance_filter() {
        // 距离过大的候选应被过滤
        // "abc" 长度 3, default_max_distance = 1
        // "xyz" 距离 3 > 1, 应被过滤
        // "abd" 距离 1, 应被保留
        let err = NuzoError::undefined_variable("abc");
        let candidates = vec!["abd".to_string(), "xyz".to_string(), "abcdef".to_string()];
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::En,
            &candidates,
        );
        let did_you_mean: Vec<_> =
            suggestions.iter().filter(|s| s.message.contains("Did you mean")).collect();
        assert_eq!(did_you_mean.len(), 1, "只有 'abd' 应被保留");
        assert!(did_you_mean[0].message.contains("abd"));
    }

    #[test]
    fn test_candidates_max_three_suggestions() {
        // 即使有多个相似候选，最多返回 3 条
        let err = NuzoError::undefined_variable("for");
        let candidates = vec![
            "for1".to_string(),
            "for2".to_string(),
            "for3".to_string(),
            "for4".to_string(),
            "for5".to_string(),
        ];
        let suggestions = ErrorClassifier::generate_structured_suggestions_with_candidates(
            &err,
            LangMode::En,
            &candidates,
        );
        let did_you_mean_count =
            suggestions.iter().filter(|s| s.message.contains("Did you mean")).count();
        assert!(did_you_mean_count <= 3, "拼写纠错建议最多 3 条，实际: {}", did_you_mean_count);
    }
}

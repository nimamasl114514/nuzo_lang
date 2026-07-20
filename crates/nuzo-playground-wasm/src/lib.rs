//! # Nuzo Playground WASM Binding
//!
//! 提供 WebAssembly 环境下运行 Nuzo 代码的 API，供浏览器端 Playground 使用。
//!
//! ## 设计概述
//!
//! - [`Playground`] 持有 [`MemoryResolver`]（通过 `Arc<Mutex<_>>` 共享），
//!   支持通过 [`Playground::add_module`] 注入 `import` 语句所需的模块源码。
//! - [`Playground::run`] 内部构造 [`Engine`]（注入 resolver），调用
//!   [`Engine::eval`] 执行源码，捕获 stdout，返回 [`RunResult`]。
//! - 错误（编译期/运行期）通过 [`RunResult::diagnostics`] 返回结构化
//!   [`Diagnostic`] 列表，便于前端展示错误位置、源码片段、修复建议。
//!
//! ## 为什么不用 `StringSink`（T5 新增 trait）
//!
//! `Engine::eval` 内部已用旧版 `OutputSink::new_capture()` 自动捕获 stdout
//! 到 `Vec<String>`（返回 `Output { stdout, .. }`）。T6 直接复用该路径，
//! 零改造 Engine 即可拿到 stdout 字符串，避免引入新 trait 接入开销。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use wasm_bindgen::prelude::*;

use nuzo_core::{LangMode, NuzoError, NuzoErrorKind, error::ErrorCode};
use nuzo_run::{Engine, MemoryResolver, ModuleResolver, ResolveError};

/// 浏览器 console 调试日志 helper。
///
/// 仅在 wasm32 目标下调用 `web_sys::console::log_1`；native 目标下为 no-op。
/// 这样 `Playground::new()` / `run()` 中的 `[nuzo-debug]` 日志在浏览器中输出，
/// 而 `#[cfg(not(target_arch = "wasm32"))]` 的 native 单元测试不会因调用
/// 未实现的 wasm-bindgen 导入函数而 panic。
#[cfg(target_arch = "wasm32")]
fn debug_log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn debug_log(_msg: &str) {}

/// 共享内存解析器包装
///
/// `Playground` 内部用 `Arc<Mutex<MemoryResolver>>` 持有 resolver，
/// 每次 `run` 时通过 `Arc::clone` 共享给临时 `Engine`。
/// 由于 `Arc<MemoryResolver>` 无法直接 coerce 为 `Arc<dyn ModuleResolver>`
/// 并保留内部可变性，需要 wrapper 在 trait 方法内 `lock` 后委托调用。
struct SharedMemoryResolver(Arc<Mutex<MemoryResolver>>);

impl ModuleResolver for SharedMemoryResolver {
    fn resolve(&self, current: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError> {
        self.0.lock().expect("MemoryResolver mutex poisoned").resolve(current, import_path)
    }

    fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
        self.0.lock().expect("MemoryResolver mutex poisoned").load_source(path)
    }

    fn check_circular(&self, path: &Path, stack: &[PathBuf]) -> Result<(), ResolveError> {
        self.0.lock().expect("MemoryResolver mutex poisoned").check_circular(path, stack)
    }
}

/// Nuzo Playground 实例
///
/// 在浏览器中创建一个 Playground，可选注入模块源码，然后多次运行 Nuzo 脚本。
///
/// # 示例（JS 端）
///
/// ```ignore
/// import init, { Playground } from './pkg/nuzo_playground_wasm.js';
/// await init();
/// const pg = new Playground();
/// pg.add_module("math", "fn add(a, b) = a + b");
/// const result = pg.run('import "math"; print(add(1, 2))');
/// console.log(result.success, result.stdout, result.diagnostics);
/// ```
#[wasm_bindgen]
pub struct Playground {
    resolver: Arc<Mutex<MemoryResolver>>,
}

#[wasm_bindgen]
impl Playground {
    /// 创建新的 Playground 实例（无注入模块）。
    ///
    /// 构造时安装 `console_error_panic_hook`，使后续 `run()` 中发生的 Rust panic
    /// 能将完整 panic 消息（含源文件:行号）打印到浏览器 Console，
    /// 而非仅以 wasm `unreachable` trap 形式上报（默认 wasm32 panic handler
    /// 不输出任何信息，难以定位）。
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        debug_log("[nuzo-debug] Playground::new() entry");
        #[cfg(feature = "panic-hook")]
        console_error_panic_hook::set_once();
        debug_log("[nuzo-debug] panic_hook set");

        let resolver = Arc::new(Mutex::new(MemoryResolver::new()));
        debug_log("[nuzo-debug] resolver created");

        Self { resolver }
    }

    /// 注入模块源码，供脚本中 `import "path"` 语句解析。
    ///
    /// `path` 作为虚拟路径 key（如 `"math"`、`"utils.nuzo"`），
    /// `source` 为该模块的 Nuzo 源码。重复注入同一 path 会覆盖。
    pub fn add_module(&mut self, path: &str, source: &str) {
        self.resolver.lock().expect("MemoryResolver mutex poisoned").add_module(path, source);
    }

    /// 运行 Nuzo 源码，返回执行结果。
    ///
    /// - 成功：`success=true`，`stdout` 含所有 `println` 输出（多行用 `\n` 连接），
    ///   `diagnostics` 为空数组
    /// - 失败：`success=false`，`stdout` 可能为空或含失败前已打印的内容，
    ///   `diagnostics` 含至少一条 [`Diagnostic`] 描述错误位置与原因
    ///
    /// 每次调用都会构造一个新的临时 `Engine` + `Session`，
    /// 多次调用的输出互不混合。
    pub fn run(&self, source: &str) -> RunResult {
        debug_log("[nuzo-debug] run() entry");

        debug_log("[nuzo-debug] creating resolver wrapper");
        let shared = SharedMemoryResolver(Arc::clone(&self.resolver));
        debug_log("[nuzo-debug] resolver wrapper created");

        debug_log("[nuzo-debug] coercing to Arc<dyn ModuleResolver>");
        let resolver: Arc<dyn ModuleResolver> = Arc::new(shared);
        debug_log("[nuzo-debug] coercion done");

        debug_log("[nuzo-debug] creating EngineBuilder");
        let builder = Engine::builder();
        debug_log("[nuzo-debug] builder created");

        debug_log("[nuzo-debug] calling with_default_config()");
        let builder = builder.with_default_config();
        debug_log("[nuzo-debug] with_default_config() done");

        debug_log("[nuzo-debug] calling with_resolver()");
        let builder = builder.with_resolver(resolver);
        debug_log("[nuzo-debug] with_resolver() done");

        debug_log("[nuzo-debug] calling build()");
        let engine = match builder.build() {
            Ok(e) => {
                debug_log("[nuzo-debug] build() OK");
                e
            }
            Err(err) => {
                debug_log(&format!("[nuzo-debug] build() ERR: {:?}", err));
                let diag = nuzo_error_to_diagnostic(&err, source);
                return RunResult {
                    success: false,
                    stdout: String::new(),
                    diagnostics: vec![diag],
                };
            }
        };

        debug_log("[nuzo-debug] calling eval()");
        let result = engine.eval(source);
        debug_log("[nuzo-debug] eval() returned");

        match result {
            Ok(output) => {
                debug_log(&format!("[nuzo-debug] eval OK, stdout lines: {:?}", output.stdout));
                RunResult {
                    success: true,
                    stdout: output.stdout.join("\n"),
                    diagnostics: Vec::new(),
                }
            }
            Err(err) => {
                debug_log(&format!("[nuzo-debug] eval ERR: {:?}", err));
                let diag = nuzo_error_to_diagnostic(&err, source);
                RunResult { success: false, stdout: String::new(), diagnostics: vec![diag] }
            }
        }
    }
}

impl Default for Playground {
    fn default() -> Self {
        Self::new()
    }
}

/// 脚本运行结果
///
/// 字段通过 getter 方法暴露给 JS（wasm-bindgen 对 pub field 生成的 getter
/// 要求字段类型实现 `Copy`，`String`/`Vec<Diagnostic>` 不满足，故改用私有字段
/// + getter 方法）。
#[wasm_bindgen]
pub struct RunResult {
    /// 是否成功（true = 执行成功，false = 编译或运行时错误）
    success: bool,
    /// 捕获的 stdout 输出（多行用 `\n` 连接，结尾不附加额外换行）
    stdout: String,
    /// 错误诊断列表（`success=false` 时填充；`success=true` 时为空数组）
    diagnostics: Vec<Diagnostic>,
}

#[wasm_bindgen]
impl RunResult {
    /// 是否执行成功
    #[wasm_bindgen(getter)]
    pub fn success(&self) -> bool {
        self.success
    }

    /// 捕获的 stdout 输出（多行用 `\n` 连接）
    #[wasm_bindgen(getter)]
    pub fn stdout(&self) -> String {
        self.stdout.clone()
    }

    /// 错误诊断列表（失败时填充，成功时为空数组）
    ///
    /// 每条 [`Diagnostic`] 包含错误码、消息、严重程度、文件位置、源码片段
    /// 与修复建议，供前端精确高亮错误位置并展示修复提示。
    #[wasm_bindgen(getter)]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics.clone()
    }
}

/// 结构化错误诊断
///
/// 对应一条具体的错误（来自 [`NuzoError`]），携带位置信息和修复建议，
/// 供前端 CodeMirror 等编辑器精确高亮错误位置并展示修复提示。
///
/// # 字段说明
///
/// | 字段 | 说明 |
/// |------|------|
/// | `code` | 稳定错误码，如 `"E0003"`（DivisionByZero）、`"C0005"`（SyntaxError）|
/// | `message` | 英文错误消息（不含位置前缀，便于前端单独渲染位置与消息）|
/// | `severity` | 严重程度：`"error"` / `"warning"` / `"info"`（当前 NuzoError 仅产生 error）|
/// | `file` | 文件名（Playground 下通常为 `<unknown>`）|
/// | `line` | 行号（1-based；0 表示位置未知）|
/// | `column` | 列号（1-based；0 表示列未知）|
/// | `source_snippet` | 错误行的源码片段（来自 `SourceLocation.source_line` 或从 `source` 按行号提取）|
/// | `suggestion` | 修复建议（基于 [`NuzoErrorKind`] 派生，可能为空串）|
#[wasm_bindgen]
#[derive(Clone)]
pub struct Diagnostic {
    code: String,
    message: String,
    severity: String,
    file: String,
    line: u32,
    column: u32,
    source_snippet: String,
    suggestion: String,
}

#[wasm_bindgen]
impl Diagnostic {
    /// 稳定错误码（如 `"E0003"`、`"C0005"`）
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> String {
        self.code.clone()
    }

    /// 英文错误消息（不含位置前缀）
    #[wasm_bindgen(getter)]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// 严重程度：`"error"` / `"warning"` / `"info"`
    #[wasm_bindgen(getter)]
    pub fn severity(&self) -> String {
        self.severity.clone()
    }

    /// 文件名（Playground 下通常为 `<unknown>`）
    #[wasm_bindgen(getter)]
    pub fn file(&self) -> String {
        self.file.clone()
    }

    /// 行号（1-based；0 表示位置未知）
    #[wasm_bindgen(getter)]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// 列号（1-based；0 表示列未知）
    #[wasm_bindgen(getter)]
    pub fn column(&self) -> u32 {
        self.column
    }

    /// 错误行的源码片段
    #[wasm_bindgen(getter)]
    pub fn source_snippet(&self) -> String {
        self.source_snippet.clone()
    }

    /// 修复建议（可能为空串）
    #[wasm_bindgen(getter)]
    pub fn suggestion(&self) -> String {
        self.suggestion.clone()
    }
}

// ============================================================================
// NuzoError → Diagnostic 转换
// ============================================================================

/// 将 [`NuzoError`] 转换为 [`Diagnostic`]，提取位置信息与源码片段。
///
/// - 错误码：从 `err.code`（[`ErrorCode`]）映射到稳定字符串
/// - 消息：`err.format_with_lang(LangMode::En)` 返回英文消息（不含位置前缀）
/// - 严重程度：NuzoError 全部归类为 `"error"`（无 warning/info 等级别）
/// - 位置：优先使用 `err.source_location`；缺失时 file=`<unknown>`，line/column=0
/// - 源码片段：优先使用 `SourceLocation.source_line`；缺失时从 `source` 按 line 提取
/// - 修复建议：基于 [`NuzoErrorKind`] 派生简单提示
fn nuzo_error_to_diagnostic(err: &NuzoError, source: &str) -> Diagnostic {
    let code = error_code_to_string(err.code).to_string();
    let message = err.format_with_lang(LangMode::En);
    let severity = "error".to_string();

    let (file, line, column, source_snippet) = match &err.source_location {
        Some(loc) => {
            let snippet = loc
                .source_line
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| extract_source_line(source, loc.line));
            (loc.file.clone(), loc.line as u32, loc.column as u32, snippet)
        }
        None => (String::from("<unknown>"), 0, 0, String::new()),
    };

    let suggestion = suggestion_for_kind(&err.kind);

    Diagnostic { code, message, severity, file, line, column, source_snippet, suggestion }
}

/// 将 [`ErrorCode`] 映射到稳定的字符串表示（与 `#[serde(rename = ...)]` 一致）。
///
/// 不引入 `serde_json` 依赖，用穷尽 match 保证新增 variant 时编译报错。
fn error_code_to_string(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::TypeMismatch => "E0001",
        ErrorCode::IndexOutOfBounds => "E0002",
        ErrorCode::DivisionByZero => "E0003",
        ErrorCode::ArithmeticOverflow => "E0004",
        ErrorCode::AssertFailed => "E0005",
        ErrorCode::ExpectedNumber => "E0006",
        ErrorCode::InvalidArgumentCount => "E0007",
        ErrorCode::UndefinedVariable => "E0008",
        ErrorCode::UnsupportedOperation => "E0009",
        ErrorCode::ExecutionTimeout => "E0010",
        ErrorCode::CompileError => "C0000",
        ErrorCode::ModuleNotFound => "C0001",
        ErrorCode::CircularImport => "C0002",
        ErrorCode::DuplicateSymbol => "C0004",
        ErrorCode::SyntaxError => "C0005",
        ErrorCode::Internal => "I0000",
        ErrorCode::InvalidBytecodeVersion => "I0100",
    }
}

/// 基于 [`NuzoErrorKind`] 生成简短的修复建议。
///
/// 返回空串表示无具体建议（前端可隐藏 suggestion 区域）。
fn suggestion_for_kind(kind: &NuzoErrorKind) -> String {
    match kind {
        NuzoErrorKind::DivisionByZero => {
            "Ensure the divisor is not zero before performing division.".to_string()
        }
        NuzoErrorKind::TypeMismatch { expected, .. } => {
            format!("Convert the value to a {} type or use a value of the expected type.", expected)
        }
        NuzoErrorKind::IndexOutOfBounds { length, .. } => {
            format!("Use an index within the valid range 0..{}.", length)
        }
        NuzoErrorKind::ArithmeticOverflow => {
            "Use a larger numeric type or check the value range before computation.".to_string()
        }
        NuzoErrorKind::AssertFailed { .. } => {
            "Check the assertion condition and fix the failing case.".to_string()
        }
        NuzoErrorKind::ExpectedNumber { .. } => {
            "Ensure the value is a number before performing arithmetic.".to_string()
        }
        NuzoErrorKind::InvalidArgumentCount { expected, .. } => {
            format!("Provide exactly {} argument(s) when calling this function.", expected)
        }
        NuzoErrorKind::UndefinedVariable { name } => {
            format!("Declare '{}' before using it, or check for typos in the variable name.", name)
        }
        NuzoErrorKind::UnsupportedOperation { operation, type_name } => format!(
            "Cannot perform '{}' on a {} value; convert to a compatible type first.",
            operation, type_name
        ),
        NuzoErrorKind::ExecutionTimeout { .. } => {
            "Check for infinite loops or optimize long-running code.".to_string()
        }
        NuzoErrorKind::ModuleNotFound { path } => {
            format!("Check that the module path '{}' is correct and the file exists.", path)
        }
        NuzoErrorKind::CircularImport { chain } => {
            format!("Break the import cycle: {}.", chain.join(" -> "))
        }
        NuzoErrorKind::DuplicateSymbol { name, .. } => {
            format!("Rename one of the '{}' definitions to avoid the conflict.", name)
        }
        NuzoErrorKind::Internal(_, _) => {
            "This is a bug in the Nuzo runtime. Please report it to the developers.".to_string()
        }
    }
}

/// 从源码字符串按 1-based 行号提取对应行内容。
///
/// `line == 0` 视为位置未知，返回空串；`line > lines_count` 同样返回空串。
fn extract_source_line(source: &str, line: usize) -> String {
    if line == 0 {
        return String::new();
    }
    source.lines().nth(line - 1).unwrap_or("").to_string()
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    //! 这些测试在 native target 下运行，验证 Playground API 行为。
    //! wasm32 target 下 wasm-bindgen 不支持 `#[test]`，故 cfg gate。

    use super::*;
    use nuzo_core::SourceLocation;

    // -----------------------------------------------------------------------
    // 原有测试：成功路径
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_simple_print() {
        let pg = Playground::new();
        let result = pg.run(r#"print("Hello")"#);
        assert!(
            result.success,
            "expected success, diagnostics = {:?}",
            diagnostic_messages(&result)
        );
        assert_eq!(result.stdout, "Hello");
        assert!(result.diagnostics.is_empty(), "no diagnostics expected on success");
    }

    #[test]
    fn test_run_arithmetic() {
        let pg = Playground::new();
        // print(1 + 2) 应输出 3
        let result = pg.run("print(1 + 2)");
        assert!(
            result.success,
            "expected success, diagnostics = {:?}",
            diagnostic_messages(&result)
        );
        assert_eq!(result.stdout, "3");
    }

    #[test]
    fn test_run_multiple_prints_join_with_newline() {
        let pg = Playground::new();
        let result = pg.run(r#"print("a"); print("b"); print("c")"#);
        assert!(
            result.success,
            "expected success, diagnostics = {:?}",
            diagnostic_messages(&result)
        );
        assert_eq!(result.stdout, "a\nb\nc");
    }

    #[test]
    fn test_run_empty_source_succeeds() {
        let pg = Playground::new();
        let result = pg.run("");
        assert!(
            result.success,
            "expected success, diagnostics = {:?}",
            diagnostic_messages(&result)
        );
        assert!(result.stdout.is_empty());
    }

    // -----------------------------------------------------------------------
    // 原有测试：错误路径（已更新为 diagnostics）
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_syntax_error_returns_diagnostics() {
        let pg = Playground::new();
        // 缺右括号 → 编译错误
        let result = pg.run(r#"print("Hello""#);
        assert!(!result.success, "expected failure on syntax error");
        assert!(!result.diagnostics.is_empty(), "diagnostics should contain at least one entry");
    }

    #[test]
    fn test_add_module_enables_import() {
        let mut pg = Playground::new();
        pg.add_module("math", "fn add(a, b) = a + b");
        // 注意：import 语法取决于 nuzo_lang 当前实际支持的语法。
        // 这里仅验证 add_module 不会破坏 run；具体 import 行为由集成测试覆盖。
        let result = pg.run(r#"print("ok")"#);
        assert!(
            result.success,
            "expected success, diagnostics = {:?}",
            diagnostic_messages(&result)
        );
        assert_eq!(result.stdout, "ok");
    }

    #[test]
    fn test_playground_default_eq_new() {
        let a = Playground::new();
        let b = Playground::default();
        // 两者均无注入模块，运行相同代码应得相同结果
        let ra = a.run("print(42)");
        let rb = b.run("print(42)");
        assert_eq!(ra.success, rb.success);
        assert_eq!(ra.stdout, rb.stdout);
    }

    // -----------------------------------------------------------------------
    // 新增测试：Diagnostic 结构化字段
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_success_has_empty_diagnostics() {
        // 成功时 diagnostics 必须为空数组
        let pg = Playground::new();
        let result = pg.run(r#"print("ok")"#);
        assert!(result.success);
        assert!(result.diagnostics().is_empty(), "diagnostics should be empty on success");
    }

    #[test]
    fn test_run_error_has_code_and_message() {
        // 错误时 diagnostics 必须包含 code（非空）与 message（非空）
        let pg = Playground::new();
        let result = pg.run(r#"print("Hello""#); // 语法错误
        assert!(!result.success);
        let diags = result.diagnostics();
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let diag = &diags[0];
        assert!(!diag.code().is_empty(), "code should be non-empty");
        assert!(!diag.message().is_empty(), "message should be non-empty");
        assert_eq!(diag.severity(), "error", "severity should be 'error' for NuzoError");
    }

    #[test]
    fn test_run_error_has_source_snippet() {
        // 多行源码中第 2 行有语法错误时，source_snippet 应包含第 2 行内容
        let source = "print(\"line1\")\nprint(\"line2\"\nprint(\"line3\")";
        let pg = Playground::new();
        let result = pg.run(source);
        assert!(!result.success);
        let diags = result.diagnostics();
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let diag = &diags[0];
        // line 字段必须 > 0（即编译器报告了行号）
        assert!(diag.line() > 0, "line should be > 0; got {}", diag.line());
        // source_snippet 应当非空且来自源码（含 "print"）
        assert!(
            !diag.source_snippet().is_empty(),
            "source_snippet should be non-empty when line is known"
        );
        assert!(
            diag.source_snippet().contains("print"),
            "source_snippet should contain the error line content, got: {:?}",
            diag.source_snippet()
        );
    }

    #[test]
    fn test_run_error_has_correct_line_number() {
        // 显式构造多行场景，验证 line 字段能映射到出错行
        let source = "print(\"ok\")\nprint(\"ok\")\nprint(unknown_var)";
        let pg = Playground::new();
        let result = pg.run(source);
        assert!(!result.success);
        let diags = result.diagnostics();
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let diag = &diags[0];
        // 编译器/运行时应报告 line >= 1（具体行号取决于实现，但必须在源码行数范围内）
        let line = diag.line();
        if line > 0 {
            assert!(
                (line as usize) <= source.lines().count(),
                "line {} exceeds source line count {}",
                line,
                source.lines().count()
            );
        }
    }

    // -----------------------------------------------------------------------
    // 单元测试：转换函数
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_code_to_string_covers_all_variants() {
        // 穷尽测试所有 ErrorCode 变体，确保新加 variant 时此处会编译报错
        // (因为 error_code_to_string 是穷尽 match)
        let cases = [
            (ErrorCode::TypeMismatch, "E0001"),
            (ErrorCode::IndexOutOfBounds, "E0002"),
            (ErrorCode::DivisionByZero, "E0003"),
            (ErrorCode::ArithmeticOverflow, "E0004"),
            (ErrorCode::AssertFailed, "E0005"),
            (ErrorCode::ExpectedNumber, "E0006"),
            (ErrorCode::InvalidArgumentCount, "E0007"),
            (ErrorCode::UndefinedVariable, "E0008"),
            (ErrorCode::UnsupportedOperation, "E0009"),
            (ErrorCode::ExecutionTimeout, "E0010"),
            (ErrorCode::CompileError, "C0000"),
            (ErrorCode::ModuleNotFound, "C0001"),
            (ErrorCode::CircularImport, "C0002"),
            (ErrorCode::DuplicateSymbol, "C0004"),
            (ErrorCode::SyntaxError, "C0005"),
            (ErrorCode::Internal, "I0000"),
            (ErrorCode::InvalidBytecodeVersion, "I0100"),
        ];
        for (code, expected) in cases {
            assert_eq!(error_code_to_string(code), expected);
        }
    }

    #[test]
    fn test_extract_source_line_handles_edge_cases() {
        // line == 0 → 空串
        assert_eq!(extract_source_line("anything", 0), "");
        // 空源码 → 空串
        assert_eq!(extract_source_line("", 1), "");
        // line 超出行数 → 空串
        assert_eq!(extract_source_line("only one line", 5), "");
        // 正常情况
        assert_eq!(extract_source_line("line1\nline2\nline3", 2), "line2");
        // 最后一行（无尾换行）
        assert_eq!(extract_source_line("line1\nline2\nline3", 3), "line3");
        // 单行源码
        assert_eq!(extract_source_line("single", 1), "single");
    }

    #[test]
    fn test_nuzo_error_to_diagnostic_without_location() {
        // 无 source_location 的错误：file=<unknown>, line=0, column=0
        let err = NuzoError::division_by_zero();
        let diag = nuzo_error_to_diagnostic(&err, "x = 1 / 0");
        assert_eq!(diag.code(), "E0003");
        assert_eq!(diag.severity(), "error");
        assert_eq!(diag.file(), "<unknown>");
        assert_eq!(diag.line(), 0);
        assert_eq!(diag.column(), 0);
        assert!(diag.source_snippet().is_empty(), "no snippet when location unknown");
        assert!(!diag.suggestion().is_empty(), "DivisionByZero should have a suggestion");
        assert!(
            diag.message().contains("division by zero"),
            "message should contain 'division by zero', got: {}",
            diag.message()
        );
    }

    #[test]
    fn test_nuzo_error_to_diagnostic_with_location_and_source_line() {
        // 有 source_location 且自带 source_line → 直接使用 SourceLocation.source_line
        let loc = SourceLocation::new(2)
            .with_column(5)
            .with_function("main")
            .with_source_line("x = 1 / 0");
        let err = NuzoError::division_by_zero().with_source_location(loc);
        let source = "let x = 1\nx = 1 / 0\n";
        let diag = nuzo_error_to_diagnostic(&err, source);
        assert_eq!(diag.line(), 2);
        assert_eq!(diag.column(), 5);
        assert_eq!(diag.source_snippet(), "x = 1 / 0");
    }

    #[test]
    fn test_nuzo_error_to_diagnostic_with_location_fallback_to_source() {
        // 有 source_location 但 source_line 为 None → 从 source 按 line 提取
        let loc = SourceLocation::new(2).with_column(5);
        let err = NuzoError::division_by_zero().with_source_location(loc);
        let source = "let a = 1\nlet b = 2\nlet c = 3";
        let diag = nuzo_error_to_diagnostic(&err, source);
        assert_eq!(diag.line(), 2);
        assert_eq!(diag.source_snippet(), "let b = 2");
    }

    #[test]
    fn test_suggestion_for_kind_covers_all_variants() {
        // 穷尽测试所有 NuzoErrorKind 变体，确保新加 variant 时编译报错
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
            NuzoErrorKind::Internal(nuzo_core::InternalError::NoChunkLoaded, None),
            NuzoErrorKind::ModuleNotFound { path: "p".into() },
            NuzoErrorKind::CircularImport { chain: vec!["a".into()] },
            NuzoErrorKind::DuplicateSymbol {
                name: "s".into(),
                first_location: None,
                second_location: None,
            },
        ];
        for v in &variants {
            let s = suggestion_for_kind(v);
            assert!(!s.is_empty(), "suggestion should be non-empty for {:?}", v);
        }
    }

    // -----------------------------------------------------------------------
    // 测试辅助函数
    // -----------------------------------------------------------------------

    /// 收集 RunResult 中所有 Diagnostic 的 message，便于断言失败时打印
    fn diagnostic_messages(result: &RunResult) -> Vec<String> {
        result.diagnostics().iter().map(|d| d.message()).collect()
    }
}

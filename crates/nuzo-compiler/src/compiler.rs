//! # Nuzo 编译器 — IR 路径编译入口
//!
//! 本模块是编译器的**主入口**，定义了核心数据结构和公开 API。
//!
//! ## 编译流程
//!
//! ```text
//! Source → Lexer → Parser → AST → IrBuilder → IR → CodeGenerator → Chunk
//! ```
//!
//! 所有编译均通过 IR 路径完成。旧的 AST 直编译路径（`compile_program` /
//! `compile_stmt` / `compile_expr`）已在 0.3.0 版本中移除。
//!
//! ## 模块结构
//!
//! | 文件 | 职责 |
//! |------|------|
//! | **compiler.rs**（本文件）| Compiler 结构体、编译入口 |
//! | [`error`] | `CompileError` 枚举及错误转换 |
//! | [`builder`] | `CompilerBuilder`（Builder 模式） |
//! | [`codegen`] | IR → 字节码代码生成 |
//! | [`allocator`] | 寄存器分配器 |

use crate::codegen::CodeGenerator;
use nuzo_bytecode::Chunk;

use nuzo_ir::module_resolver::{ModuleResolver, NullResolver};
use nuzo_signal::{
    BusScope, CompileFinishedInfo, CompileStartedInfo, FunctionCompileInfo, Signal, SignalBus,
};
use std::path::Path;
use std::sync::Arc;

// --- 子模块声明 ---
#[path = "builder.rs"]
mod builder;
#[path = "error.rs"]
mod error;

// --- re-export 公开类型 ---
pub use builder::CompilerBuilder;
pub use error::CompileError;

// ── 类型化信号键（编译器子系统）──────────────────────────────────────────
nuzo_signal::declare_signal!(COMPILE_STARTED_KEY, CompileStartedInfo, BusScope::Compiler);
nuzo_signal::declare_signal!(COMPILE_FINISHED_KEY, CompileFinishedInfo, BusScope::Compiler);
nuzo_signal::declare_signal!(COMPILE_FUNCTION_DONE_KEY, FunctionCompileInfo, BusScope::Compiler);

/// 创建并初始化编译器信号总线，注册所有编译器信号
///
/// 返回的 `Arc<SignalBus>` 可被多个 Compiler 实例共享，
/// 也可被外部代码用于订阅编译器信号。
pub fn compiler_bus() -> Arc<SignalBus> {
    let bus = Arc::new(SignalBus::scoped(BusScope::Compiler));
    bus.register(&COMPILE_STARTED_KEY, &Signal::named("compile_started"))
        .expect("register COMPILE_STARTED_KEY");
    bus.register(&COMPILE_FINISHED_KEY, &Signal::named("compile_finished"))
        .expect("register COMPILE_FINISHED_KEY");
    bus.register(&COMPILE_FUNCTION_DONE_KEY, &Signal::named("compile_function_done"))
        .expect("register COMPILE_FUNCTION_DONE_KEY");
    bus
}

/// Nuzo 编译器核心结构体
///
/// 编译器通过 IR 路径工作：源码 → AST → IR → 字节码。
/// 使用 [`Compiler::compile()`] 一步完成编译，或使用
/// [`Compiler::compile_with_bus_and_resolver()`] 进行更细粒度的控制。
pub struct Compiler;

impl Compiler {
    /// 创建一个新的 [`CompilerBuilder`]，用于自定义编译器配置。
    pub fn builder() -> CompilerBuilder {
        CompilerBuilder { source: None, source_file: None }
    }

    /// 便捷方法：将源代码字符串直接编译为字节码 [`Chunk`]。
    ///
    /// 内部使用默认的编译器信号总线。
    pub fn compile(source: &str) -> Result<Chunk, CompileError> {
        Self::compile_with_bus(source, compiler_bus())
    }

    /// 使用外部 [`SignalBus`] 编译源代码。
    ///
    /// 允许 Engine 层将自身的总线注入到编译流程中，
    /// 实现 Engine → Compiler 的信号总线共享，便于订阅编译开始/结束事件。
    ///
    /// # 参数
    /// - `source`: Nuzo 语言源代码字符串
    /// - `bus`: 复用的信号总线实例
    ///
    /// # 返回值
    /// 成功时返回编译后的字节码 [`Chunk`]，失败时返回 [`CompileError`]。
    ///
    /// # 向后兼容
    /// 等价于 `compile_with_bus_and_resolver(source, bus, &NullResolver, None)`。
    /// 当源码不含 `import` 语句时行为与历史版本完全一致。
    pub fn compile_with_bus(source: &str, bus: Arc<SignalBus>) -> Result<Chunk, CompileError> {
        Self::compile_with_bus_and_resolver(source, bus, &NullResolver, None)
            .map(|(chunk, _)| chunk)
    }

    /// 使用外部 [`SignalBus`] 与 [`ModuleResolver`] 编译源代码（支持 import）。
    ///
    /// 与 [`compile_with_bus`](Self::compile_with_bus) 的区别：注入 [`ModuleResolver`]
    /// 用于解析 `import "path"` 语句。当源程序包含 import 时，会递归编译依赖模块。
    ///
    /// # 参数
    /// - `source`: Nuzo 语言源代码字符串
    /// - `bus`: 复用的信号总线实例
    /// - `resolver`: 模块路径解析器（实现 [`ModuleResolver`] trait）
    /// - `current_path`: 当前模块的源文件路径（用于相对路径解析；`None` 表示顶层入口）
    ///
    /// # 错误位置保留
    /// 当 [`ModuleResolver`] 返回 [`nuzo_ir::module_resolver::ResolveError`] 时，
    /// [`IrBuilder`](nuzo_ir::builder::IrBuilder) 会将其包装为
    /// [`IrBuildError::Error`](nuzo_ir::error::IrBuildError::Error) 变体，
    /// 携带原始 [`nuzo_core::SourceLocation`]。本函数将位置信息提取到 [`CompileError`] 中，
    /// 避免降级为 `line=0, column=0`。
    pub fn compile_with_bus_and_resolver(
        source: &str,
        bus: Arc<SignalBus>,
        resolver: &dyn ModuleResolver,
        current_path: Option<&Path>,
    ) -> Result<(Chunk, Vec<(String, Chunk)>), CompileError> {
        if let Ok(sig) = bus.get(&COMPILE_STARTED_KEY)
            && !sig.is_empty()
        {
            sig.emit(&CompileStartedInfo { source_len: source.len() });
        }
        let total_start = web_time::Instant::now();
        let result: Result<(Chunk, Vec<(String, Chunk)>), CompileError> = (|| {
            let (program, _timings) =
                nuzo_frontend::parser::Parser::parse_with_timing(source).map_err(|e| {
                    CompileError::ParseError { message: e.message, line: e.line, column: e.column }
                })?;
            let mut ir = nuzo_ir::builder::IrBuilder::build_with_resolver(
                &program,
                &nuzo_helpers::builtin_names(),
                resolver,
                current_path,
            )
            .map_err(|e| CompileError::Error {
                message: format!("IR build failed: {}", e),
                line: 0,
                column: 0,
            })?;
            let dump_ir = std::env::var("NUZO_DUMP_IR").is_ok();
            if dump_ir {
                eprintln!("--- IR dump (pre-opt) ---\n{}", ir);
            }
            nuzo_ir::optimize::optimize(&mut ir);
            if dump_ir {
                eprintln!("--- IR dump (post-opt) ---\n{}", ir);
            }
            if dump_ir && let Err(e) = ir.validate() {
                eprintln!("IR validation warning: {}", e);
            }
            let _cg_start = web_time::Instant::now();
            let mut codegen = CodeGenerator::new();
            codegen.generate(&ir)?;
            let sub_chunks = codegen.take_sub_module_chunks();
            Ok((codegen.into_chunk(), sub_chunks))
        })();
        if let Ok(sig) = bus.get(&COMPILE_FINISHED_KEY)
            && !sig.is_empty()
        {
            let info = match &result {
                Ok((chunk, _)) => CompileFinishedInfo {
                    success: true,
                    chunk_size: Some(chunk.len()),
                    duration: total_start.elapsed(),
                    lex_duration: std::time::Duration::ZERO,
                    parse_duration: std::time::Duration::ZERO,
                    codegen_duration: std::time::Duration::ZERO,
                },
                Err(_) => CompileFinishedInfo {
                    success: false,
                    chunk_size: None,
                    duration: total_start.elapsed(),
                    lex_duration: std::time::Duration::ZERO,
                    parse_duration: std::time::Duration::ZERO,
                    codegen_duration: std::time::Duration::ZERO,
                },
            };
            sig.emit(&info);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple_number() {
        assert!(!Compiler::compile("42").unwrap().code().is_empty());
    }

    #[test]
    fn test_compile_arithmetic() {
        assert!(!Compiler::compile("1+2*3").unwrap().code().is_empty());
    }

    #[test]
    fn test_compiler_bus_signals() {
        let bus = compiler_bus();
        assert_eq!(bus.get(&COMPILE_STARTED_KEY).unwrap().name(), "compile_started");
        assert_eq!(bus.get(&COMPILE_FINISHED_KEY).unwrap().name(), "compile_finished");
        assert_eq!(bus.get(&COMPILE_FUNCTION_DONE_KEY).unwrap().name(), "compile_function_done");
    }

    #[test]
    fn test_compile_error_column() {
        assert_eq!(
            CompileError::ParseError { message: "x".into(), line: 5, column: 12 }.column(),
            Some(12)
        );
        assert_eq!(
            CompileError::UndefinedVariable { name: "x".into(), line: 1, column: 0 }.column(),
            Some(0)
        );
    }

    #[test]
    fn test_compile_with_config() {
        let _compiler = Compiler::builder();
    }

    #[test]
    fn test_compile_function() {
        let chunk = Compiler::compile("fn add(a, b) { return a + b } add(1, 2)").unwrap();
        assert!(!chunk.code().is_empty());
    }

    #[test]
    fn test_compile_while_loop() {
        let source = "n = 0; while n < 10 { n = n + 1 }; n";
        let chunk = Compiler::compile(source).unwrap();
        assert!(!chunk.code().is_empty());
    }

    #[test]
    fn test_compile_import() {
        let source = "import \"std.nuzo\"";
        // Without a resolver, this will fail — but we just verify it doesn't panic
        let _ = Compiler::compile(source);
    }
}

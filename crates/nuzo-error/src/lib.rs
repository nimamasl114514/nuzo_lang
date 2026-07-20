//! # Nuzo Error — Nuzo 结构化错误处理体系
//!
//! **层级**: L4（错误与诊断基础设施层）—— 统一全栈错误类型、收集、格式化与分类，为编译期和运行时提供结构化诊断。
//!
//! **主要入口**: [`ErrorCollector`], [`DiagnosticFormatter`], [`DiagnosticRenderer`], [`ErrorClassifier`], [`signal_error_to_nuzo_error`]
//!
//! ## 模块职责
//!
//! | 模块 | 职责 | 入口类型 |
//! |------|------|----------|
//! | [`types`] | 错误类型定义（严重程度/分类/执行上下文） | [`ErrorSeverity`](types::ErrorSeverity), [`ErrorCategory`](types::ErrorCategory), [`ExecutionContext`](types::ExecutionContext) |
//! | [`collector`] | 错误收集器（多错误累积 + 截止策略） | [`ErrorCollector`](collector::ErrorCollector) |
//! | [`diagnostic`] | 执行上下文诊断信息（寄存器快照/调用栈） | [`DiagnosticError`](diagnostic::DiagnosticError) |
//! | [`formatter`] | 错误格式化输出（终端彩色/IDE 友好） | [`DiagnosticFormatter`](formatter::DiagnosticFormatter) |
//! | [`classifier`] | 错误根因分类器 | [`ErrorClassifier`](classifier::ErrorClassifier) |
//! | [`smart_types`] | 智能类型推断辅助 | (内部) |
//!
//! ## 设计理念
//!
//! 三层错误模型：
//! - **InternalError**: VM 内部不变量违反（寄存器越界/GC 失败/字节码非法）
//! - **NuzoError**: 用户可见运行时错误（类型不匹配/未定义变量/除零等）
//! - **CompileError**: 编译期错误（语法错误/作用域冲突/类型检查失败）
//!
//! ## 开发者速查：常见任务 → 代码位置
//!
//! | 任务 | 位置 |
//! |------|------|
//! | "加新错误变体" | `types.rs: ErrorSeverity 或 ErrorCategory 枚举` |
//! | "改错误消息格式" | `formatter.rs: DiagnosticFormatter` |
//! | "改错误收集策略" | `collector.rs: ErrorCollector` |

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 4, description = "错误诊断与渲染", entry_type = "Diagnostic")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod classifier;
pub mod collector;
pub mod diagnostic;
pub mod formatter;
pub mod renderer;
pub mod similar;
pub mod sink;
pub mod smart_types;
pub mod types;

// --- types: 核心错误类型 ---
pub use types::{
    ErrorCategory, ErrorSeverity, ExecutionContext, StackFrameInfo, StructuredSuggestion,
};

// --- collector: 错误收集器 ---
pub use collector::ErrorCollector;

// --- sink: VM → Collector 事件流通道 ---
pub use sink::{ErrorEvent, ErrorSink};

// --- diagnostic: 执行上下文诊断 ---
pub use diagnostic::DiagnosticError;

// --- formatter: 格式化输出 ---
pub use formatter::DiagnosticFormatter;

// --- renderer: 统一诊断渲染 ---
pub use renderer::DiagnosticRenderer;

// --- classifier: 根因分类 ---
pub use classifier::ErrorClassifier;

// ============================================================================
// 跨 Crate 桥接（Orphan Rule 兼容）
// ============================================================================

/// 将 SignalError 转为 NuzoError（nuzo_error 是同时依赖 values 和 signal 的唯一合法位置）
pub fn signal_error_to_nuzo_error(e: nuzo_signal::SignalError) -> nuzo_values::NuzoError {
    nuzo_values::NuzoError::internal(
        nuzo_values::InternalError::IoError { message: format!("signal error: {}", e) },
        None,
    )
}

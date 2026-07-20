//! ## 调试辅助函数
//!
//! 本模块提供**调试和性能分析**工具集，帮助开发者诊断问题、优化性能。
//! 所有调试输出均发送到 **stderr**（标准错误流），避免干扰正常 stdout 输出。
//!
//! ## 可用函数（4 个）
//!
//! | 函数 | 签名 | 说明 | 输出目标 |
//! |------|------|------|----------|
//! | `dump` | `dump(value) → nil` | 打印值的详细信息（类型+值）| stderr |
//! | `format` | `format(template, args...) → string` | 格式化字符串（类似 printf）| — |
//! | `time` | `time(label) → nil` | 启动带标签的计时器 | stderr |
//! | `time_end` | `time_end(label) → nil` | 结束计时器并打印耗时 | stderr |
//!
//! ## 核心功能
//!
//! ## 1. 变量转储（dump）
//!
//! 打印变量的完整信息，包括类型和值：
//!
//! ```text
//! dump(42)           // [dump] type=number, value=42
//! dump("hello")      // [dump] type=string, value=hello
//! dump([1, 2, 3])    // [dump] type=array, value=[1, 2, 3]
//! dump(nil)          // [dump] type=nil, value=nil
//! ```
//!
//! 适用场景：
//! - 快速查看变量内容和类型
//! - 调试复杂嵌套数据结构
//! - 验证函数返回值
//!
//! ## 2. 字符串格式化（format）
//!
//! 简单的模板替换，将 `{}` 占位符依次替换为参数：
//!
//! ```text
//! format("Hello, {}!", "World")     // → "Hello, World!"
//! format("{} + {} = {}", 1, 2, 3)   // → "1 + 2 = 3"
//! format("No args")                 // → "No args" (保留 {})
//! ```
//!
//! 特点：
//! - 使用 `{}` 作为占位符（类似 Python format）
//! - 参数按顺序替换
//! - 未匹配的占位符保留原样
//! - 类型自动转换为字符串
//!
//! ## 3. 性能计时器（time / time_end）
//!
//! 用于测量代码块执行时间：
//!
//! ```text
//! time("database_query")           // 启动计时器
//! // ... 执行数据库查询 ...
//! time_end("database_query")       // 输出: [time] database_query - 123.456ms
//! ```
//!
//! 特点：
//! - **标签命名**：支持多个并发计时器（不同标签）
//! - **高精度**：毫秒级显示（内部纳秒精度）
//! - **自动清理**：`time_end` 后移除计时器
//! - **错误容忍**：对未启动的标签调用 `time_end` 仅输出警告
//!
//! ## 设计理念
//!
//! ### 为什么输出到 stderr？
//!
//! 1. **不污染 stdout**：正常程序输出与调试信息分离
//! 2. **可重定向**：`./program 2>debug.log`
//! 3. **无条件刷新**：stderr 默认行缓冲，确保崩溃前输出
//! 4. **管道安全**：`program | grep pattern` 不受调试信息影响
//!
//! ## 使用示例
//!
//! ```text
//! // 调试复杂逻辑
//! fn process(data) {
//!     dump(data)
//!     let result = transform(data)
//!     dump(result)
//!     return result
//! }
//!
//! // 性能分析
//! time("total")
//! time("init")
//! initialize()
//! time_end("init")
//!
//! time("compute")
//! compute()
//! time_end("compute")
//!
//! time_end("total")
//! // 输出:
//! // [time] init - timer started
//! // [time] init - 12.345ms
//! // [time] compute - timer started
//! // [time] compute - 98.765ms
//! // [time] total - 111.110ms
//! ```
//!
//! ## 注意事项
//!
//! - **生产环境**：建议在发布版本中移除或条件禁用调试调用
//! - **线程安全**：计时器使用 Mutex 保护，可多线程使用
//! - **内存占用**：计时器在 `time_end` 前一直保存在内存中

use nuzo_core::{XxHashMap, xx_hash_map_new};
use std::sync::Mutex;
use web_time::Instant;

use once_cell::sync::Lazy;

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{NIL, NuzoError, ValueExt};

// ============================================================================
// 注册函数
// ============================================================================

/// 注册所有调试工具函数到 BuiltinRegistry
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "dump" => builtin_dump, arity = 1,
            signature = "dump(value) -> nil",
            desc = "打印值的详细信息（类型 + 值），用于调试。";
        "format" => builtin_format, arity = 1,
            signature = "format(template, args...) -> string",
            desc = "简单格式化字符串。将 {} 依次替换为后续参数的字符串表示。";
        "time" => builtin_time, arity = 1,
            signature = "time(label) -> nil",
            desc = "启动一个带标签的计时器。";
        "time_end" => builtin_time_end, arity = 1,
            signature = "time_end(label) -> nil",
            desc = "结束带标签的计时器并打印耗时（毫秒）。";
    }
}

// ============================================================================
// 计时器存储
// ============================================================================

static TIMERS: Lazy<Mutex<XxHashMap<String, Instant>>> =
    Lazy::new(|| Mutex::new(xx_hash_map_new()));

/// 计时器最大数量上限（P2-9）
///
/// 防止程序无限调用 `time()` 创建计时器导致内存泄漏。
/// 超过上限时拒绝新建并发出警告日志。
const MAX_TIMERS: usize = 100;

// ============================================================================
// 内置函数实现
// ============================================================================

/// **dump(value)** → nil
///
/// 打印值的详细信息（类型 + 值），用于调试。
fn builtin_dump(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];
    let type_name = val.type_name();
    let display = val.to_string();
    log::debug!("[dump] type={}, value={}", type_name, display);
    Ok(NIL)
}

/// **format(template, args...)** → string
///
/// 简单格式化字符串。将 {} 依次替换为后续参数的字符串表示。
fn builtin_format(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let template = if args[0].is_string() {
        args[0].as_string_opt().unwrap_or_default()
    } else {
        return Err(NuzoError::type_mismatch("string", args[0].type_name()));
    };

    let mut result = String::with_capacity(template.len() * 2);
    let mut arg_idx = 1;
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next(); // 消费 '}'
            if arg_idx < args.len() {
                result.push_str(&args[arg_idx].to_string());
                arg_idx += 1;
            } else {
                result.push_str("{}");
            }
        } else {
            result.push(ch);
        }
    }

    Ok(Value::from_string(&result))
}

/// **time(label)** → nil
///
/// 启动一个带标签的计时器。
fn builtin_time(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let label = if args[0].is_string() {
        args[0].as_string_opt().unwrap_or_default()
    } else {
        args[0].to_string()
    };

    let mut timers = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
    // P2-9: 检查计时器数量上限，防止内存泄漏
    if !timers.contains_key(&label) && timers.len() >= MAX_TIMERS {
        log::warn!(
            "[time] timer limit ({}) reached, refusing to create timer '{}'; \
             call time_end() to release existing timers",
            MAX_TIMERS,
            label
        );
        return Err(NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!(
                    "timer limit ({}) reached; call time_end() to release timers before creating new ones",
                    MAX_TIMERS
                ),
            },
            None,
        ));
    }
    timers.insert(label.clone(), Instant::now());
    log::debug!("[time] {} - timer started", label);
    Ok(NIL)
}

/// **time_end(label)** → nil
///
/// 结束带标签的计时器并打印耗时（毫秒）。
fn builtin_time_end(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let label = if args[0].is_string() {
        args[0].as_string_opt().unwrap_or_default()
    } else {
        args[0].to_string()
    };

    let mut timers = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
    match timers.remove(&label) {
        Some(start) => {
            let elapsed = start.elapsed();
            let ms =
                elapsed.as_secs() as f64 * 1000.0 + elapsed.subsec_nanos() as f64 / 1_000_000.0;
            log::debug!("[time] {} - {:.3}ms", label, ms);
        }
        None => {
            log::warn!("[time] {} - no such timer", label);
        }
    }
    Ok(NIL)
}

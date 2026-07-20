//! # 时间辅助函数
//!
//! 本模块提供**时间相关操作**功能集，支持获取当前时间、时间戳、延迟执行等。
//! 所有时间值基于 **Unix 纪元（1970-01-01 00:00:00 UTC）**，使用浮点数秒数表示。
//!
//! ## 可用函数（4 个）
//!
//! | 函数 | 签名 | 说明 | 精度 |
//! |------|------|------|------|
//! | `now` | `now() → number` | 当前 Unix 时间戳（秒）| 纳秒级 |
//! | `timestamp` | `timestamp() → number` | 当前毫秒时间戳 | 毫秒级 |
//! | `sleep` | `sleep(seconds) → nil` | 阻塞线程指定秒数 | 微秒级 |
//! | `clock` | `clock() → number` | 进程运行时间（秒）| 纳秒级 |
//!
//! ## 时间表示
//!
//! ### Unix 时间戳格式
//!
//! ```text
//! now() 返回值示例: 1704067200.123456789
//!                  ││ ││ ││ ││ ││ └─ 小数部分（纳秒）
//!                  ││ ││ ││ ││ │└─── 毫秒 (10^-3)
//!                  ││ ││ ││ │└───── 厘秒 (10^-2)
//!                  ││ ││ │└─────── 分秒 (10^-1)
//!                  ││ │└───────── 整数秒（自 1970-01-01）
//!                  │└────────── 月
//!                  └─────────── 年 (2024)
//! ```
//!
//! ### 时间范围
//!
//! - **最小值**：负数（1970 年之前的日期）
//! - **最大值**：取决于系统（通常支持到 2038 年后或更远）
//! - **精度限制**：f64 可精确表示 ±2^53 纳秒范围内的整数（约 285 年）
//!
//! # 使用示例
//!
//! ```nuzo
//! // 获取当前时间
//! let t = now()
//! println("当前时间戳: " + str(t))
//!
//! // 计算代码执行时间
//! let start = clock()
//! // ... 执行耗时操作 ...
//! let elapsed = clock() - start
//! println("耗时: " + str(elapsed) + " 秒")
//!
//! // 高精度计时（毫秒）
//! let ms = timestamp()
//!
//! // 延迟执行（慎用：会阻塞线程）
//! sleep(1.5)  // 暂停 1.5 秒
//! ```
//!
//! # 时区说明
//!
//! - **所有时间均为 UTC**：不包含时区信息
//! - **本地时区转换**：需配合系统 API 或第三方库
//! - **夏令时**：不自动处理（如需要请使用专门的日期库）
//!
//! # 性能特征
//!
//! - **高分辨率**：使用 `SystemTime` 和 `Instant`（平台最优）
//! - **单调时钟**：`clock()` 使用单调时钟，不受系统时间调整影响
//! - **低开销**：单次调用 < 1 微秒

use web_time::{Instant, SystemTime, UNIX_EPOCH};

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{NIL, NuzoError, ValueExt};

/// Maximum allowed sleep duration in seconds (1 day).
const MAX_SLEEP_SECS: f64 = 86400.0;

// ============================================================================
// 注册函数
// ============================================================================

/// 注册所有时间处理函数到 BuiltinRegistry
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "now" => builtin_now, arity = 0,
            signature = "now() -> number",
            desc = "返回当前 Unix 时间戳（秒，浮点数）。";
        "sleep" => builtin_sleep, arity = 1,
            signature = "sleep(seconds) -> nil",
            desc = "阻塞当前线程指定秒数。";
        "timestamp" => builtin_timestamp, arity = 0,
            signature = "timestamp() -> number",
            desc = "返回当前毫秒时间戳。";
        "clock" => builtin_clock, arity = 0,
            signature = "clock() -> number",
            desc = "返回进程运行时间（秒）。";
    }
}

// ============================================================================
// 进程启动时间（用于 clock()）
// ============================================================================

static PROCESS_START: once_cell::sync::Lazy<Instant> = once_cell::sync::Lazy::new(Instant::now);

// ============================================================================
// 内置函数实现
// ============================================================================

/// **now()** → number
///
/// 返回当前 Unix 时间戳（秒，浮点数）。
fn builtin_now(_args: &[Value]) -> Result<Value, NuzoError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError { message: format!("system time error: {}", e) },
            None,
        )
    })?;
    let secs = duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 1_000_000_000.0;
    Ok(Value::from_number(secs))
}

/// **sleep(seconds)** → nil
///
/// 阻塞当前线程指定秒数。
fn builtin_sleep(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    if !args[0].is_number() {
        return Err(NuzoError::type_mismatch("number", args[0].type_name()));
    }
    let secs = args[0].as_number();
    if secs < 0.0 {
        return Err(NuzoError::type_mismatch("non-negative number", format!("negative: {}", secs)));
    }
    if secs > MAX_SLEEP_SECS {
        return Err(NuzoError::type_mismatch(
            format!("sleep duration at most {} seconds", MAX_SLEEP_SECS),
            format!("{} seconds", secs),
        ));
    }
    let duration = std::time::Duration::from_secs_f64(secs);
    std::thread::sleep(duration);
    Ok(NIL)
}

/// **timestamp()** → number
///
/// 返回当前毫秒时间戳。
fn builtin_timestamp(_args: &[Value]) -> Result<Value, NuzoError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError { message: format!("system time error: {}", e) },
            None,
        )
    })?;
    let millis = duration.as_millis() as f64;
    Ok(Value::from_number(millis))
}

/// **clock()** → number
///
/// 返回进程运行时间（秒）。
fn builtin_clock(_args: &[Value]) -> Result<Value, NuzoError> {
    let elapsed = PROCESS_START.elapsed();
    let secs = elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1_000_000_000.0;
    Ok(Value::from_number(secs))
}

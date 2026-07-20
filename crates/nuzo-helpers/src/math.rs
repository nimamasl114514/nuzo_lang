//! # 数学辅助函数
//!
//! 本模块提供**数学运算**功能集，覆盖基础算术、三角函数、统计等场景。
//! 所有运算基于 IEEE 754 双精度浮点数（f64），确保跨平台一致性。
//!
//! ## 可用函数（15 个）
//!
//! ### 基础运算（5 个）
//! | 函数 | 签名 | 说明 | 边界检查 |
//! |------|------|------|----------|
//! | `abs` | `abs(x) → number` | 绝对值 | ✅ 无溢出风险 |
//! | `floor` | `floor(x) → number` | 向下取整 | ✅ |
//! | `ceil` | `ceil(x) → number` | 向上取整 | ✅ |
//! | `round` | `round(x) → number` | 四舍五入 | ✅ |
//! | `sqrt` | `sqrt(x) → number` | 平方根 | ⚠️ x 必须 >= 0 |
//!
//! ### 幂与对数（2 个）
//! | 函数 | 签名 | 说明 | 边界检查 |
//! |------|------|------|----------|
//! | `pow` | `pow(base, exp) → number` | 幂运算 | ⚠️ 可能溢出/下溢 |
//! | `log` | `log(x) → number` | 自然对数 | ⚠️ x 必须 > 0 |
//!
//! ### 统计函数（2 个）
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `min` | `min(a, b) → number` | 最小值 |
//! | `max` | `max(a, b) → number` | 最大值 |
//!
//! ### 三角函数（3 个）
//! | 函数 | 签名 | 说明 | 单位 |
//! |------|------|------|------|
//! | `sin` | `sin(x) → number` | 正弦 | 弧度 |
//! | `cos` | `cos(x) → number` | 余弦 | 弧度 |
//! | `tan` | `tan(x) → number` | 正切 | 弧度 |
//!
//! ### 随机数与常量（3 个）
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `random` | `random() → number` | [0, 1) 伪随机数 |
//! | `pi` | `pi() → number` | 圆周率 π ≈ 3.14159 |
//! | `e` | `e() → number` | 自然常数 e ≈ 2.71828 |
//!
//! ## 精度与溢出处理
//!
//! ### IEEE 754 f64 特性
//!
//! - **精度**：约 15-17 位有效数字
//! - **范围**：±1.8 × 10^308
//! - **特殊值**：支持 NaN、Infinity、-Infinity
//!
//! ### 边界条件行为
//!
//! | 函数 | 输入 | 输出 | 错误处理 |
//! |------|------|------|----------|
//! | `sqrt(-1)` | 负数 | — | 返回 TypeMismatch 错误 |
//! | `log(0)` 或 `log(-1)` | 非正数 | — | 返回 TypeMismatch 错误 |
//! | `pow(10, 1000)` | 大指数 | Infinity | 允许（IEEE 754 规范）|
//! | `abs(Infinity)` | Infinity | Infinity | 正常 |
//! | `round(NaN)` | NaN | NaN | 正常 |
//!
//! ### 随机数生成器
//!
//! 使用 **Xorshift64** 算法（线程局部状态）：
//! - **周期**：2^64 - 1
//! - **种子**：固定值 `0xDEADBEEFCAFEBABE`
//! - **线程安全**：每个线程独立状态
//! - **性能**：极快（仅几次位运算）
//!
//! ⚠️ **注意**：当前实现使用固定种子，不适合加密场景。
//!
//! ## 使用示例
//!
//! ```nuzo
//! // 基础运算
//! abs(-5)          // → 5
//! floor(3.7)       // → 3.0
//! ceil(3.2)        // → 4.0
//! round(3.5)       // → 4.0
//! sqrt(16.0)       // → 4.0
//!
//! // 幂与对数
//! pow(2, 10)       // → 1024.0
//! log(2.71828)     // → ~1.0
//!
//! // 三角函数（弧度制）
//! sin(0)           // → 0.0
//! cos(pi())        // → -1.0
//! tan(pi()/4)      // → 1.0
//!
//! // 随机数
//! let r = random()  // → [0, 1) 的浮点数
//! ```
//!
//! # 性能特征
//!
//! - **硬件加速**：x86 架构自动使用 SSE/AVX 指令
//! - **内联优化**：小型函数（abs, floor 等）通常被内联
//! - **无堆分配**：所有运算在栈上完成

/// 默认随机数种子回退值（仅在系统熵不可用时使用）。
///
/// P2-10 修复：原实现使用固定种子 `0x0123_4567_89AB_CDEF`，导致每次程序启动
/// 产生相同的随机序列。现在首次调用 `random()` 时从系统熵（SystemTime + 线程 ID）
/// 派生种子，仅在系统熵不可用时回退到此固定值。
const RNG_FALLBACK_SEED: u64 = 0x0123_4567_89AB_CDEF;

/// 从系统熵派生随机种子（P2-10）
///
/// 混合 `SystemTime` 和 `thread::current().id()` 提供每次运行不同的种子。
/// 若 `SystemTime` 不可用，回退到 `RNG_FALLBACK_SEED`。
fn derive_seed_from_system_entropy() -> u64 {
    use web_time::{SystemTime, UNIX_EPOCH};

    let time_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(RNG_FALLBACK_SEED);

    // 混入线程 ID（不同线程获得不同种子）
    let thread_id = {
        let tid = format!("{:?}", std::thread::current().id());
        // 简单 FNV-1a hash
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in tid.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    };

    // 混合 time + thread_id，避免零值
    let mixed = time_nanos.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(thread_id);

    if mixed == 0 { RNG_FALLBACK_SEED } else { mixed }
}

/// 设置随机数种子（P2-10 新增，供测试和需要可复现序列的场景使用）
///
/// 调用后立即更新当前线程的 RNG 状态。测试中可通过此函数固定种子
/// 以获得可复现的随机序列。
pub fn set_random_seed(seed: u64) {
    RNG_STATE.with(|state| state.set(seed));
}

// RNG 状态声明移到模块级，供 set_random_seed 和 builtin_random 共享
use std::cell::Cell;
thread_local! {
    static RNG_STATE: Cell<u64> = const { Cell::new(0) };
}

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{NuzoError, ValueExt};

// ============================================================================
// 注册函数
// ============================================================================

/// 注册所有数学运算函数到 BuiltinRegistry
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "abs" => builtin_abs, arity = 1,
            signature = "abs(x) -> number",
            desc = "返回 x 的绝对值。";
        "floor" => builtin_floor, arity = 1,
            signature = "floor(x) -> number",
            desc = "返回不大于 x 的最大整数。";
        "ceil" => builtin_ceil, arity = 1,
            signature = "ceil(x) -> number",
            desc = "返回不小于 x 的最小整数。";
        "round" => builtin_round, arity = 1,
            signature = "round(x) -> number",
            desc = "返回 x 四舍五入后的整数。";
        "sqrt" => builtin_sqrt, arity = 1,
            signature = "sqrt(x) -> number",
            desc = "返回 x 的平方根。x 不能为负数。";
        "pow" => builtin_pow, arity = 2,
            signature = "pow(base, exp) -> number",
            desc = "返回 base^exp。";
        "min" => builtin_min, arity = 2,
            signature = "min(a, b) -> number",
            desc = "返回 a 和 b 中的较小值。";
        "max" => builtin_max, arity = 2,
            signature = "max(a, b) -> number",
            desc = "返回 a 和 b 中的较大值。";
        "random" => builtin_random, arity = 0,
            signature = "random() -> number",
            desc = "返回 [0, 1) 范围内的伪随机浮点数。";
        "sin" => builtin_sin, arity = 1,
            signature = "sin(x) -> number",
            desc = "返回 x（弧度）的正弦值。";
        "cos" => builtin_cos, arity = 1,
            signature = "cos(x) -> number",
            desc = "返回 x（弧度）的余弦值。";
        "tan" => builtin_tan, arity = 1,
            signature = "tan(x) -> number",
            desc = "返回 x（弧度）的正切值。";
        "log" => builtin_log, arity = 1,
            signature = "log(x) -> number",
            desc = "返回 x 的自然对数。x 必须为正数。";
        "pi" => builtin_pi, arity = 0,
            signature = "pi() -> number",
            desc = "返回圆周率 π 的值。";
        "e" => builtin_e, arity = 0,
            signature = "e() -> number",
            desc = "返回自然常数 e 的值。";
    }
}

// ============================================================================
// 辅助宏：提取单个数字参数
// ============================================================================

macro_rules! require_one_number {
    ($args:expr, $name:expr) => {{
        if $args.len() != 1 {
            return Err(NuzoError::invalid_argument_count(1, $args.len()));
        }
        if !$args[0].is_number() {
            return Err(NuzoError::type_mismatch("number", $args[0].type_name()));
        }
        $args[0].as_number()
    }};
}

macro_rules! require_two_numbers {
    ($args:expr, $name:expr) => {{
        if $args.len() != 2 {
            return Err(NuzoError::invalid_argument_count(2, $args.len()));
        }
        if !$args[0].is_number() {
            return Err(NuzoError::type_mismatch("number", $args[0].type_name()));
        }
        if !$args[1].is_number() {
            return Err(NuzoError::type_mismatch("number", $args[1].type_name()));
        }
        ($args[0].as_number(), $args[1].as_number())
    }};
}

// ============================================================================
// 内置函数实现
// ============================================================================

define_builtin_impl! {
    /// **abs(x)** → number
    ///
    /// 返回 x 的绝对值。
    fn builtin_abs(args = args, count = 1, check = [require_number @ 0]) {
        let n = args[0].as_number();
        Ok(Value::from_number(n.abs()))
    }
}

define_builtin_impl! {
    /// **floor(x)** → number
    ///
    /// 返回不大于 x 的最大整数。
    fn builtin_floor(args = args, count = 1, check = [require_number @ 0]) {
        let n = args[0].as_number();
        Ok(Value::from_number(n.floor()))
    }
}

/// **ceil(x)** → number
///
/// 返回不小于 x 的最小整数。
fn builtin_ceil(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "ceil");
    Ok(Value::from_number(n.ceil()))
}

/// **round(x)** → number
///
/// 返回 x 四舍五入后的整数。
fn builtin_round(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "round");
    Ok(Value::from_number(n.round()))
}

/// **sqrt(x)** → number
///
/// 返回 x 的平方根。x 不能为负数。
fn builtin_sqrt(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "sqrt");
    if n < 0.0 {
        return Err(NuzoError::type_mismatch("non-negative number", format!("negative: {}", n)));
    }
    Ok(Value::from_number(n.sqrt()))
}

/// **pow(base, exp)** → number
///
/// 返回 base^exp。
fn builtin_pow(args: &[Value]) -> Result<Value, NuzoError> {
    let (base, exp) = require_two_numbers!(args, "pow");
    Ok(Value::from_number(base.powf(exp)))
}

/// **min(a, b)** → number
///
/// 返回 a 和 b 中的较小值。
fn builtin_min(args: &[Value]) -> Result<Value, NuzoError> {
    let (a, b) = require_two_numbers!(args, "min");
    Ok(Value::from_number(a.min(b)))
}

/// **max(a, b)** → number
///
/// 返回 a 和 b 中的较大值。
fn builtin_max(args: &[Value]) -> Result<Value, NuzoError> {
    let (a, b) = require_two_numbers!(args, "max");
    Ok(Value::from_number(a.max(b)))
}

/// **random()** → number
///
/// 返回 [0, 1) 范围内的伪随机浮点数。
///
/// P2-10 修复：首次调用时从系统熵（SystemTime + 线程 ID）派生种子，
/// 不再使用固定种子。可通过 [`set_random_seed`](super::math::set_random_seed)
/// 手动设置种子以获得可复现序列。
fn builtin_random(_args: &[Value]) -> Result<Value, NuzoError> {
    RNG_STATE.with(|state| {
        let mut s = state.get();
        // 首次调用：state 为 0，从系统熵派生种子
        if s == 0 {
            s = derive_seed_from_system_entropy();
        }
        // xorshift64
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        state.set(s);
        // 映射到 [0, 1)
        let result = (s >> 11) as f64 / (1u64 << 53) as f64;
        Ok(Value::from_number(result))
    })
}

/// **sin(x)** → number
///
/// 返回 x（弧度）的正弦值。
fn builtin_sin(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "sin");
    Ok(Value::from_number(n.sin()))
}

/// **cos(x)** → number
///
/// 返回 x（弧度）的余弦值。
fn builtin_cos(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "cos");
    Ok(Value::from_number(n.cos()))
}

/// **tan(x)** → number
///
/// 返回 x（弧度）的正切值。
fn builtin_tan(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "tan");
    Ok(Value::from_number(n.tan()))
}

/// **log(x)** → number
///
/// 返回 x 的自然对数。x 必须为正数。
fn builtin_log(args: &[Value]) -> Result<Value, NuzoError> {
    let n = require_one_number!(args, "log");
    if n <= 0.0 {
        return Err(NuzoError::type_mismatch("positive number", format!("non-positive: {}", n)));
    }
    Ok(Value::from_number(n.ln()))
}

/// **pi()** → number
///
/// 返回圆周率 π 的值。
fn builtin_pi(_args: &[Value]) -> Result<Value, NuzoError> {
    Ok(Value::from_number(std::f64::consts::PI))
}

/// **e()** → number
///
/// 返回自然常数 e 的值。
fn builtin_e(_args: &[Value]) -> Result<Value, NuzoError> {
    Ok(Value::from_number(std::f64::consts::E))
}

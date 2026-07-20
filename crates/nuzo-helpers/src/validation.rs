#![allow(unused_macros)]
#![allow(dead_code)]

//! # 参数验证宏 — 统一消除各模块重复的参数校验代码
//!
//! 本模块提供两个核心验证宏，用于替代 nuzo_helpers 各子模块中
//! 手写的 `invalid_argument_count` 检查（原 31+ 处重复）。
//!
//! ## 使用方式
//!
//! ```rust,ignore
//! use nuzo_helpers::validation::*;  // 或通过 #[macro_use] 自动导入
//!
//! fn my_builtin(args: &[Value]) -> Result<Value, NuzoError> {
//!     require_arg_count!(args, 2);  // 精确要求 2 个参数
//!     // ... 业务逻辑 ...
//! }
//! ```
//!
//! ## 设计原则
//!
//! - **零开销**：宏展开后与手写代码完全等价，不引入任何运行时开销
//! - **行为不变**：产生完全相同的 `NuzoError::invalid_argument_count(...)`
//! - **两种模式**：精确匹配 (`require_arg_count!`) 和最小数量 (`require_min_args!`)

use nuzo_core::NuzoError;

// ============================================================================
// 核心验证宏
// ============================================================================

/// 验证参数数量为精确值 N。
///
/// 当 `$args.len() != $expected` 时，返回 `NuzoError::invalid_argument_count`。
///
/// # 示例
///
/// ```rust,ignore
/// require_arg_count!(args, 1);   // 要求恰好 1 个参数
/// require_arg_count!(args, 3);   // 要求恰好 3 个参数
/// ```
macro_rules! require_arg_count {
    ($args:expr, $expected:expr) => {
        if $args.len() != $expected {
            return Err(NuzoError::invalid_argument_count($expected, $args.len()));
        }
    };
}

/// 验证参数数量至少为 N。
///
/// 当 `$args.len() < $min` 时，返回 `NuzoError::invalid_argument_count`。
/// 适用于可变参数函数（如 `format(template, args...)`）或最小参数需求场景。
///
/// # 示例
///
/// ```rust,ignore
/// require_min_args!(args, 1);   // 要求至少 1 个参数
/// require_min_args!(args, 2);   // 要求至少 2 个参数
/// ```
macro_rules! require_min_args {
    ($args:expr, $min:expr) => {
        if $args.len() < $min {
            return Err(NuzoError::invalid_argument_count($min, $args.len()));
        }
    };
}

// ============================================================================
// 类型检查专用宏 — 替代重复的 is_xxx() + type_mismatch 模式
// ============================================================================

/// 验证第 `$idx` 个参数为数字类型。
///
/// 当 `!$args[$idx].is_number()` 时，返回 `NuzoError::type_mismatch("number", ...)`。
///
/// # 示例
///
/// ```rust,ignore
/// require_number!(args, 0);   // 要求 args[0] 是数字
/// require_number!(args, 1);   // 要求 args[1] 是数字
/// ```
macro_rules! require_number {
    ($args:expr, $idx:expr) => {
        if !$args[$idx].is_number() {
            return Err(NuzoError::type_mismatch("number", $args[$idx].type_name()));
        }
    };
}

/// 验证第 `$idx` 个参数为字符串类型。
///
/// 当 `!$args[$idx].is_string()` 时，返回 `NuzoError::type_mismatch("string", ...)`。
///
/// # 示例
///
/// ```rust,ignore
/// require_string!(args, 0);   // 要求 args[0] 是字符串
/// ```
macro_rules! require_string {
    ($args:expr, $idx:expr) => {
        if !$args[$idx].is_string() {
            return Err(NuzoError::type_mismatch("string", $args[$idx].type_name()));
        }
    };
}

/// 验证第 `$idx` 个参数为数组/堆对象类型。
///
/// 当 `!$args[$idx].is_heap_object()` 时，返回 `NuzoError::type_mismatch("array", ...)`。
///
/// # 示例
///
/// ```rust,ignore
/// require_array!(args, 0);    // 要求 args[0] 是数组
/// ```
macro_rules! require_array {
    ($args:expr, $idx:expr) => {
        if !$args[$idx].is_heap_object() {
            return Err(NuzoError::type_mismatch("array", $args[$idx].type_name()));
        }
    };
}

// ============================================================================
// builtin 函数定义模板 — 统一函数签名 + 参数校验样板
// ============================================================================

/// 统一的 builtin 函数定义模板。
///
/// 消除「`fn` 签名 + 参数数量校验 + 类型校验」的样板代码，让 builtin 实现
/// 聚焦于业务逻辑。展开后与手写代码完全等价，无运行时开销。
///
/// # 形式 1：仅参数数量校验
///
/// 适用于无需特定类型校验的 builtin（如 `is_nil`、`is_number` 等类型判断函数，
/// 它们接受任意类型参数）。
///
/// ```rust,ignore
/// define_builtin_impl! {
///     fn builtin_is_nil(args = args, count = 1) {
///         Ok(Value::from_bool(args[0].is_nil()))
///     }
/// }
/// ```
///
/// # 形式 2：参数数量 + 类型校验
///
/// `check` 列表中每项形如 `<require_macro> @ <index>`，展开后依次调用
/// 对应的 `require_<type>!` 宏。例如 `require_number @ 0` 展开为
/// `require_number!(args, 0)`。
///
/// ```rust,ignore
/// define_builtin_impl! {
///     fn builtin_abs(args = args, count = 1, check = [require_number @ 0]) {
///         let n = args[0].as_number();
///         Ok(Value::from_number(n.abs()))
///     }
/// }
/// ```
///
/// # 设计约束
///
/// - 生成的函数为 `fn`（模块私有），与现有 builtin 一致；
/// - 函数体内 `Value` 与 `NuzoError` 必须在调用点已 in-scope（调用模块需 `use`）；
/// - 行为与手写代码完全等价：使用相同的 `NuzoError::invalid_argument_count`
///   和 `NuzoError::type_mismatch` 错误类型与错误信息。
/// - 支持在 `fn` 前附加 `#[...]` 属性或 `///` 文档注释，会被转发到生成的函数上。
macro_rules! define_builtin_impl {
    // 形式 1：仅参数数量校验
    (
        $(#[$meta:meta])*
        fn $name:ident(args = $args:ident, count = $count:expr) {
            $($body:tt)*
        }
    ) => {
        $(#[$meta])*
        fn $name($args: &[Value]) -> Result<Value, NuzoError> {
            require_arg_count!($args, $count);
            $($body)*
        }
    };

    // 形式 2：参数数量 + 类型校验
    (
        $(#[$meta:meta])*
        fn $name:ident(args = $args:ident, count = $count:expr,
                       check = [$($req:ident @ $idx:expr),* $(,)?]) {
            $($body:tt)*
        }
    ) => {
        $(#[$meta])*
        fn $name($args: &[Value]) -> Result<Value, NuzoError> {
            require_arg_count!($args, $count);
            $($req!($args, $idx);)*
            $($body)*
        }
    };
}

// ============================================================================
// 公共验证函数 — 消除子模块重复
// ============================================================================

/// 验证索引值为非负有限数，并转换为 usize。
///
/// 当输入为负数、NaN 或无穷大时返回类型错误。
///
/// # 示例
///
/// ```rust,ignore
/// let idx = validate_index(3.0)?;   // Ok(3)
/// let idx = validate_index(-1.0)?;  // Err(TypeMismatch)
/// ```
/// 索引值的最大可表示上限。
///
/// `f64` 转 `usize` 时,超过 `usize::MAX` 的值会饱和到 `usize::MAX`,
/// 导致后续边界检查失效(例如 `i >= len` 几乎恒为 false)。
/// 用 `usize::MAX / 2` 作为安全上限:既留出浮点精度余量(大 f64 无法精确
/// 表示 usize::MAX 附近的整数),又能可靠拦截会导致饱和的溢出输入。
pub(crate) const MAX_INDEXABLE: f64 = (usize::MAX as f64) / 2.0;

pub(crate) fn validate_index(n: f64) -> Result<usize, NuzoError> {
    if n < 0.0 || n.is_nan() || n.is_infinite() {
        return Err(NuzoError::type_mismatch("non-negative integer index", format!("{}", n)));
    }
    if n > MAX_INDEXABLE {
        return Err(NuzoError::type_mismatch(
            "index within addressable range",
            format!("{} (exceeds MAX_INDEXABLE={})", n, MAX_INDEXABLE),
        ));
    }
    Ok(n as usize)
}

// 宏通过 #[macro_use] mod validation 自动导入所有子模块，无需 re-export

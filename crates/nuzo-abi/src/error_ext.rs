//! NuzoError 工厂方法扩展，统一错误构造模式。
//!
//! 各 crate 中存在大量 `NuzoError::type_mismatch(...)` 等重复构造调用，
//! 此类型提供统一的扩展接口，减少样板代码并确保错误码和字段名一致。
//!
//! # 为什么不用 NuzoError 自身的工厂方法？
//!
//! `NuzoError` 已经提供了 `type_mismatch()`、`index_out_of_bounds()` 等方法。
//! 此类型的定位是：当未来需要跨 crate 统一错误构造策略（如自动附加
//! source_location、统一的中文消息模板等）时，只需修改此类型的实现，
//! 而不必修改每个调用点。
//!
//! 当前版本直接委托给 `NuzoError` 的工厂方法，保持零开销抽象。
//!
//! # 用法
//!
//! ```ignore
//! use nuzo_abi::NuzoErrorExt;
//!
//! // 替换 NuzoError::type_mismatch("array", actual.to_string())
//! let err = NuzoErrorExt::type_mismatch("array", actual);
//!
//! // 替换 NuzoError::invalid_argument_count(2, argc)
//! let err = NuzoErrorExt::arity_mismatch(2, argc);
//! ```

use nuzo_core::NuzoError;

/// NuzoError 工厂方法扩展。
///
/// 以零大小 struct 提供语义化的工厂方法，确保每个错误都附带正确的
/// `ErrorCode` 和 `NuzoErrorKind` 字段名。所有方法均为关联函数，
/// 通过 `NuzoErrorExt::method(...)` 调用，不会与 `NuzoError` 的
/// 同名固有方法产生歧义。
pub struct NuzoErrorExt;

impl NuzoErrorExt {
    /// 类型不匹配错误。
    pub fn type_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> NuzoError {
        NuzoError::type_mismatch(expected, actual)
    }

    /// 索引越界错误。
    pub fn index_out_of_bounds(index: impl Into<String>, length: impl Into<String>) -> NuzoError {
        NuzoError::index_out_of_bounds(index, length)
    }

    /// 未定义变量错误。
    pub fn undefined_variable(name: impl Into<String>) -> NuzoError {
        NuzoError::undefined_variable(name)
    }

    /// 不支持的操作错误。
    pub fn unsupported_operation(op: impl Into<String>, on_type: impl Into<String>) -> NuzoError {
        NuzoError::unsupported_operation(op, on_type)
    }

    /// 参数数量不匹配错误。
    pub fn arity_mismatch(expected: usize, got: usize) -> NuzoError {
        NuzoError::invalid_argument_count(expected, got)
    }

    /// 期望数字但得到其他类型。
    pub fn expected_number(got: impl Into<String>) -> NuzoError {
        NuzoError::expected_number(got)
    }

    /// 断言失败。
    pub fn assert_failed(message: impl Into<String>) -> NuzoError {
        NuzoError::assert_failed(message)
    }

    /// 除零错误。
    pub fn division_by_zero() -> NuzoError {
        NuzoError::division_by_zero()
    }

    /// 算术溢出。
    pub fn arithmetic_overflow() -> NuzoError {
        NuzoError::arithmetic_overflow()
    }
}

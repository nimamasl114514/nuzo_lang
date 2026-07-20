//! # 类型转换辅助函数
//!
//! 本模块提供**安全的类型转换**和**类型判断**功能，是 Nuzo 动态类型系统的核心工具集。
//! 所有转换函数都遵循严格的参数校验规则，确保运行时安全。
//!
//! ## 可用函数
//!
//! ### 类型转换（4 个）
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `int` | `int(x) → number` | 转换为整数（截断小数部分） |
//! | `float` | `float(x) → number` | 转换为浮点数 |
//! | `bool` | `bool(x) → bool` | 转换为布尔值 |
//! | `num` | `num(s) → number` | 字符串解析为数字 |
//!
//! ### 类型判断（6 个）
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `is_nil` | `is_nil(x) → bool` | 判断是否为 nil |
//! | `is_number` | `is_number(x) → bool` | 判断是否为数字 |
//! | `is_string` | `is_string(x) → bool` | 判断是否为字符串 |
//! | `is_array` | `is_array(x) → bool` | 判断是否为数组 |
//! | `is_dict` | `is_dict(x) → bool` | 判断是否为字典 |
//! | `is_closure` | `is_closure(x) → bool` | 判断是否为闭包/函数 |
//!
//! ## 设计原则
//!
//! ### 安全性保证
//!
//! 1. **参数数量校验**：所有函数严格验证参数数量，不匹配时返回明确的错误
//! 2. **类型兼容性检查**：只允许有意义的类型转换，非法组合返回 `TypeMismatch` 错误
//! 3. **边界条件处理**：
//!    - 空字符串、nil、特殊浮点值（NaN/Infinity）都有明确定义的行为
//!    - 字符串解析失败时提供清晰的错误信息（包含原始值）
//!
//! ### 类型转换规则
//!
//! #### int(x) 的转换逻辑
//! ```text
//! 输入类型          →  输出          示例
//! ──────────────────────────────────────
//! 数字 (f64)        →  截断整数      int(3.7)   = 3
//!                                       int(-2.9)  = -2
//! 布尔              →  0 或 1        int(true)  = 1
//!                                       int(false) = 0
//! nil               →  0             int(nil)   = 0
//! 字符串            →  解析整数      int("42")  = 42
//!                     （失败则尝试浮点后截断）
//! 其他类型          →  错误          int([1])   = TypeMismatch
//! ```
//!
//! #### float(x) 的转换逻辑
//! ```text
//! 输入类型          →  输出          示例
//! ──────────────────────────────────────
//! 数字 (f64)        →  原值          float(3)    = 3.0
//! 布尔              →  0.0 或 1.0    float(true)= 1.0
//! nil               →  0.0           float(nil)  = 0.0
//! 字符串            →  解析浮点      float("3.14")= 3.14
//! 其他类型          →  错误
//! ```
//!
//! #### bool(x) 的真值规则
//! ```text
//! falsy 值: false, nil, 0, "" (空字符串)
//! truthy 值: 其他所有值（包括负数、非空字符串、数组等）
//! ```
//!
//! ## 使用示例
//!
//! ```nuzo
//! // 类型转换
//! let x = int("42")           // x = 42 (number)
//! let y = float("3.14")       // y = 3.14
//! let b = bool(1)             // b = true
//! let n = num("2.71828")      // n = 2.71828
//!
//! // 类型判断
//! if is_string(x) {
//!     println("x 是字符串")
//! }
//! if is_number(y) {
//!     println("y 是数字")
//! }
//!
//! // 数据清洗场景
//! let input = read_file("data.txt")
//! let lines = split(input, "\n")
//! for line in lines {
//!     let trimmed = trim(line)
//!     if !is_empty(trimmed) {
//!         let value = num(trimmed)
//!         if !is_nil(value) {  // 注意: num 失败会抛错，这里需要 try-catch
//!             process(value)
//!         }
//!     }
//! }
//! ```
//!
//! ## 错误处理
//!
//! 所有函数在遇到无法处理的输入时会返回结构化错误：
//!
//! - **参数数量错误**：`NuzoError::InvalidArgumentCount`
//! - **类型不匹配**：`NuzoError::TypeMismatch`（包含期望类型和实际类型）
//! - **解析失败**：`NuzoError::TypeMismatch`（包含原始字符串值，便于调试）
//!
//! # 性能特征
//!
//! - **零堆分配**：基本类型转换（number, bool, nil）无内存分配
//! - **惰性解析**：字符串只在必要时才进行解析
//! - **短路求值**：类型检查在第一个不匹配项即返回

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{NuzoError, ValueExt};

// ============================================================================
// 注册函数
// ============================================================================

/// 将所有类型转换函数注册到 BuiltinRegistry
///
/// 此函数应在 VM 初始化时调用，将以下函数添加到全局命名空间：
/// - `int`, `float`, `bool`, `num`（转换函数）
/// - `is_nil`, `is_number`, `is_string`, `is_array`, `is_dict`, `is_closure`（判断函数）
///
/// # 参数
///
/// * `reg` - 可变的 BuiltinRegistry 引用
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "int" => builtin_int, arity = 1,
            signature = "int(x) -> number",
            desc = "将值转换为整数，通过截断小数部分实现（非四舍五入）。";
        "float" => builtin_float, arity = 1,
            signature = "float(x) -> number",
            desc = "将值转换为浮点数表示。";
        "bool" => builtin_bool, arity = 1,
            signature = "bool(x) -> bool",
            desc = "根据值的真值性（truthiness）转换为布尔值。";
        "num" => builtin_num, arity = 1,
            signature = "num(s) -> number",
            desc = "将字符串或数字解析为数值。";
        "is_nil" => builtin_is_nil, arity = 1,
            signature = "is_nil(x) -> bool",
            desc = "检测值是否为 nil（空值/无值）。";
        "is_number" => builtin_is_number, arity = 1,
            signature = "is_number(x) -> bool",
            desc = "检测值是否为数字类型（f64 浮点数）。";
        "is_string" => builtin_is_string, arity = 1,
            signature = "is_string(x) -> bool",
            desc = "检测值是否为字符串类型。";
        "is_array" => builtin_is_array, arity = 1,
            signature = "is_array(x) -> bool",
            desc = "检测值是否为数组（有序集合）。";
        "is_dict" => builtin_is_dict, arity = 1,
            signature = "is_dict(x) -> bool",
            desc = "检测值是否为字典（键值对集合）。";
        "is_closure" => builtin_is_closure, arity = 1,
            signature = "is_closure(x) -> bool",
            desc = "检测值是否为可调用的闭包或内置函数。";
    }
}

// ============================================================================
// 内置函数实现
// ============================================================================

/// **int(x)** → number（整数转换）
///
/// 将值转换为整数，通过**截断小数部分**实现（非四舍五入）。
///
/// # 支持的输入类型与转换规则
///
/// | 输入类型 | 转换逻辑 | 示例 |
/// |----------|----------|------|
/// | 数字 (f64) | 向零截断（`trunc()`）| `int(3.7)` → 3, `int(-2.9)` → -2 |
/// | 布尔 | `true` → 1, `false` → 0 | `int(true)` → 1 |
/// | nil | 固定返回 0 | `int(nil)` → 0 |
/// | 字符串 | 尝试解析为整数，失败则尝试解析浮点后截断 | `int("42")` → 42 |
/// | 其他类型 | 返回 TypeMismatch 错误 | `int([1])` → ❌ |
///
/// # 边界条件处理
///
/// - **空字符串**：解析失败，返回错误
/// - **浮点字符串**：先尝试 i64 解析，失败后尝试 f64 截断
///   - `"3.14"` → 3.0
///   - `"abc"` → 错误
/// - **特殊浮点值**：NaN、Infinity 等会被截断为特定整数值
///
/// # 安全性说明
///
/// ✅ **安全**：
/// - 参数数量校验（必须恰好 1 个）
/// - 类型兼容性检查
/// - 字符串解析失败时的清晰错误信息（包含原始值）
///
/// ⚠️ **注意**：
/// - 不进行四舍五入（使用 `trunc()` 而非 `round()`）
/// - 大整数可能损失精度（f64 的整数精度限制在 2^53 以内）
///
/// # 示例
///
/// ```nuzo
/// int(3.7)        // → 3.0
/// int(-2.9)       // → -2.0
/// int(true)       // → 1.0
/// int(false)      // → 0.0
/// int(nil)        // → 0.0
/// int("42")       // → 42.0
/// int("3.14")     // → 3.0  (先解析为 3.14，再截断)
/// int("hello")    // → Error: type mismatch (期望 numeric string)
/// ```
fn builtin_int(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];

    if val.is_number() {
        return Ok(Value::from_number(val.as_number().trunc()));
    }
    if val.is_bool() {
        return Ok(Value::from_number(if val.is_truthy() { 1.0 } else { 0.0 }));
    }
    if val.is_nil() {
        return Ok(Value::from_number(0.0));
    }
    if val.is_string() {
        let s = val.as_string_opt().unwrap_or_default();
        if let Ok(n) = s.parse::<i64>() {
            return Ok(Value::from_number(n as f64));
        }
        if let Ok(n) = s.parse::<f64>() {
            return Ok(Value::from_number(n.trunc()));
        }
        return Err(NuzoError::type_mismatch("numeric string", format!("\"{}\"", s)));
    }
    Err(NuzoError::type_mismatch("number, string, bool or nil", val.type_name()))
}

/// **float(x)** → number（浮点数转换）
///
/// 将值转换为浮点数表示。
///
/// # 支持的输入类型与转换规则
///
/// | 输入类型 | 转换逻辑 | 示例 |
/// |----------|----------|------|
/// | 数字 (f64) | 原样返回 | `float(3)` → 3.0 |
/// | 布尔 | `true` → 1.0, `false` → 0.0 | `float(true)` → 1.0 |
/// | nil | 固定返回 0.0 | `float(nil)` → 0.0 |
/// | 字符串 | 解析为 f64 | `float("3.14")` → 3.14 |
/// | 其他类型 | 返回 TypeMismatch 错误 | — |
///
/// # 与 int() 的区别
///
/// - `float()` 保留小数部分（不截断）
/// - `float()` 对字符串只接受有效的浮点格式（不接受纯整数格式以外的非法字符）
///
/// # 示例
///
/// ```nuzo
/// float(3)         // → 3.0
/// float(true)      // → 1.0
/// float(nil)       // → 0.0
/// float("2.718")   // → 2.718
/// float("abc")     // → Error: type mismatch
/// ```
fn builtin_float(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];

    if val.is_number() {
        return Ok(Value::from_number(val.as_number()));
    }
    if val.is_bool() {
        return Ok(Value::from_number(if val.is_truthy() { 1.0 } else { 0.0 }));
    }
    if val.is_nil() {
        return Ok(Value::from_number(0.0));
    }
    if val.is_string() {
        let s = val.as_string_opt().unwrap_or_default();
        if let Ok(n) = s.parse::<f64>() {
            return Ok(Value::from_number(n));
        }
        return Err(NuzoError::type_mismatch("numeric string", format!("\"{}\"", s)));
    }
    Err(NuzoError::type_mismatch("number, string, bool or nil", val.type_name()))
}

/// **bool(x)** → bool（布尔转换）
///
/// 根据值的**真值性（truthiness）**转换为布尔值。
///
/// # 真值规则（Truthiness Rules）
///
/// ## Falsy 值（返回 false）
/// - `false`
/// - `nil`
/// - `0` （数字零）
/// - `""` （空字符串）
///
/// ## Truthy 值（返回 true）
/// - 所有其他值，包括：
///   - 非零数字（正数、负数、Infinity、-Infinity）
///   - 非空字符串
///   - 数组（即使是空数组 `[]`）
///   - 字典（即使是空字典 `{}`）
///   - 闭包/函数
///
/// # 设计理念
///
/// 采用**最小意外原则（Principle of Least Surprise）**：
/// - 只将"明显为空/无"的值视为 falsy
/// - 避免类似 JavaScript 中 `[] == false` 的反直觉行为
/// - 空集合（数组、字典）是 truthy 的，因为它们是有效对象
///
/// # 示例
///
/// ```nuzo
/// bool(false)     // → false
/// bool(nil)       // → false
/// bool(0)         // → false
/// bool("")        // → false
/// bool(1)         // → true
/// bool(-1)        // → true
/// bool("hello")   // → true
/// bool([])        // → true  (空数组仍是 truthy)
/// bool({})        // → true  (空字典仍是 truthy)
/// ```
fn builtin_bool(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    Ok(Value::from_bool(args[0].is_truthy()))
}

/// **num(s)** → number（数字解析）
///
/// 将字符串或数字解析为数值。
///
/// # 功能定位
///
/// 此函数是 [`builtin_float`] 的简化版本，主要用于：
/// - 从用户输入读取数字
/// - 解析配置文件中的数值
/// - 数据清洗场景
///
/// # 支持的输入类型
///
/// | 输入类型 | 行为 | 示例 |
/// |----------|------|------|
/// | 数字 | 原样返回（恒等操作）| `num(42)` → 42 |
/// | 字符串 | 解析为 f64 | `num("3.14")` → 3.14 |
/// | 其他类型 | TypeMismatch 错误 | — |
///
/// # 解析能力
///
/// 支持标准 Rust f64 解析格式：
/// - 整数：`"42"`, `"-17"`
/// - 浮点：`"3.14"`, `"-0.5"`, `".25"`
/// - 科学计数法：`"1e10"`, `"2.5E-3"`
/// - 特殊值：`"infinity"`, `"NaN"`（大小写不敏感的部分实现）
///
/// # 错误处理
///
/// 解析失败时返回清晰的错误信息，包含原始字符串值：
/// ```text
/// Error: type mismatch: expected "numeric string", got "abc"
/// ```
///
/// # 示例
///
/// ```nuzo
/// num("42")       // → 42.0
/// num("3.14")     // → 3.14
/// num("-0.5")     // → -0.5
/// num("1e3")      // → 1000.0
/// num(42)         // → 42.0  (直接返回)
/// num("abc")      // → Error
/// ```
fn builtin_num(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];

    if val.is_number() {
        return Ok(*val);
    }
    if val.is_string() {
        let s = val.as_string_opt().unwrap_or_default();
        if let Ok(n) = s.parse::<f64>() {
            return Ok(Value::from_number(n));
        }
        return Err(NuzoError::type_mismatch("numeric string", format!("\"{}\"", s)));
    }
    Err(NuzoError::type_mismatch("string or number", val.type_name()))
}

define_builtin_impl! {
    /// **is_nil(x)** → bool（nil 判断）
    ///
    /// 检测值是否为 `nil`（空值/无值）。
    ///
    /// # 语义
    ///
    /// `nil` 在 Nuzo 中表示"无值"或"缺失"，类似于：
    /// - Python 的 `None`
    /// - JavaScript 的 `null/undefined`
    /// - Java 的 `null`
    /// - Rust 的 `Option::None`
    ///
    /// # 使用场景
    ///
    /// - 可选参数的默认值检测
    /// - 函数返回值的空值检查
    /// - 字典键存在性判断（配合 `keys()` 使用）
    ///
    /// # 示例
    ///
    /// ```nuzo
    /// is_nil(nil)        // → true
    /// is_nil(0)          // → false (0 不是 nil)
    /// is_nil("")         // → false (空字符串不是 nil)
    /// is_nil(false)      // → false (false 不是 nil)
    /// ```
    fn builtin_is_nil(args = args, count = 1) {
        Ok(Value::from_bool(args[0].is_nil()))
    }
}

define_builtin_impl! {
    /// **is_number(x)** → bool（数字类型判断）
    ///
    /// 检测值是否为数字类型（f64 浮点数）。
    ///
    /// # 覆盖范围
    ///
    /// 返回 `true` 的情况：
    /// - 整数：`42`, `0`, `-17`
    /// - 浮点数：`3.14`, `-0.5`, `2.5e10`
    /// - 特殊值：`Infinity`, `-Infinity`, `NaN`
    ///
    /// 返回 `false` 的情况：
    /// - 字符串形式的数字（如 `"42"`）
    /// - 布尔、nil、数组等其他类型
    ///
    /// # 性能说明
    ///
    /// 此检查是 O(1) 操作，仅查看 Value 的类型标记，
    /// 不涉及任何解析或计算。
    ///
    /// # 示例
    ///
    /// ```nuzo
    /// is_number(42)      // → true
    /// is_number(3.14)    // → true
    /// is_number("42")    // → false (是字符串，非数字)
    /// is_number(nil)     // → false
    /// ```
    fn builtin_is_number(args = args, count = 1) {
        Ok(Value::from_bool(args[0].is_number()))
    }
}

define_builtin_impl! {
    /// **is_string(x)** → bool（字符串类型判断）
    ///
    /// 检测值是否为字符串类型。
    ///
    /// # UTF-8 编码保证
    ///
    /// Nuzo 的字符串原生使用 UTF-8 编码，因此：
    /// - 支持完整 Unicode 字符集（包括 emoji、CJK 等）
    /// - 字符长度按 Unicode 码点计算（非字节长度）
    /// - 所有字符串操作（如 `len()`）基于字符而非字节
    ///
    /// # 示例
    ///
    /// ```nuzo
    /// is_string("hello")   // → true
    /// is_string("")        // → true (空字符串仍是字符串)
    /// is_string(42)        // → false
    /// is_string(nil)       // → false
    /// ```
    fn builtin_is_string(args = args, count = 1) {
        Ok(Value::from_bool(args[0].is_string()))
    }
}

/// **is_array(x)** → bool（数组类型判断）
///
/// 检测值是否为数组（有序集合）。
///
/// # 实现细节
///
/// 数组在 Nuzo 中是**堆分配对象**（HeapObject::Array），
/// 因此此函数需要：
/// 1. 检查值是否为堆对象指针
/// 2. 通过 GC 安全地借用堆对象
/// 3. 匹配具体的 HeapObject 变体
///
/// # 性能影响
///
/// 由于涉及堆对象访问，性能略高于基本类型检查（is_number 等），
/// 但仍为 O(1) 操作。
///
/// # 示例
///
/// ```nuzo
/// is_array([1, 2, 3])  // → true
/// is_array([])          // → true (空数组仍是数组)
/// is_array({"a": 1})    // → false (是字典)
/// is_array("hello")     // → false
/// ```
fn builtin_is_array(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];
    if val.is_heap_object()
        && let Some(is_arr) =
            val.with_heap_object(|obj| matches!(obj, nuzo_values::HeapObject::Array(_)))
    {
        return Ok(Value::from_bool(is_arr));
    }
    Ok(Value::from_bool(false))
}

/// **is_dict(x)** → bool（字典类型判断）
///
/// 检测值是否为字典（键值对集合）。
///
/// # 字典特性
///
/// Nuzo 的字典：
/// - 键必须是字符串
/// - 值可以是任意类型
/// - 无序存储（实现细节可能使用 HashMap）
/// - 通过 `keys()` 获取所有键列表
///
/// # 与数组的区别
///
/// | 特性 | 数组 | 字典 |
/// |------|------|------|
/// | 索引方式 | 数字索引 | 字符串键 |
/// | 有序性 | 有序 | 无序 |
/// | 字面量 | `[1,2,3]` | `{"a":1}` |
/// | 典型用途 | 序列数据 | 结构化数据 |
///
/// # 示例
///
/// ```nuzo
/// is_dict({"name": "Alice"})  // → true
/// is_dict({})                 // → true (空字典仍是字典)
/// is_dict([1, 2])             // → false (是数组)
/// is_dict(nil)                // → false
/// ```
fn builtin_is_dict(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let val = &args[0];
    if val.is_heap_object()
        && let Some(is_dict) =
            val.with_heap_object(|obj| matches!(obj, nuzo_values::HeapObject::Dict(_)))
    {
        return Ok(Value::from_bool(is_dict));
    }
    Ok(Value::from_bool(false))
}

/// **is_closure(x)** → bool（闭包/函数类型判断）
///
/// 检测值是否为可调用的闭包或内置函数。
///
/// # 覆盖范围
///
/// 返回 `true` 的情况：
/// - 用户定义的函数/闭包（lambda）
/// - 内置函数（builtin function）
///
/// 返回 `false` 的情况：
/// - 其他所有类型（数字、字符串、数组等）
///
/// # 使用场景
///
/// - 高阶函数参数校验（确保传入的是函数）
/// - 回调模式中的类型守卫
/// - 函数作为一等公民的场景验证
///
/// # 示例
///
/// ```nuzo
/// // 定义一个函数
/// fn add(a, b) { a + b }
///
/// is_closure(add)       // → true
/// is_closure(print)     // → true (内置函数)
/// is_closure(42)        // → false
/// is_closure(nil)       // → false
/// ```
fn builtin_is_closure(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    Ok(Value::from_bool(args[0].is_closure() || args[0].is_builtin_fn()))
}

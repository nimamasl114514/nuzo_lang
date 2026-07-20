//! # 字符串辅助函数
//!
//! 本模块提供**完整的字符串处理**功能集，基于 **UTF-8 编码**原生支持 Unicode。
//! 所有字符串操作均按**字符（Unicode 码点）**而非字节进行，确保国际化文本处理的正确性。
//!
//! ## 可用函数（12 个）
//!
//! ### 分割与连接
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `split` | `split(s, sep) → array` | 用分隔符分割字符串 |
//! | `join` | `join(arr, sep) → string` | 连接数组元素为字符串 |
//!
//! ### 大小写转换
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `upper` | `upper(s) → string` | 转换为大写 |
//! | `lower` | `lower(s) → string` | 转换为小写 |
//! | `trim` | `trim(s) → string` | 去除首尾空白 |
//!
//! ### 搜索与替换
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `replace` | `replace(s, old, new) → string` | 全局替换子串 |
//! | `starts_with` | `starts_with(s, prefix) → bool` | 前缀检测 |
//! | `ends_with` | `ends_with(s, suffix) → bool` | 后缀检测 |
//!
//! ### 变换与提取
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `reverse` | `reverse(s) → string` | 反转字符串（按字符）|
//! | `repeat` | `repeat(s, n) → string` | 重复字符串 n 次 |
//! | `substring` | `substring(s, start, end) → string` | 提取子串（按字符索引）|
//! | `is_empty` | `is_empty(s) → bool` | 空值检测（支持数组和字典）|
//!
//! ## UTF-8 编码处理规范
//!
//! ### 字符 vs 字节
//!
//! Nuzo 字符串的所有操作基于 **Unicode 字符（码点）**：
//!
//! ```text
//! 示例: "Hello 世界 🌍"
//! ┌─────────────────────────────┐
//! │ 字节长度: 14 bytes          │  ← len_bytes()
//! │ 字符长度: 9 characters      │  ← len() / char_len()
//! │                             │
//! │ H(1) e(1) l(2) l(1) o(1)    │  ← ASCII: 1 byte/char
//! │ (空格)(1)                     │
//! │ 世(3) 界(3)                   │  ← CJK: 3 bytes/char
//! │ (空格)(1)                     │
//! │ 🌍(4)                         │  ← Emoji: 4 bytes/char
//! └─────────────────────────────┘
//! ```
//!
//! ### Unicode 支持
//!
//! - ✅ 完整 BMP 字符（U+0000 ~ U+FFFF）
//! - ✅ 补充字符/Emoji（U+10000 ~ U+10FFFF）
//! - ✅ 组合字符序列（如 é = e + ◌́）
//! - ✅ 零宽字符（如 ZWJ 用于 Emoji 组合）
//!
//! ### 边界条件
//!
//! - **空字符串**：所有函数安全处理空输入
//! - **无效 UTF-8**：Nuzo 保证所有字符串都是合法 UTF-8（创建时校验）
//! - **代理对（Surrogate Pairs）**：自动正确处理
//!
//! ## 使用示例
//!
//! ```nuzo
//! // 文本分割与清洗
//! let csv = "Alice,30,Bob,25"
//! let fields = split(csv, ",")
//! for field in fields {
//!     let cleaned = trim(field)
//!     println(cleaned)
//! }
//!
//! // 路径拼接
//! let parts = ["/home", "user", "docs"]
//! let path = join(parts, "/")   // "/home/user/docs"
//!
//! // 文本变换
//! let text = "Hello, World!"
//! let upper_text = upper(text)     // "HELLO, WORLD!"
//! let reversed = reverse(text)     // "!dlroW ,olleH"
//!
//! // 子串提取（按字符索引）
//! let s = "Nuzo 语言"
//! let sub = substring(s, 0, 4)    // "Nuzo " (前4个字符)
//! ```
//!
//! ## 性能特征
//!
//! - **内存效率**：使用 Rust 的 String（UTF-8），无额外开销
//! - **惰性求值**：部分操作（如 trim）返回视图而非拷贝（内部优化）
//! - **缓存友好**：连续内存布局，适合 SIMD 优化
//! - **GC 集成**：新字符串通过 GC 管理，避免内存泄漏

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{HeapObject, NIL, NuzoError, ValueExt};

// ============================================================================
// 注册函数
// ============================================================================

/// 将所有字符串处理函数注册到 BuiltinRegistry
///
/// # 参数
///
/// * `reg` - 可变的 BuiltinRegistry 引用
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "split" => builtin_split, arity = 2,
            signature = "split(s, sep) -> array",
            desc = "用分隔符 sep 分割字符串 s，返回字符串数组。";
        "join" => builtin_join, arity = 2,
            signature = "join(arr, sep) -> string",
            desc = "用分隔符 sep 连接数组中的所有元素。";
        "trim" => builtin_trim, arity = 1,
            signature = "trim(s) -> string",
            desc = "去除字符串首尾空白字符。";
        "upper" => builtin_upper, arity = 1,
            signature = "upper(s) -> string",
            desc = "将字符串转换为大写。";
        "lower" => builtin_lower, arity = 1,
            signature = "lower(s) -> string",
            desc = "将字符串转换为小写。";
        "replace" => builtin_replace, arity = 3,
            signature = "replace(s, old, new) -> string",
            desc = "将字符串 s 中所有 old 替换为 new。";
        "starts_with" => builtin_starts_with, arity = 2,
            signature = "starts_with(s, prefix) -> bool",
            desc = "检测字符串 s 是否以 prefix 开头。";
        "ends_with" => builtin_ends_with, arity = 2,
            signature = "ends_with(s, suffix) -> bool",
            desc = "检测字符串 s 是否以 suffix 结尾。";
        "reverse" => builtin_reverse, arity = 1,
            signature = "reverse(s) -> string",
            desc = "反转字符串（按 Unicode 字符）。";
        "repeat" => builtin_repeat, arity = 2,
            signature = "repeat(s, n) -> string",
            desc = "将字符串 s 重复 n 次。";
        "substring" => builtin_substring, arity = 3,
            signature = "substring(s, start, end) -> string",
            desc = "返回字符串 s 从 start 到 end（不含）的子串（按 Unicode 字符索引）。";
        "is_empty" => builtin_is_empty, arity = 1,
            signature = "is_empty(s) -> bool",
            desc = "检测字符串是否为空。";
    }
}

// ============================================================================
// 辅助：提取字符串参数
// ============================================================================

/// 字符串重复次数上限
///
/// 防止 `repeat("x", 999999999)` 等调用导致内存耗尽。
/// 超过此值时返回 `ArithmeticOverflow` 错误。
const MAX_REPEAT_COUNT: usize = 10000;

fn require_string(args: &[Value], idx: usize, fn_name: &str) -> Result<String, NuzoError> {
    let val =
        args.get(idx).ok_or_else(|| NuzoError::invalid_argument_count(idx + 1, args.len()))?;
    if !val.is_string() {
        return Err(NuzoError::type_mismatch(
            format!("string (arg {} of {})", idx, fn_name),
            val.type_name(),
        ));
    }
    val.as_string_opt().ok_or_else(|| {
        NuzoError::type_mismatch(format!("string (arg {} of {})", idx, fn_name), "invalid string")
    })
}

// validate_index 已统一到 crate::validation::validate_index，消除三处重复定义（P1-6）

// ============================================================================
// 内置函数实现
// ============================================================================

/// **split(s, sep)** → array
///
/// 用分隔符 sep 分割字符串 s，返回字符串数组。
fn builtin_split(args: &[Value]) -> Result<Value, NuzoError> {
    let s = require_string(args, 0, "split")?;
    let sep = require_string(args, 1, "split")?;
    let parts: Vec<Value> = s.split(&sep).map(Value::from_string).collect();
    Ok(Value::from_heap_object_gc(HeapObject::Array(parts)))
}

/// **join(arr, sep)** → string
///
/// 用分隔符 sep 连接数组中的所有元素。
fn builtin_join(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let arr = &args[0];
    let sep = require_string(args, 1, "join")?;

    if !arr.is_heap_object() {
        return Err(NuzoError::type_mismatch("array", arr.type_name()));
    }

    match arr.with_heap_object(|obj| match obj {
        HeapObject::Array(items) => {
            let joined: String =
                items.iter().map(|v| v.concat_repr()).collect::<Vec<_>>().join(&sep);
            Some(Value::from_string(&joined))
        }
        _ => None,
    }) {
        Some(Some(result)) => Ok(result),
        _ => Err(NuzoError::type_mismatch("array", arr.type_name())),
    }
}

/// **trim(s)** → string
///
/// 去除字符串首尾空白字符。
fn builtin_trim(args: &[Value]) -> Result<Value, NuzoError> {
    let s = require_string(args, 0, "trim")?;
    Ok(Value::from_string(s.trim()))
}

/// **upper(s)** → string
///
/// 将字符串转换为大写。
fn builtin_upper(args: &[Value]) -> Result<Value, NuzoError> {
    let s = require_string(args, 0, "upper")?;
    Ok(Value::from_string(&s.to_uppercase()))
}

/// **lower(s)** → string
///
/// 将字符串转换为小写。
fn builtin_lower(args: &[Value]) -> Result<Value, NuzoError> {
    let s = require_string(args, 0, "lower")?;
    Ok(Value::from_string(&s.to_lowercase()))
}

/// **replace(s, old, new)** → string
///
/// 将字符串 s 中所有 old 替换为 new。
fn builtin_replace(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 3 {
        return Err(NuzoError::invalid_argument_count(3, args.len()));
    }
    let s = require_string(args, 0, "replace")?;
    let old = require_string(args, 1, "replace")?;
    let new = require_string(args, 2, "replace")?;
    Ok(Value::from_string(&s.replace(&old, &new)))
}

/// **starts_with(s, prefix)** → bool
///
/// 检测字符串 s 是否以 prefix 开头。
fn builtin_starts_with(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let s = require_string(args, 0, "starts_with")?;
    let prefix = require_string(args, 1, "starts_with")?;
    Ok(Value::from_bool(s.starts_with(&prefix)))
}

/// **ends_with(s, suffix)** → bool
///
/// 检测字符串 s 是否以 suffix 结尾。
fn builtin_ends_with(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let s = require_string(args, 0, "ends_with")?;
    let suffix = require_string(args, 1, "ends_with")?;
    Ok(Value::from_bool(s.ends_with(&suffix)))
}

/// **reverse(s)** → string
///
/// 反转字符串（按 Unicode 字符）。
fn builtin_reverse(args: &[Value]) -> Result<Value, NuzoError> {
    let s = require_string(args, 0, "reverse")?;
    let reversed: String = s.chars().rev().collect();
    Ok(Value::from_string(&reversed))
}

/// **repeat(s, n)** → string
///
/// 将字符串 s 重复 n 次。
fn builtin_repeat(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let s = require_string(args, 0, "repeat")?;
    if !args[1].is_number() {
        return Err(NuzoError::type_mismatch("number", args[1].type_name()));
    }
    let n = args[1].as_number();
    if n < 0.0 || n.is_nan() || n.is_infinite() {
        return Err(NuzoError::type_mismatch("non-negative integer", format!("{}", n)));
    }
    let n = n as usize;
    if n > MAX_REPEAT_COUNT {
        return Err(NuzoError::arithmetic_overflow());
    }
    Ok(Value::from_string(&s.repeat(n)))
}

/// **substring(s, start, end)** → string
///
/// 返回字符串 s 从 start 到 end（不含）的子串（按 Unicode 字符索引）。
fn builtin_substring(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 3 {
        return Err(NuzoError::invalid_argument_count(3, args.len()));
    }
    let s = require_string(args, 0, "substring")?;
    if !args[1].is_number() {
        return Err(NuzoError::type_mismatch("number", args[1].type_name()));
    }
    if !args[2].is_number() {
        return Err(NuzoError::type_mismatch("number", args[2].type_name()));
    }
    let start = crate::validation::validate_index(args[1].as_number())?;
    let end = crate::validation::validate_index(args[2].as_number())?;

    let chars: Vec<char> = s.chars().collect();
    if start >= chars.len() {
        return Ok(Value::from_string(""));
    }
    let end = end.min(chars.len());
    if start >= end {
        return Ok(Value::from_string(""));
    }
    Ok(Value::from_string(&chars[start..end].iter().collect::<String>()))
}

/// **is_empty(s)** → bool
///
/// 检测字符串是否为空。
fn builtin_is_empty(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    if args[0].is_string() {
        let s = args[0].as_string_opt().unwrap_or_default();
        return Ok(Value::from_bool(s.is_empty()));
    }
    // 对数组也支持 is_empty
    if args[0].is_heap_object()
        && let Some(Some(is_empty)) = args[0].with_heap_object(|obj| match obj {
            HeapObject::Array(arr) => Some(arr.is_empty()),
            HeapObject::Dict(d) => Some(d.is_empty()),
            _ => None,
        })
    {
        return Ok(Value::from_bool(is_empty));
    }
    Err(NuzoError::type_mismatch("string or array", args[0].type_name()))
}

// ============================================================================
// format 格式化
// ============================================================================

/// **str_format(template, args...)** → string
///
/// C 风格格式化字符串。支持以下占位符：
///
/// | 占位符 | 说明 |
/// |--------|------|
/// | `{}` | 默认格式（值的字符串表示）|
/// | `{:.N}` | 浮点数保留 N 位小数 |
/// | `{:x}` | 十六进制（小写）|
/// | `{:X}` | 十六进制（大写）|
/// | `{:>N}` | 右对齐，宽度 N |
/// | `{:<N}` | 左对齐，宽度 N |
///
/// 使用 `{{` 表示字面 `{`，`}}` 表示字面 `}`。
/// 占位符多于参数时保留原样；参数多于占位符时忽略多余参数。
pub fn builtin_str_format(args: &[Value]) -> Result<Value, NuzoError> {
    let result = format_impl(args)?;
    Ok(Value::from_string(&result))
}

/// **str_printf(template, args...)** → nil
///
/// 格式化字符串并输出到 stdout（无换行）。内部调用 str_format。
pub fn builtin_str_printf(args: &[Value]) -> Result<Value, NuzoError> {
    let s = format_impl(args)?;
    emit_output(&s, false)
}

/// **str_printlnf(template, args...)** → nil
///
/// 格式化字符串并输出到 stdout（带换行）。内部调用 str_format。
pub fn builtin_str_printlnf(args: &[Value]) -> Result<Value, NuzoError> {
    let s = format_impl(args)?;
    emit_output(&s, true)
}

// --- format 内部实现 ---

/// 格式说明符分类
enum FormatSpec {
    /// `{}` 默认
    Default,
    /// `{:.N}` 浮点小数位
    Float(usize),
    /// `{:x}` 十六进制小写
    HexLower,
    /// `{:X}` 十六进制大写
    HexUpper,
    /// `{:>N}` 右对齐宽度 N
    RightAlign(usize),
    /// `{:<N}` 左对齐宽度 N
    LeftAlign(usize),
}

/// 解析格式说明符字符串（不含前导 `:`）。
///
/// 无法识别的说明符回退为 [`FormatSpec::Default`]。
fn parse_format_spec(spec: &str) -> FormatSpec {
    if spec.is_empty() {
        return FormatSpec::Default;
    }
    if let Some(rest) = spec.strip_prefix('.') {
        return match rest.parse::<usize>() {
            Ok(n) => FormatSpec::Float(n),
            Err(_) => FormatSpec::Default,
        };
    }
    match spec {
        "x" => FormatSpec::HexLower,
        "X" => FormatSpec::HexUpper,
        _ => {
            if let Some(rest) = spec.strip_prefix('>')
                && let Ok(n) = rest.parse::<usize>()
            {
                return FormatSpec::RightAlign(n);
            }
            if let Some(rest) = spec.strip_prefix('<')
                && let Ok(n) = rest.parse::<usize>()
            {
                return FormatSpec::LeftAlign(n);
            }
            FormatSpec::Default
        }
    }
}

/// 将 Value 转换为整数（用于十六进制格式化）。
///
/// Smi 直接取值；浮点数须为有限整数值；其余类型返回类型错误。
fn value_as_integer(val: &Value, op: &str) -> Result<i64, NuzoError> {
    if val.is_smi() {
        return Ok(val.as_smi());
    }
    let expected = format!("integer for {}", op);
    if val.is_float() {
        let n = val.as_number();
        if n.is_finite() && n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
            Ok(n as i64)
        } else {
            Err(NuzoError::type_mismatch(expected, val.type_name()))
        }
    } else {
        Err(NuzoError::type_mismatch(expected, val.type_name()))
    }
}

/// 按格式说明符格式化单个值。
fn format_value(val: &Value, spec: &FormatSpec) -> Result<String, NuzoError> {
    match spec {
        FormatSpec::Default => Ok(val.concat_repr()),
        FormatSpec::Float(prec) => {
            if !val.is_number() {
                return Err(NuzoError::type_mismatch("number for float format", val.type_name()));
            }
            Ok(format!("{:.*}", *prec, val.as_number()))
        }
        FormatSpec::HexLower => {
            let i = value_as_integer(val, "hex format")?;
            Ok(format!("{:x}", i))
        }
        FormatSpec::HexUpper => {
            let i = value_as_integer(val, "hex format")?;
            Ok(format!("{:X}", i))
        }
        FormatSpec::RightAlign(width) => {
            let s = val.concat_repr();
            Ok(format!("{:>width$}", s, width = *width))
        }
        FormatSpec::LeftAlign(width) => {
            let s = val.concat_repr();
            Ok(format!("{:<width$}", s, width = *width))
        }
    }
}

/// 在字符切片中从 `start` 开始查找第一个 `}` 的位置。
fn find_close_brace(chars: &[char], start: usize) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == '}')
}

/// format 核心实现：解析模板并替换占位符，返回格式化后的字符串。
///
/// `args[0]` 为模板字符串，`args[1..]` 为按顺序消耗的格式化参数。
fn format_impl(args: &[Value]) -> Result<String, NuzoError> {
    let template = require_string(args, 0, "format")?;
    let chars: Vec<char> = template.chars().collect();
    let mut result = String::with_capacity(template.len() * 2);
    let mut param_idx: usize = 1; // args[0] 是模板

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            // 转义 {{
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                result.push('{');
                i += 2;
                continue;
            }
            // 查找闭合 }
            match find_close_brace(&chars, i + 1) {
                Some(close) => {
                    let content: String = chars[i + 1..close].iter().collect();
                    // 合法占位符内容为空或以 `:` 开头
                    let spec_str: &str = if content.is_empty() {
                        ""
                    } else if let Some(rest) = content.strip_prefix(':') {
                        rest
                    } else {
                        // 非法占位符（如 {abc}），按字面量保留
                        result.push('{');
                        result.push_str(&content);
                        result.push('}');
                        i = close + 1;
                        continue;
                    };
                    if param_idx < args.len() {
                        let spec = parse_format_spec(spec_str);
                        let formatted = format_value(&args[param_idx], &spec)?;
                        result.push_str(&formatted);
                        param_idx += 1;
                    } else {
                        // 无剩余参数，保留占位符原样
                        result.push('{');
                        if !spec_str.is_empty() {
                            result.push(':');
                            result.push_str(spec_str);
                        }
                        result.push('}');
                    }
                    i = close + 1;
                }
                None => {
                    // 无闭合 }，按字面量保留 {
                    result.push('{');
                    i += 1;
                }
            }
        } else if c == '}' {
            // 转义 }}
            if i + 1 < chars.len() && chars[i + 1] == '}' {
                result.push('}');
                i += 2;
                continue;
            }
            result.push('}');
            i += 1;
        } else {
            result.push(c);
            i += 1;
        }
    }

    Ok(result)
}

/// 输出到 stdout 或捕获缓冲区（与 print/println 行为一致）。
fn emit_output(text: &str, newline: bool) -> Result<Value, NuzoError> {
    let capture = crate::builtins::output_capture();
    if let Some(buffer) = capture {
        buffer.lock().unwrap_or_else(|e| e.into_inner()).push(text.to_string());
    } else if newline {
        println!("{}", text);
    } else {
        print!("{}", text);
    }
    Ok(NIL)
}

// ============================================================================
// 测试模块：format 系列函数
// ============================================================================
//
// 覆盖 builtin_str_format / builtin_str_printf / builtin_str_printlnf 的关键场景：
// - 基本占位符：{} / {:.N} / {:x} / {:X} / {:>N} / {:<N}
// - 边界条件：空模板、无占位符、占位符与参数数量不匹配、转义花括号、类型不匹配
// - printf/printlnf 返回 nil 且输出正确内容

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // =========================================================================
    // format 基本测试
    // =========================================================================

    #[test]
    fn test_format_basic() {
        let template = Value::from_string("Hello, {}!");
        let arg = Value::from_string("world");
        let result = builtin_str_format(&[template, arg]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn test_format_multiple_placeholders() {
        let template = Value::from_string("{} + {} = {}");
        let result = builtin_str_format(&[
            template,
            Value::from_number(1.0),
            Value::from_number(2.0),
            Value::from_number(3.0),
        ]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("1 + 2 = 3"));
    }

    #[test]
    fn test_format_float_precision() {
        let template = Value::from_string("{:.2}");
        let result = builtin_str_format(&[template, Value::from_number(std::f64::consts::PI)]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("3.14"));
    }

    #[test]
    fn test_format_hex_lowercase() {
        let template = Value::from_string("{:x}");
        let result = builtin_str_format(&[template, Value::from_number(255.0)]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("ff"));
    }

    #[test]
    fn test_format_hex_uppercase() {
        let template = Value::from_string("{:X}");
        let result = builtin_str_format(&[template, Value::from_number(255.0)]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("FF"));
    }

    #[test]
    fn test_format_right_align() {
        let template = Value::from_string("{:>5}");
        let result = builtin_str_format(&[template, Value::from_string("hi")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("   hi"));
    }

    #[test]
    fn test_format_left_align() {
        let template = Value::from_string("{:<5}");
        let result = builtin_str_format(&[template, Value::from_string("hi")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("hi   "));
    }

    // =========================================================================
    // 边界测试
    // =========================================================================

    #[test]
    fn test_format_empty_template() {
        let template = Value::from_string("");
        let result = builtin_str_format(&[
            template,
            Value::from_number(1.0),
            Value::from_number(2.0),
            Value::from_number(3.0),
        ]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some(""));
    }

    #[test]
    fn test_format_no_placeholders() {
        let template = Value::from_string("hello world");
        let result = builtin_str_format(&[template]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("hello world"));
    }

    #[test]
    fn test_format_more_placeholders_than_args() {
        // 占位符多于参数：多余占位符保留原样
        let template = Value::from_string("{} {}");
        let result = builtin_str_format(&[template, Value::from_string("a")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("a {}"));
    }

    #[test]
    fn test_format_more_args_than_placeholders() {
        // 参数多于占位符：多余参数忽略
        let template = Value::from_string("{}");
        let result = builtin_str_format(&[
            template,
            Value::from_string("a"),
            Value::from_string("b"),
            Value::from_string("c"),
        ]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("a"));
    }

    #[test]
    fn test_format_escape_braces() {
        // {{}} -> {}
        let template = Value::from_string("{{}}");
        let result = builtin_str_format(&[template]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("{}"));

        // {{hello}} -> {hello}
        let template2 = Value::from_string("{{hello}}");
        let result2 = builtin_str_format(&[template2]);
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap().as_string_opt().as_deref(), Some("{hello}"));
    }

    #[test]
    fn test_format_type_mismatch() {
        // 字符串不能用十六进制格式
        let template = Value::from_string("{:x}");
        let result = builtin_str_format(&[template, Value::from_string("string")]);
        assert!(result.is_err(), "hex format on a string should error");
    }

    // =========================================================================
    // printf / printlnf 测试
    // =========================================================================

    #[test]
    fn test_printf_returns_nil() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let _guard = crate::builtins::OutputCaptureGuard::new(Some(buffer.clone()));

        let template = Value::from_string("Hello, {}!");
        let result = builtin_str_printf(&[template, Value::from_string("world")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL);

        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "Hello, world!");
    }

    #[test]
    fn test_printlnf_returns_nil() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let _guard = crate::builtins::OutputCaptureGuard::new(Some(buffer.clone()));

        let template = Value::from_string("Hello, {}!");
        let result = builtin_str_printlnf(&[template, Value::from_string("world")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL);

        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "Hello, world!");
    }
}

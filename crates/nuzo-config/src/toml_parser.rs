//! 轻量 TOML 解析器 — 零依赖，仅支持 nuzo.toml 所需的子集
//!
//! 支持特性：
//! - `[section]` 和 `[section.subsection]` 表
//! - `key = value` 键值对
//! - 值类型：整数、浮点、布尔、字符串（双引号/单引号）
//! - `#` 行注释
//! - 多行字符串（三引号）
//!
//! 不支持（不需要）：
//! - 数组、内联表、日期时间
//! - 键的点号路径（如 `gc.threshold = 10`）

use std::collections::HashMap;
use std::fmt;

use crate::error::{ConfigError, ConfigResult};
use crate::value::ConfigValue;

/// TOML 文档解析结果：扁平化的 key → value 映射
///
/// 键格式：`section.key` 或 `section.subsection.key`
/// 例如：`gc.threshold`、`vm.max_stack_size`、`arena.enabled`
#[derive(Debug, Clone, Default)]
pub struct TomlTable {
    values: HashMap<String, ConfigValue>,
}

impl TomlTable {
    /// 创建空表
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析 TOML 字符串
    pub fn parse(input: &str) -> ConfigResult<Self> {
        let mut table = Self::new();
        let mut current_section = String::new();

        for (line_idx, raw_line) in input.lines().enumerate() {
            let line_no = line_idx + 1;
            let line = strip_comment(raw_line);
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') {
                current_section = parse_section_header(line, line_no)?;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let val_str = line[eq_pos + 1..].trim();

                if key.is_empty() {
                    return Err(ConfigError::TomlParse {
                        line: line_no,
                        message: "empty key".to_string(),
                    });
                }

                let value = parse_value(val_str, line_no)?;
                let full_key = if current_section.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", current_section, key)
                };
                table.values.insert(full_key, value);
                continue;
            }

            return Err(ConfigError::TomlParse {
                line: line_no,
                message: format!("unexpected syntax: {:?}", line),
            });
        }

        Ok(table)
    }

    /// 获取值
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.values.get(key)
    }

    /// 获取所有键
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }

    /// 合并另一个表（other 覆盖 self）
    pub fn merge(&mut self, other: &TomlTable) {
        for (k, v) in &other.values {
            self.values.insert(k.clone(), v.clone());
        }
    }

    /// 键值对数量
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 插入单条值
    pub fn insert(&mut self, key: &str, value: ConfigValue) {
        self.values.insert(key.to_string(), value);
    }
}

impl fmt::Display for TomlTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut keys: Vec<_> = self.values.keys().collect();
        keys.sort();
        for key in keys {
            let val = &self.values[key];
            writeln!(f, "{} = {}", key, val)?;
        }
        Ok(())
    }
}

/// 去除行注释
///
/// `#` 是注释起始，但字符串内的 `#` 不是。
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut quote_char = '\0';
    let mut escape = false;

    for (i, ch) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if !in_string {
            if ch == '"' || ch == '\'' {
                in_string = true;
                quote_char = ch;
            } else if ch == '#' {
                return &line[..i];
            }
        } else if ch == quote_char {
            in_string = false;
        }
    }
    line
}

/// 解析表头 `[section]` 或 `[[array]]`（后者不支持，报错）
fn parse_section_header(line: &str, line_no: usize) -> ConfigResult<String> {
    let line = line.trim();
    if !line.ends_with(']') {
        return Err(ConfigError::TomlParse {
            line: line_no,
            message: "unclosed section header".to_string(),
        });
    }
    if line.starts_with("[[") {
        return Err(ConfigError::TomlParse {
            line: line_no,
            message: "array of tables ([[...]]) is not supported".to_string(),
        });
    }
    let section = &line[1..line.len() - 1];
    let section = section.trim();
    if section.is_empty() {
        return Err(ConfigError::TomlParse {
            line: line_no,
            message: "empty section name".to_string(),
        });
    }
    Ok(section.to_string())
}

/// 解析值
fn parse_value(s: &str, line_no: usize) -> ConfigResult<ConfigValue> {
    let s = s.trim();

    if s.starts_with("\"\"\"") || s.starts_with("'''") {
        let quote = &s[..3];
        if s.ends_with(quote) && s.len() > 6 {
            let inner = &s[3..s.len() - 3];
            return Ok(ConfigValue::Text(inner.to_string()));
        }
        return Err(ConfigError::TomlParse {
            line: line_no,
            message: "unclosed multi-line string".to_string(),
        });
    }

    if s.starts_with('"') {
        return parse_quoted_string(s, '"', line_no);
    }

    // 单引号字符串（字面量）
    if s.starts_with('\'') {
        return parse_quoted_string(s, '\'', line_no);
    }

    match s {
        "true" => return Ok(ConfigValue::Boolean(true)),
        "false" => return Ok(ConfigValue::Boolean(false)),
        _ => {}
    }

    // 整数（支持 _ 分隔符和 0x/0o/0b 前缀）
    let cleaned = s.replace('_', "");
    match cleaned.parse::<i64>() {
        Ok(n) => return Ok(ConfigValue::Integer(n)),
        Err(e) => {
            // 仅当不是「数字格式错误」而是溢出时，立即报错（避免被浮点解析静默吞掉）
            if parse_int_overflow_kind(&e).is_some() {
                return Err(ConfigError::TomlParse {
                    line: line_no,
                    message: format!("integer overflow: {:?} exceeds i64 range", s),
                });
            }
            // 否则不是整数格式，继续尝试其他类型
        }
    }
    if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
        match i64::from_str_radix(&cleaned[2..], 16) {
            Ok(n) => return Ok(ConfigValue::Integer(n)),
            Err(e) if parse_int_overflow_kind(&e).is_some() => {
                return Err(ConfigError::TomlParse {
                    line: line_no,
                    message: format!("hex integer overflow: {:?} exceeds i64 range", s),
                });
            }
            _ => {}
        }
    }
    if cleaned.starts_with("0o") || cleaned.starts_with("0O") {
        match i64::from_str_radix(&cleaned[2..], 8) {
            Ok(n) => return Ok(ConfigValue::Integer(n)),
            Err(e) if parse_int_overflow_kind(&e).is_some() => {
                return Err(ConfigError::TomlParse {
                    line: line_no,
                    message: format!("octal integer overflow: {:?} exceeds i64 range", s),
                });
            }
            _ => {}
        }
    }
    if cleaned.starts_with("0b") || cleaned.starts_with("0B") {
        match i64::from_str_radix(&cleaned[2..], 2) {
            Ok(n) => return Ok(ConfigValue::Integer(n)),
            Err(e) if parse_int_overflow_kind(&e).is_some() => {
                return Err(ConfigError::TomlParse {
                    line: line_no,
                    message: format!("binary integer overflow: {:?} exceeds i64 range", s),
                });
            }
            _ => {}
        }
    }

    if let Ok(f) = cleaned.parse::<f64>() {
        return Ok(ConfigValue::Float(f));
    }

    Err(ConfigError::TomlParse { line: line_no, message: format!("cannot parse value: {:?}", s) })
}

/// 判断 `std::num::ParseIntError` 是否为溢出错误
///
/// 返回 `Some(true)` 表示正溢出，`Some(false)` 表示负溢出，`None` 表示非溢出错误
/// （例如空字符串、无效字符等）。
fn parse_int_overflow_kind(e: &std::num::ParseIntError) -> Option<bool> {
    use std::num::IntErrorKind::*;
    match e.kind() {
        PosOverflow => Some(true),
        NegOverflow => Some(false),
        _ => None,
    }
}

/// 解析引号字符串
fn parse_quoted_string(s: &str, quote: char, line_no: usize) -> ConfigResult<ConfigValue> {
    if !s.ends_with(quote) || s.len() < 2 {
        return Err(ConfigError::TomlParse {
            line: line_no,
            message: format!("unclosed string: {:?}", s),
        });
    }
    let inner = &s[1..s.len() - 1];

    // 单引号 = 字面量，不处理转义
    if quote == '\'' {
        return Ok(ConfigValue::Text(inner.to_string()));
    }

    // 双引号 = 处理转义序列
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('0') => result.push('\0'),
                Some('u') => {
                    // 支持 \u{XXXX} 形式的 Unicode 转义（与 Rust 字符串字面量一致）
                    // 例如："\u{4e2d}" 表示中文字符 "中"
                    let mut code_point = String::new();
                    let mut found_brace = false;
                    let mut next_char = chars.next();
                    if next_char == Some('{') {
                        found_brace = true;
                        next_char = chars.next();
                    }
                    while let Some(c) = next_char {
                        if c == '}' {
                            break;
                        }
                        code_point.push(c);
                        next_char = chars.next();
                    }
                    if !found_brace {
                        return Err(ConfigError::TomlParse {
                            line: line_no,
                            message: "\\u escape requires brace form: \\u{XXXX}".to_string(),
                        });
                    }
                    let code = u32::from_str_radix(&code_point, 16).map_err(|_| {
                        ConfigError::TomlParse {
                            line: line_no,
                            message: format!("invalid unicode escape: \\u{{{}}}", code_point),
                        }
                    })?;
                    let c = char::from_u32(code).ok_or_else(|| ConfigError::TomlParse {
                        line: line_no,
                        message: format!(
                            "invalid unicode code point: \\u{{{}}} (not a valid scalar value)",
                            code_point
                        ),
                    })?;
                    result.push(c);
                }
                Some(c) => {
                    result.push('\\');
                    result.push(c);
                }
                None => {
                    return Err(ConfigError::TomlParse {
                        line: line_no,
                        message: "trailing backslash in string".to_string(),
                    });
                }
            }
        } else {
            result.push(ch);
        }
    }

    Ok(ConfigValue::Text(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let table = TomlTable::parse("").unwrap();
        assert!(table.is_empty());
    }

    #[test]
    fn parse_comments() {
        let input = "# comment\nkey = 42 # inline comment\n";
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("key").unwrap().as_i64(), Some(42));
    }

    #[test]
    fn parse_section() {
        let input = "[gc]\nthreshold = 1024\nrate = 8\n";
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("gc.threshold").unwrap().as_i64(), Some(1024));
        assert_eq!(table.get("gc.rate").unwrap().as_i64(), Some(8));
    }

    #[test]
    fn parse_all_types() {
        let input = r#"
int_val = 42
neg_val = -100
hex_val = 0xFF
float_val = 2.5
bool_val = true
str_dq = "hello\nworld"
str_sq = 'raw string'
underscore = 1_000_000
"#;
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("int_val").unwrap().as_i64(), Some(42));
        assert_eq!(table.get("neg_val").unwrap().as_i64(), Some(-100));
        assert_eq!(table.get("hex_val").unwrap().as_i64(), Some(255));
        assert_eq!(table.get("float_val").unwrap().as_f64(), Some(2.5));
        assert_eq!(table.get("bool_val").unwrap().as_bool(), Some(true));
        assert_eq!(table.get("str_dq").unwrap().as_str(), Some("hello\nworld"));
        assert_eq!(table.get("str_sq").unwrap().as_str(), Some("raw string"));
        assert_eq!(table.get("underscore").unwrap().as_i64(), Some(1_000_000));
    }

    #[test]
    fn parse_nuzo_toml() {
        let input = r#"
# Nuzo 配置文件
[vm]
max_stack_size = 131072
max_call_frames = 2000000

[gc]
threshold = 20_000_000
growth_factor = 2
survival_ratio = 0.5

[compiler]
max_locals = 65535
max_function_locals = 4096

[arena]
max_frame_arena_size = 131072
max_region_size = 33554432
enabled = true

[frame_paging]
capacity = 400
low_watermark = 100
spill_batch = 200
"#;
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("vm.max_stack_size").unwrap().as_usize(), Some(131072));
        assert_eq!(table.get("gc.threshold").unwrap().as_usize(), Some(20_000_000));
        assert_eq!(table.get("gc.survival_ratio").unwrap().as_f64(), Some(0.5));
        assert_eq!(table.get("arena.enabled").unwrap().as_bool(), Some(true));
        assert_eq!(table.get("frame_paging.capacity").unwrap().as_usize(), Some(400));
    }

    #[test]
    fn merge_tables() {
        let mut a = TomlTable::parse("key1 = 1\nkey2 = 2").unwrap();
        let b = TomlTable::parse("key2 = 99\nkey3 = 3").unwrap();
        a.merge(&b);
        assert_eq!(a.get("key1").unwrap().as_i64(), Some(1));
        assert_eq!(a.get("key2").unwrap().as_i64(), Some(99)); // 覆盖
        assert_eq!(a.get("key3").unwrap().as_i64(), Some(3));
    }

    #[test]
    fn error_unclosed_section() {
        let result = TomlTable::parse("[section");
        assert!(result.is_err());
    }

    #[test]
    fn error_array_of_tables() {
        let result = TomlTable::parse("[[array]]");
        assert!(result.is_err());
    }

    #[test]
    fn strip_comment_in_string() {
        let line = r#"key = "value # not a comment""#;
        assert_eq!(strip_comment(line), line);
    }

    #[test]
    fn parse_unicode_escape() {
        // \u{XXXX} Unicode 转义（与 Rust 字符串字面量一致）
        let input = r#"key = "\u{4e2d}\u{6587}""#; // "中文"
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("key").unwrap().as_str(), Some("中文"));
    }

    #[test]
    fn parse_unicode_escape_mixed() {
        // 混合普通字符与 Unicode 转义
        let input = r#"key = "Hello \u{4e16}\u{754c}!""#; // "Hello 世界!"
        let table = TomlTable::parse(input).unwrap();
        assert_eq!(table.get("key").unwrap().as_str(), Some("Hello 世界!"));
    }

    #[test]
    fn error_invalid_unicode_escape() {
        // \u 后必须跟 {XXXX} 形式
        let input = r#"key = "\uZZZZ""#;
        let result = TomlTable::parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn error_integer_overflow_decimal() {
        // 超出 i64 范围的十进制整数应报错，不应静默回退为浮点数
        let input = "key = 99999999999999999999999999999999";
        let result = TomlTable::parse(input);
        assert!(result.is_err(), "expected integer overflow error, got {:?}", result);
    }

    #[test]
    fn error_integer_overflow_hex() {
        // 超出 i64 范围的十六进制整数应报错
        let input = "key = 0xFFFFFFFFFFFFFFFFFF";
        let result = TomlTable::parse(input);
        assert!(result.is_err(), "expected hex overflow error, got {:?}", result);
    }
}

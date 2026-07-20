//! # ValueLayout -- NaN-tagged Value 位布局诊断模块
//!
//! 本模块提供 [`ValueLayout`] 结构体，用于解析和可视化 [`Value`] 的 64 位 NaN 标记编码。
//!
//! ## 设计目标
//!
//! 1. **诊断辅助**：将 u64 位模式解码为人类可读的类型标签 + 有效载荷描述
//! 2. **可视化**：生成 ASCII 位域图，直观展示 Tag / Payload / GC-bit 等字段
//! 3. **合法性校验**：检测非法位模式（如落入 NaN 空间但不符合任何已知标签）
//!
//! ## 使用示例
//!
//! ```ignore
//! use nuzo_values::{Value, ValueLayout};
//!
//! let val = Value::from_smi(42);
//! let layout = ValueLayout::from_value(val);
//! println!("{}", layout.format_visual());
//! assert!(layout.validate().is_ok());
//! ```

use crate::constants::*;
use crate::value::Value;

// ============================================================================
// 合法性校验阈值常量
// ============================================================================

/// 堆索引合理上限。
///
/// 实际堆索引为 45 位无符号整数（最大 2^45-1 ≈ 35 万亿），
/// 但运行时堆大小远小于此值。此处设为 2^24 (16M) 作为合理上限，
/// 超过此值几乎可以确定是位模式损坏。
const HEAP_INDEX_SANITY_LIMIT: u64 = 1u64 << 24;

/// 字符串索引合理上限。
///
/// 字符串索引为 47 位无符号整数，同理设为 2^24 (16M) 作为合理上限。
const STRING_INDEX_SANITY_LIMIT: u64 = 1u64 << 24;

// ============================================================================
// layout.rs 专属命名常量 -- 消灭魔法数字
// ============================================================================

/// Nil/Bool 类型载荷掩码：bits[48:0] 共 49 位。
///
/// 用于 `format_visual` 中提取 Nil/Bool 的完整载荷字段。
const NIL_BOOL_PAYLOAD_MASK: u64 = (1u64 << 49) - 1;

/// IEEE 754 双精度浮点数尾数掩码（低 52 位）。
const MANTISSA_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

/// IEEE 754 双精度浮点数指数掩码（11 位，bits[62:52]）。
///
/// 用于 `format_visual` Float 分支中提取指数字段。
const IEEE754_EXP_MASK: u64 = 0x7FF;

/// Nil/Bool 特殊值低位载荷掩码（低 4 位）。
///
/// Nil/Bool 值的低 4 位编码了具体子类型：
/// - 1 = Nil
/// - 2 = false
/// - 3 = true
const LOW_NIBBLE_MASK: u64 = 0xF;

/// 浮点数以整数格式显示的绝对值阈值。
///
/// 当 `|n| < FLOAT_INTEGER_DISPLAY_THRESHOLD` 且 `n.fract() == 0.0` 时，
/// 使用 `format!("{:.1}", n)` 显示（如 `42.0`），避免大数科学计数法。
const FLOAT_INTEGER_DISPLAY_THRESHOLD: f64 = 1e15;

/// 位域可视化表格输出字符串的初始预分配容量。
///
/// 典型的表格包含头部行 + 2-3 行数据 + 边框，约 150-250 字节，
/// 256 字节预分配可避免大多数重分配。
const LAYOUT_OUTPUT_CAPACITY: usize = 256;

// ============================================================================
// ValueLayout 结构体
// ============================================================================

/// NaN-tagged Value 的位布局诊断信息。
///
/// 将 [`Value`] 的 u64 位模式解码为结构化的类型标签、有效载荷、
/// GC 管理标志和合法性状态，便于调试和可视化。
///
/// # 不变量
///
/// - `tag` 始终为非空静态字符串
/// - `raw_bits` 始终为 16 位十六进制格式（`0x7FF9_0000_0000_002A`）
/// - `gc_managed` 仅对 HeapObject 类型有实际意义，其他类型始终为 `false`
/// - `is_valid` 为 `false` 时，`validate()` 必定返回 `Err`
#[derive(Debug, Clone)]
pub struct ValueLayout {
    /// 类型标签名称（如 "Nil", "Smi", "Float" 等）
    pub tag: &'static str,
    /// 有效载荷描述（如 "42", "index=5", "addr=0x7FFE1234" 等）
    pub payload: String,
    /// 原始位模式十六进制表示
    pub raw_bits: String,
    /// 原始位模式的 u64 值，避免字符串往返解析。
    ///
    /// `format_visual()` 和 `validate()` 直接使用此字段，
    /// 不再从 `raw_bits` 字符串反向解析。
    pub raw_bits_u64: u64,
    /// 是否为 GC 管理的堆对象
    pub gc_managed: bool,
    /// 位模式是否合法（符合已知编码规则）
    pub is_valid: bool,
}

impl ValueLayout {
    // ========================================================================
    // 内部辅助：raw_bits 十六进制解析
    // ========================================================================

    /// 将 `raw_bits` 字段从十六进制字符串解析为 u64。
    ///
    /// 直接返回缓存的原始位模式 u64 值。
    ///
    /// 旧实现从 `raw_bits` 字符串反向解析，现在直接使用 `raw_bits_u64` 字段，
    /// 消除字符串往返解析的开销。
    fn parse_raw_bits(&self) -> u64 {
        self.raw_bits_u64
    }

    // ========================================================================
    // 核心解析：from_value
    // ========================================================================

    /// 从 [`Value`] 解析位布局诊断信息。
    ///
    /// 纯函数，无副作用。覆盖所有已知类型标签的解码逻辑，
    /// 未识别的位模式归类为 "Unknown"。
    ///
    /// # 类型映射
    ///
    /// | Value 类型 | tag      | payload 格式              |
    /// |-----------|----------|--------------------------|
    /// | Nil       | "Nil"    | "singleton"              |
    /// | Bool      | "Bool"   | "true" / "false"         |
    /// | Smi       | "Smi"    | 整数值，如 "42"           |
    /// | Float     | "Float"  | 浮点值，如 "3.14"         |
    /// | String    | "String" | "index=N"                |
    /// | HeapObject| "HeapObject" | "index=N, gc=true/false" |
    /// | Pointer   | "Pointer"| "addr=0xHEXADDR"        |
    /// | Unknown   | "Unknown"| "unclassified"           |
    pub fn from_value(value: Value) -> Self {
        let bits = value.into_raw_bits();
        let raw_bits = format_raw_bits(bits);

        // 按优先级依次检测类型（与 Value::tag() 逻辑一致）
        if value.is_nil() {
            return Self {
                tag: "Nil",
                payload: "singleton".to_string(),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: true,
            };
        }

        if value.is_bool() {
            return Self {
                tag: "Bool",
                payload: if value.as_bool() { "true" } else { "false" }.to_string(),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: true,
            };
        }

        if value.is_smi() {
            return Self {
                tag: "Smi",
                payload: format!("{}", value.as_smi()),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: true,
            };
        }

        if value.is_float() {
            let n = value.as_number();
            return Self {
                tag: "Float",
                payload: format_float(n),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: true,
            };
        }

        if value.is_string() {
            let idx = (bits & STRING_INDEX_MASK) as u32;
            let valid = (idx as u64) < STRING_INDEX_SANITY_LIMIT;
            return Self {
                tag: "String",
                payload: format!("index={}", idx),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: valid,
            };
        }

        if value.is_heap_object() {
            let gc = value.is_gc_managed();
            let idx = (bits & HEAP_INDEX_MASK_NO_GC) as u32;
            let valid = (idx as u64) < HEAP_INDEX_SANITY_LIMIT;
            return Self {
                tag: "HeapObject",
                payload: format!("index={}, gc={}", idx, gc),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: gc,
                is_valid: valid,
            };
        }

        if value.is_ptr() {
            let addr = (bits & PTR_MASK) as usize;
            return Self {
                tag: "Pointer",
                payload: format!("addr={:#010X}", addr),
                raw_bits,
                raw_bits_u64: bits,
                gc_managed: false,
                is_valid: true,
            };
        }

        // 落入 NaN 标记空间但不符合任何已知标签 → Unknown
        Self {
            tag: "Unknown",
            payload: "unclassified".to_string(),
            raw_bits,
            raw_bits_u64: bits,
            gc_managed: false,
            is_valid: false,
        }
    }

    // ========================================================================
    // ASCII 位域可视化
    // ========================================================================

    /// 生成 ASCII 位域可视化图。
    ///
    /// 输出格式根据类型标签自适应调整位域划分：
    ///
    /// ```text
    /// Smi(42) [0x7FF9_0000_0000_002A]
    /// ┌──────────┬──────────────────────────────────────────────┐
    /// │ Tag[63:49]│ Payload[48:0]                                │
    /// │ 0x7FF9    │ 0x0000_0000_002A (=42)                       │
    /// └──────────┴──────────────────────────────────────────────┘
    /// ```
    ///
    /// 对于 HeapObject 类型，额外展示 GC-bit：
    ///
    /// ```text
    /// HeapObject(index=3, gc=true) [0x7FF8_4000_0020_0003]
    /// ┌──────────┬────┬─────────────────────────────────────────┐
    /// │ Tag[63:49]│ GC │ Index[44:0]                              │
    /// │ 0x7FF8_4  │ 1  │ 0x0000_0000_0003 (=3)                    │
    /// └──────────┴────┴─────────────────────────────────────────┘
    /// ```
    pub fn format_visual(&self) -> String {
        let bits = self.parse_raw_bits();

        let header = format!("{}({}) [{}]", self.tag, self.payload, self.raw_bits);

        match self.tag {
            "Nil" | "Bool" => {
                let tag_val = (bits >> 49) & 0x7FFF;
                let payload_val = bits & NIL_BOOL_PAYLOAD_MASK;
                format_table(
                    &header,
                    &[
                        ("Tag[63:49]", format!("0x{:04X}", tag_val)),
                        ("Payload[48:0]", format!("0x{:012X} (={})", payload_val, self.payload)),
                    ],
                )
            }
            "Smi" => {
                let tag_val = (bits >> 48) & 0xFFFF;
                let payload_val = bits & SMI_VALUE_MASK;
                let smi_num = self.payload.parse::<i64>().unwrap_or(0);
                format_table(
                    &header,
                    &[
                        ("Tag[63:48]", format!("0x{:04X}", tag_val)),
                        ("Payload[47:0]", format!("0x{:012X} (={})", payload_val, smi_num)),
                    ],
                )
            }
            "Float" => {
                let sign = (bits >> 63) & 1;
                let exponent = (bits >> 52) & IEEE754_EXP_MASK;
                let mantissa = bits & MANTISSA_MASK;
                format_table(
                    &header,
                    &[
                        ("S[63]", format!("{}", sign)),
                        ("Exp[62:52]", format!("0x{:03X} (={})", exponent, exponent)),
                        ("Mantissa[51:0]", format!("0x{:013X}", mantissa)),
                    ],
                )
            }
            "String" => {
                let tag_val = (bits >> 47) & 0x1FFFF;
                let index_val = bits & STRING_INDEX_MASK;
                let idx_num = self.payload.trim_start_matches("index=").parse::<u64>().unwrap_or(0);
                format_table(
                    &header,
                    &[
                        ("Tag[63:47]", format!("0x{:05X}", tag_val)),
                        ("Index[46:0]", format!("0x{:012X} (={})", index_val, idx_num)),
                    ],
                )
            }
            "HeapObject" => {
                let tag_val = (bits >> 46) & 0x3FFFF;
                let gc_bit = (bits >> 45) & 1;
                let index_val = bits & HEAP_INDEX_MASK_NO_GC;
                let idx_num = self
                    .payload
                    .split(',')
                    .next()
                    .unwrap_or("index=0")
                    .trim_start_matches("index=")
                    .parse::<u64>()
                    .unwrap_or(0);
                format_table(
                    &header,
                    &[
                        ("Tag[63:46]", format!("0x{:05X}", tag_val)),
                        ("GC[45]", format!("{}", gc_bit)),
                        ("Index[44:0]", format!("0x{:012X} (={})", index_val, idx_num)),
                    ],
                )
            }
            "Pointer" => {
                let tag_val = (bits >> 48) & 0xFFFF;
                let addr_val = bits & PTR_MASK;
                format_table(
                    &header,
                    &[
                        ("Tag[63:48]", format!("0x{:04X}", tag_val)),
                        ("Addr[45:0]", format!("0x{:012X}", addr_val)),
                    ],
                )
            }
            _ => {
                // Unknown 类型：展示完整的 64 位十六进制
                let high = (bits >> 32) & 0xFFFF_FFFF;
                let low = bits & 0xFFFF_FFFF;
                format_table(
                    &header,
                    &[
                        ("High[63:32]", format!("0x{:08X}", high)),
                        ("Low[31:0]", format!("0x{:08X}", low)),
                    ],
                )
            }
        }
    }

    // ========================================================================
    // 合法性校验
    // ========================================================================

    /// 检测非法位模式，返回校验结果。
    ///
    /// # 校验规则
    ///
    /// 1. **NaN 空间未分类**：位模式落入 NaN 标记空间（`SPECIAL_MASK` 匹配）
    ///    但不符合任何已知标签 → 非法
    /// 2. **堆索引越界**：HeapObject 的索引超过合理上限（16M）→ 疑似损坏
    /// 3. **字符串索引越界**：String 的索引超过合理上限（16M）→ 疑似损坏
    /// 4. **特殊值低位非法**：Nil/Bool 的低位载荷应为 1/2/3，其他值 → 非法
    pub fn validate(&self) -> Result<(), String> {
        let bits = self.parse_raw_bits();

        // 规则 1：NaN 空间未分类
        if self.tag == "Unknown" {
            // 判断是否确实在 NaN 标记空间内
            let in_nan_space =
                (bits & SPECIAL_MASK) == SPECIAL_MASK || (bits & SMI_MASK) == SMI_TAG;
            if in_nan_space {
                return Err(format!(
                    "位模式 0x{:016X} 落入 NaN 标记空间但不符合任何已知标签",
                    bits
                ));
            }
            // 不在 NaN 空间的 Unknown：可能是合法的浮点数边缘情况
            return Err(format!("位模式 0x{:016X} 未被分类为任何已知类型", bits));
        }

        // 规则 2：堆索引越界
        if self.tag == "HeapObject" {
            let idx = bits & HEAP_INDEX_MASK_NO_GC;
            if idx >= HEAP_INDEX_SANITY_LIMIT {
                return Err(format!(
                    "堆索引 {} 超出合理上限 {}，疑似位模式损坏",
                    idx, HEAP_INDEX_SANITY_LIMIT
                ));
            }
        }

        // 规则 3：字符串索引越界
        if self.tag == "String" {
            let idx = bits & STRING_INDEX_MASK;
            if idx >= STRING_INDEX_SANITY_LIMIT {
                return Err(format!(
                    "字符串索引 {} 超出合理上限 {}，疑似位模式损坏",
                    idx, STRING_INDEX_SANITY_LIMIT
                ));
            }
        }

        // 规则 4：特殊值低位载荷校验
        if self.tag == "Nil" {
            let low = bits & LOW_NIBBLE_MASK;
            if low != 1 {
                return Err(format!("Nil 值低位载荷应为 1，实际为 {}，疑似位模式损坏", low));
            }
        }
        if self.tag == "Bool" {
            let low = bits & LOW_NIBBLE_MASK;
            if low != 2 && low != 3 {
                return Err(format!(
                    "Bool 值低位载荷应为 2(false) 或 3(true)，实际为 {}，疑似位模式损坏",
                    low
                ));
            }
        }

        Ok(())
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 u64 格式化为带下划线分隔的十六进制字符串。
///
/// 格式：`0x7FF9_0000_0000_002A`
fn format_raw_bits(bits: u64) -> String {
    format!(
        "0x{:04X}_{:04X}_{:04X}_{:04X}",
        (bits >> 48) & 0xFFFF,
        (bits >> 32) & 0xFFFF,
        (bits >> 16) & 0xFFFF,
        bits & 0xFFFF,
    )
}

/// 格式化浮点数为可读字符串。
///
/// - 整数浮点：显示为 `3.0`（保留一位小数以区分 Smi）
/// - 非整数：显示为 `3.14`
/// - 特殊值：`inf`, `-inf`, `NaN`
fn format_float(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n.is_sign_negative() { "-inf".to_string() } else { "inf".to_string() }
    } else if n == 0.0 {
        if n.is_sign_negative() { "-0.0".to_string() } else { "0.0".to_string() }
    } else if n.fract() == 0.0 && n.abs() < FLOAT_INTEGER_DISPLAY_THRESHOLD {
        format!("{:.1}", n)
    } else {
        format!("{}", n)
    }
}

/// 生成 ASCII 表格。
///
/// 将位域名称和值格式化为带边框的表格，自适应列宽。
fn format_table(header: &str, rows: &[(&str, String)]) -> String {
    let name_width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let value_width = rows.iter().map(|(_, val)| val.len()).max().unwrap_or(0);

    let mut out = String::with_capacity(LAYOUT_OUTPUT_CAPACITY);
    out.push_str(header);
    out.push('\n');

    out.push_str(&format!("┌{}┬{}┐\n", "─".repeat(name_width + 2), "─".repeat(value_width + 2),));

    for (name, value) in rows.iter() {
        out.push_str(&format!(
            "│ {:<name_w$} │ {:<value_w$} │\n",
            name,
            value,
            name_w = name_width,
            value_w = value_width,
        ));
    }

    out.push_str(&format!("└{}┴{}┘", "─".repeat(name_width + 2), "─".repeat(value_width + 2),));

    out
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueExt;

    // ------------------------------------------------------------------
    // Happy Path: 所有类型解析正确
    // ------------------------------------------------------------------

    #[test]
    fn test_nil_layout() {
        // SAFETY: NIL_VALUE is a well-known constant with valid NaN-tag encoding.
        let layout = ValueLayout::from_value(unsafe { Value::from_raw_bits(NIL_VALUE) });
        assert_eq!(layout.tag, "Nil");
        assert_eq!(layout.payload, "singleton");
        assert!(!layout.gc_managed);
        assert!(layout.is_valid);
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn test_bool_true_layout() {
        // SAFETY: TRUE_VALUE is a well-known constant with valid NaN-tag encoding.
        let layout = ValueLayout::from_value(unsafe { Value::from_raw_bits(TRUE_VALUE) });
        assert_eq!(layout.tag, "Bool");
        assert_eq!(layout.payload, "true");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_bool_false_layout() {
        // SAFETY: FALSE_VALUE is a well-known constant with valid NaN-tag encoding.
        let layout = ValueLayout::from_value(unsafe { Value::from_raw_bits(FALSE_VALUE) });
        assert_eq!(layout.tag, "Bool");
        assert_eq!(layout.payload, "false");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_smi_layout() {
        let val = Value::from_smi(42);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Smi");
        assert_eq!(layout.payload, "42");
        assert!(!layout.gc_managed);
        assert!(layout.is_valid);
    }

    #[test]
    fn test_smi_negative_layout() {
        let val = Value::from_smi(-100);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Smi");
        assert_eq!(layout.payload, "-100");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_float_layout() {
        let val = Value::from_number(2.5);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert!(layout.payload.contains("2.5"));
        assert!(layout.is_valid);
    }

    #[test]
    fn test_float_nan_layout() {
        let val = Value::from_number(f64::NAN);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert_eq!(layout.payload, "NaN");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_float_infinity_layout() {
        let val = Value::from_number(f64::INFINITY);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert_eq!(layout.payload, "inf");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_float_negative_infinity_layout() {
        let val = Value::from_number(f64::NEG_INFINITY);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert_eq!(layout.payload, "-inf");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_string_layout() {
        let val = Value::from_string("hello");
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "String");
        assert!(layout.payload.starts_with("index="));
        assert!(!layout.gc_managed);
        assert!(layout.is_valid);
    }

    #[test]
    fn test_heap_object_layout() {
        // SAFETY: HEAP_TAG | 42 produces a valid NaN-tagged heap object encoding.
        let val = unsafe { Value::from_raw_bits(HEAP_TAG | 42) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "HeapObject");
        assert!(layout.payload.contains("index=42"));
        assert!(layout.payload.contains("gc=false"));
        assert!(!layout.gc_managed);
        assert!(layout.is_valid);
    }

    #[test]
    fn test_heap_object_gc_layout() {
        // SAFETY: HEAP_TAG | GC_MANAGED_BIT | 7 produces a valid GC-managed heap object encoding.
        let val = unsafe { Value::from_raw_bits(HEAP_TAG | GC_MANAGED_BIT | 7) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "HeapObject");
        assert!(layout.payload.contains("index=7"));
        assert!(layout.payload.contains("gc=true"));
        assert!(layout.gc_managed);
        assert!(layout.is_valid);
    }

    #[test]
    fn test_pointer_layout() {
        // SAFETY: PTR_TAG | 0xDEAD produces a valid pointer-tagged encoding.
        let val = unsafe { Value::from_raw_bits(PTR_TAG | 0xDEAD) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Pointer");
        assert!(layout.payload.starts_with("addr="));
        assert!(!layout.gc_managed);
        assert!(layout.is_valid);
    }

    // ------------------------------------------------------------------
    // Edge Case: 边界值
    // ------------------------------------------------------------------

    #[test]
    fn test_smi_max_layout() {
        let val = Value::from_smi(SMI_MAX);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Smi");
        assert_eq!(layout.payload, format!("{}", SMI_MAX));
        assert!(layout.is_valid);
    }

    #[test]
    fn test_smi_min_layout() {
        let val = Value::from_smi(SMI_MIN);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Smi");
        assert_eq!(layout.payload, format!("{}", SMI_MIN));
        assert!(layout.is_valid);
    }

    #[test]
    fn test_smi_zero_layout() {
        let val = Value::from_smi(0);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Smi");
        assert_eq!(layout.payload, "0");
    }

    #[test]
    fn test_float_zero_layout() {
        let val = Value::from_number(0.0);
        let layout = ValueLayout::from_value(val);
        // 0.0 可能被编码为 Smi(0)
        assert!(layout.tag == "Smi" || layout.tag == "Float");
    }

    #[test]
    fn test_float_negative_zero_layout() {
        let val = Value::from_number(-0.0);
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert_eq!(layout.payload, "-0.0");
    }

    #[test]
    fn test_canonical_nan_layout() {
        // SAFETY: CANONICAL_NAN is a well-known constant with valid NaN encoding.
        let val = unsafe { Value::from_raw_bits(CANONICAL_NAN) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert_eq!(layout.payload, "NaN");
    }

    // ------------------------------------------------------------------
    // Poison Pill: 非法位模式
    // ------------------------------------------------------------------

    #[test]
    fn test_unknown_nan_space_layout() {
        // 构造一个在 NaN 空间但不符合任何已知标签的位模式
        //
        // 0x7FFC_0000_0000_0000 的逐项验证：
        // - (bits & SMI_MASK)    = 0x7FFC != SMI_TAG(0x7FF9)       → not smi
        // - (bits & HEAP_MASK)   = 0x7FF8_C000 != HEAP_TAG          → not heap
        // - (bits & STRING_MASK) = 0x7FF8_0000 != STRING_TAG         → not string
        // - (bits & QNAN_MASK)   = 0x7FFC_0000 != PTR_TAG            → not ptr
        // - (bits & SPECIAL_MASK)= 0x7FF8_0000 == SPECIAL_MASK       → is_special
        // - 不是 nil, 不是 bool → Unknown
        let bits = 0x7FFC_0000_0000_0000;
        // SAFETY: This test deliberately constructs an invalid bit pattern to verify
        // that ValueLayout correctly detects it as Unknown. The invalid Value is never
        // used beyond the layout diagnostic, so no UB is triggered.
        let val = unsafe { Value::from_raw_bits(bits) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Unknown");
        assert!(!layout.is_valid);
        assert!(layout.validate().is_err());
    }

    #[test]
    fn test_heap_index_overflow_layout() {
        // 堆索引超出合理上限
        let bits = HEAP_TAG | (HEAP_INDEX_SANITY_LIMIT + 1);
        // SAFETY: This test constructs a deliberately invalid heap index for edge-case testing.
        let val = unsafe { Value::from_raw_bits(bits) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "HeapObject");
        assert!(!layout.is_valid);
        let err = layout.validate().unwrap_err();
        assert!(err.contains("超出合理上限"));
    }

    #[test]
    fn test_string_index_overflow_layout() {
        // 字符串索引超出合理上限
        let bits = STRING_TAG | (STRING_INDEX_SANITY_LIMIT + 1);
        // SAFETY: This test constructs a deliberately invalid string index for edge-case testing.
        let val = unsafe { Value::from_raw_bits(bits) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "String");
        assert!(!layout.is_valid);
        let err = layout.validate().unwrap_err();
        assert!(err.contains("超出合理上限"));
    }

    #[test]
    fn test_nil_corrupted_low_bits_layout() {
        // Nil 值低位载荷不是 1
        let bits = PTR_TAG; // 低位 = 0，不是合法的 nil
        // SAFETY: PTR_TAG | 0 produces a valid pointer-tagged encoding (null pointer).
        let val = unsafe { Value::from_raw_bits(bits) };
        // 这种值会被 is_ptr() 识别（PTR_TAG | 0 = 空指针）
        let layout = ValueLayout::from_value(val);
        // 空指针地址为 0，is_ptr() 返回 true
        assert_eq!(layout.tag, "Pointer");
    }

    #[test]
    fn test_all_zeros_layout() {
        // 全零位模式 = IEEE 754 正零
        // SAFETY: All-zeros is a valid IEEE 754 representation of +0.0.
        let val = unsafe { Value::from_raw_bits(0) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "Float");
        assert!(layout.is_valid);
    }

    #[test]
    fn test_all_ones_layout() {
        // 全1位模式 = 0xFFFF_FFFF_FFFF_FFFF
        // (bits & STRING_MASK) == STRING_TAG 为 true，因此被识别为 String
        // 这是因为 STRING_MASK == STRING_TAG，全1与 STRING_MASK 仍等于 STRING_TAG
        // SAFETY: This test verifies edge-case behaviour of the tag classifier.
        let val = unsafe { Value::from_raw_bits(0xFFFF_FFFF_FFFF_FFFF) };
        let layout = ValueLayout::from_value(val);
        assert_eq!(layout.tag, "String");
    }

    // ------------------------------------------------------------------
    // format_visual 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_smi_visual_output() {
        let val = Value::from_smi(42);
        let layout = ValueLayout::from_value(val);
        let visual = layout.format_visual();
        assert!(visual.contains("Smi(42)"));
        assert!(visual.contains("Tag[63:48]"));
        assert!(visual.contains("Payload[47:0]"));
        assert!(visual.contains("0x7FF9"));
    }

    #[test]
    fn test_heap_object_visual_output() {
        // SAFETY: HEAP_TAG | GC_MANAGED_BIT | 3 produces a valid GC-managed heap object encoding.
        let val = unsafe { Value::from_raw_bits(HEAP_TAG | GC_MANAGED_BIT | 3) };
        let layout = ValueLayout::from_value(val);
        let visual = layout.format_visual();
        assert!(visual.contains("HeapObject"));
        assert!(visual.contains("GC[45]"));
        assert!(visual.contains("Index[44:0]"));
    }

    #[test]
    fn test_float_visual_output() {
        let val = Value::from_number(2.5);
        let layout = ValueLayout::from_value(val);
        let visual = layout.format_visual();
        assert!(visual.contains("Float"));
        assert!(visual.contains("S[63]"));
        assert!(visual.contains("Exp[62:52]"));
        assert!(visual.contains("Mantissa[51:0]"));
    }

    #[test]
    fn test_string_visual_output() {
        let val = Value::from_string("test");
        let layout = ValueLayout::from_value(val);
        let visual = layout.format_visual();
        assert!(visual.contains("String"));
        assert!(visual.contains("Tag[63:47]"));
        assert!(visual.contains("Index[46:0]"));
    }

    // ------------------------------------------------------------------
    // format_raw_bits 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_format_raw_bits_smi_tag() {
        let bits = SMI_TAG;
        assert_eq!(format_raw_bits(bits), "0x7FF9_0000_0000_0000");
    }

    #[test]
    fn test_format_raw_bits_nil() {
        let bits = NIL_VALUE;
        assert_eq!(format_raw_bits(bits), "0x7FF8_0000_0000_0001");
    }

    // ------------------------------------------------------------------
    // validate 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_validate_valid_smi() {
        let layout = ValueLayout::from_value(Value::from_smi(100));
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_nil() {
        // SAFETY: NIL_VALUE is a well-known constant with valid NaN-tag encoding.
        let layout = ValueLayout::from_value(unsafe { Value::from_raw_bits(NIL_VALUE) });
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_bool() {
        // SAFETY: TRUE_VALUE/FALSE_VALUE are well-known constants with valid NaN-tag encoding.
        let layout_true = ValueLayout::from_value(unsafe { Value::from_raw_bits(TRUE_VALUE) });
        let layout_false = ValueLayout::from_value(unsafe { Value::from_raw_bits(FALSE_VALUE) });
        assert!(layout_true.validate().is_ok());
        assert!(layout_false.validate().is_ok());
    }

    #[test]
    fn test_validate_unknown_fails() {
        // 使用与 test_unknown_nan_space_layout 相同的位模式
        let bits = 0x7FFC_0000_0000_0000;
        // SAFETY: Deliberately invalid bit pattern for validation testing.
        let layout = ValueLayout::from_value(unsafe { Value::from_raw_bits(bits) });
        assert!(layout.validate().is_err());
    }

    // ------------------------------------------------------------------
    // Clone / Debug trait 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_layout_clone() {
        let layout = ValueLayout::from_value(Value::from_smi(42));
        let cloned = layout.clone();
        assert_eq!(layout.tag, cloned.tag);
        assert_eq!(layout.payload, cloned.payload);
        assert_eq!(layout.raw_bits, cloned.raw_bits);
        assert_eq!(layout.gc_managed, cloned.gc_managed);
        assert_eq!(layout.is_valid, cloned.is_valid);
    }

    #[test]
    fn test_layout_debug() {
        let layout = ValueLayout::from_value(Value::from_smi(42));
        let debug_str = format!("{:?}", layout);
        assert!(debug_str.contains("Smi"));
        assert!(debug_str.contains("42"));
    }

    // ------------------------------------------------------------------
    // ValueTag 一致性测试
    // ------------------------------------------------------------------

    #[test]
    fn test_layout_tag_matches_value_tag() {
        // 确保 ValueLayout 的 tag 字符串与 Value::tag() 枚举一致
        // SAFETY: All bit patterns below are well-known constants with valid NaN-tag encoding.
        let cases: Vec<(Value, &'static str)> = vec![
            (unsafe { Value::from_raw_bits(NIL_VALUE) }, "Nil"),
            (unsafe { Value::from_raw_bits(TRUE_VALUE) }, "Bool"),
            (Value::from_smi(0), "Smi"),
            (Value::from_number(2.5), "Float"),
            (Value::from_string("x"), "String"),
            (unsafe { Value::from_raw_bits(HEAP_TAG | 1) }, "HeapObject"),
        ];
        for (val, expected_tag) in cases {
            let layout = ValueLayout::from_value(val);
            assert_eq!(
                layout.tag, expected_tag,
                "ValueLayout tag mismatch for {:?}: expected {}, got {}",
                val, expected_tag, layout.tag
            );
        }
    }
}

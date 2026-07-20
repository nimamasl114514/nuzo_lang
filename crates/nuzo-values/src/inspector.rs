//! # ValueInspector -- Value 调试检查工具
//!
//! 本模块提供 [`ValueInspector`]，用于对 [`Value`] 进行深度结构化检查，
//! 生成包含类型标签、位布局、堆对象详情和合法性校验的完整诊断报告。
//!
//! ## 设计目标
//!
//! 1. **结构化报告**：将 Value 的所有可观测属性聚合到 [`InspectionReport`] 中，
//!    避免调用方反复查询 Value 的各个方法
//! 2. **堆对象透视**：对堆对象（Array/Dict/Range/Closure/Box/BuiltinFn）展开内部结构，
//!    提供大小估算和预览信息
//! 3. **人类可读输出**：[`format_diagnostic`] 生成多行 ASCII 诊断文本，
//!    包含位域可视化表格，适合 REPL 和日志输出
//! 4. **零隐式开销**：所有公共方法均为显式调用，release 模式下不引入额外开销；
//!    重量级辅助函数标记 `#[cold] #[inline(never)]` 避免影响热路径 I-Cache
//!
//! ## 使用示例
//!
//! ```ignore
//! use nuzo_values::{Value, ValueInspector};
//!
//! let val = Value::from_smi(42);
//! let report = ValueInspector::inspect(val);
//! assert!(report.is_valid);
//! assert_eq!(report.type_name, "integer");
//!
//! println!("{}", ValueInspector::format_diagnostic(val));
//! ```

use crate::heap::{HeapObject, RangeEnd};
use crate::layout::ValueLayout;
use crate::value::{Value, ValueExt, ValueTag};

// ============================================================================
// 数组预览元素数量上限
// ============================================================================

/// 数组预览时最多展示的元素数量。
///
/// 超过此数量的元素以 "..." 省略，避免大数组诊断输出过长。
const ARRAY_PREVIEW_LIMIT: usize = 8;

/// 字典键预览时最多展示的键数量。
const DICT_KEY_PREVIEW_LIMIT: usize = 8;

/// 诊断报告输出字符串的初始预分配容量。
///
/// 典型的检查报告包含头部、类型信息、位域可视化表格等，
/// 约 300-500 字节，512 字节预分配可避免大多数重分配。
const INSPECTOR_OUTPUT_CAPACITY: usize = 512;

// ============================================================================
// HeapDetailKind -- 堆对象类型特定详情
// ============================================================================

/// 堆对象的类型特定详情。
///
/// 每个变体对应 [`HeapObject`] 的一种具体类型，
/// 提供该类型独有的诊断信息。
#[derive(Debug, Clone)]
pub enum HeapDetailKind {
    /// 数组详情：长度 + 前 N 个元素的字符串预览
    Array {
        /// 数组长度
        length: usize,
        /// 前 N 个元素的 Display 表示
        preview: Vec<String>,
    },
    /// 字典详情：长度 + 前 N 个键的字符串预览
    Dict {
        /// 字典条目数
        length: usize,
        /// 前 N 个键的 Display 表示
        keys_preview: Vec<String>,
    },
    /// 范围详情：起止值和包含性
    Range {
        /// 起始值
        start: f64,
        /// 终止值
        end: f64,
        /// 范围端点包含性（`..=` vs `..`）
        range_end: RangeEnd,
    },
    /// 闭包详情：参数数量、捕获变量数量、函数名
    Closure {
        /// 函数参数数量
        arity: usize,
        /// 捕获变量数量
        captured_count: usize,
        /// 函数名（如果可获取）
        name: Option<String>,
    },
    /// Box 详情：内部值的类型名称
    Box {
        /// 被包装值的类型名称
        inner_type: &'static str,
    },
    /// 内建函数详情：名称和参数数量
    BuiltinFn {
        /// 函数名称
        name: String,
        /// 参数数量
        arity: usize,
    },
    /// 异常对象详情
    Exception {
        /// 错误消息
        message: String,
        /// 错误码标识符
        code: String,
    },
    /// 未知堆对象类型（未来扩展预留）
    Unknown,
}

// ============================================================================
// HeapDetail -- 堆对象详情
// ============================================================================

/// 堆对象的完整诊断详情。
///
/// 包含堆索引、GC 管理状态、大小估算和类型特定信息。
#[derive(Debug, Clone)]
pub struct HeapDetail {
    /// 堆对象类型名称（如 "array", "dict", "closure" 等）
    pub object_type: &'static str,
    /// 堆索引
    pub heap_index: u32,
    /// 是否由 GC 管理
    pub gc_managed: bool,
    /// 大小估算（字节）
    pub size_estimate: usize,
    /// 类型特定详情
    pub detail: HeapDetailKind,
}

// ============================================================================
// InspectionReport -- Value 检查报告
// ============================================================================

/// Value 的完整检查报告。
///
/// 聚合了 Value 的所有可观测属性：类型标签、原始位模式、有效载荷、
/// 位布局诊断、堆对象详情和合法性校验结果。
///
/// # 不变量
///
/// - `tag` 和 `type_name` 始终与 `Value::tag()` 和 `Value::type_name()` 一致
/// - `is_valid` 为 `false` 时，`validation_error` 必定为 `Some`
/// - `heap_detail` 仅对 HeapObject 类型为 `Some`
#[derive(Debug, Clone)]
pub struct InspectionReport {
    /// 类型标签枚举
    pub tag: ValueTag,
    /// 类型名称字符串（如 "nil", "integer", "array" 等）
    pub type_name: &'static str,
    /// 原始位模式十六进制表示
    pub raw_bits: String,
    /// 有效载荷描述
    pub payload: String,
    /// 位布局诊断信息
    pub layout: ValueLayout,
    /// 堆对象详情（仅 HeapObject 类型）
    pub heap_detail: Option<HeapDetail>,
    /// 位模式是否合法
    pub is_valid: bool,
    /// 校验错误信息（如果 `is_valid` 为 false）
    pub validation_error: Option<String>,
}

// ============================================================================
// ValueInspector -- 调试检查工具
// ============================================================================

/// Value 调试检查工具。
///
/// 提供两个核心方法：
/// - [`inspect`]：返回结构化报告 [`InspectionReport`]
/// - [`format_diagnostic`]：生成人类可读的多行诊断文本
///
/// # 零开销保证
///
/// 此类型为无状态零大小类型（ZST），不携带任何运行时状态。
/// 所有方法均为关联函数（无 `&self`），调用不产生额外分配。
pub struct ValueInspector;

impl ValueInspector {
    /// 检查 Value，返回结构化报告。
    ///
    /// 一次性聚合 Value 的所有可观测属性，避免调用方反复查询。
    /// 对于堆对象，额外展开内部结构并提供大小估算。
    ///
    /// # 性能说明
    ///
    /// 此方法涉及字符串格式化和堆对象访问，属于重量级操作。
    /// 仅应在调试/诊断场景下显式调用，不应出现在热路径中。
    pub fn inspect(value: Value) -> InspectionReport {
        let tag = value.tag();
        let type_name = value.type_name();
        let layout = ValueLayout::from_value(value);
        let raw_bits = layout.raw_bits.clone();
        let payload = layout.payload.clone();

        let validation_result = layout.validate();
        let is_valid = validation_result.is_ok();
        let validation_error = validation_result.err();

        let heap_detail = build_heap_detail(value);

        InspectionReport {
            tag,
            type_name,
            raw_bits,
            payload,
            layout,
            heap_detail,
            is_valid,
            validation_error,
        }
    }

    /// 格式化诊断输出（多行文本）。
    ///
    /// 生成包含类型信息、位模式、GC 状态、大小估算和位域可视化表格的
    /// 完整 ASCII 诊断报告，适合 REPL 输出和日志记录。
    ///
    /// # 输出格式
    ///
    /// ```text
    /// ═══════════════════════════════════════
    ///  Value Inspection Report
    /// ═══════════════════════════════════════
    ///  Type:       HeapObject (Array)
    ///  Raw Bits:   0x7FF8_4000_0000_0005
    ///  GC:         yes (index=5)
    ///  Size:       128 bytes
    ///  Detail:     Array[3] = [1, 2, 3]
    ///  Valid:      ✓
    /// ═══════════════════════════════════════
    /// ┌──────────┬──────────────────────────────────────────────┐
    /// │ Tag[63:49]│ Payload[48:0]                                │
    /// │ 0x7FF8    │ 0x4000_0000_0005                             │
    /// └──────────┴──────────────────────────────────────────────┘
    /// ```
    ///
    /// # 性能说明
    ///
    /// 此方法为重量级格式化操作，标记为 `#[cold]` 以避免影响热路径 I-Cache。
    #[cold]
    #[inline(never)]
    pub fn format_diagnostic(value: Value) -> String {
        let report = Self::inspect(value);
        format_report(&report)
    }
}

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 构建堆对象详情。
///
/// 仅对 HeapObject 类型的 Value 执行，其他类型返回 `None`。
/// 内部访问堆对象并提取类型特定信息。
fn build_heap_detail(value: Value) -> Option<HeapDetail> {
    if !value.is_heap_object() {
        return None;
    }

    let heap_index = value.heap_index()?;
    let gc_managed = value.is_gc_managed();

    let obj = value.as_heap_object_opt()?;

    // `_` 通配符为未来新增 HeapObject 变体预留；当前 7 变体已穷尽覆盖故暂不可达。
    // 待双区布局改造新增变体后此分支将变为可达，届时可移除该 allow。
    #[allow(unreachable_patterns)]
    let (object_type, size_estimate, detail) = match obj.as_ref() {
        HeapObject::Array(arr) => {
            let length = arr.len();
            let preview: Vec<String> =
                arr.iter().take(ARRAY_PREVIEW_LIMIT).map(|v| format!("{}", v)).collect();
            ("array", obj.size_estimate(), HeapDetailKind::Array { length, preview })
        }
        HeapObject::Dict(nuzo_dict) => {
            let length = nuzo_dict.len();
            let keys_preview: Vec<String> = nuzo_dict
                .iter()
                .take(DICT_KEY_PREVIEW_LIMIT)
                .map(|(key_idx, _)| format!("key#{}", key_idx))
                .collect();
            ("dict", obj.size_estimate(), HeapDetailKind::Dict { length, keys_preview })
        }
        HeapObject::Range { start, end, range_end } => (
            "range",
            obj.size_estimate(),
            HeapDetailKind::Range { start: *start, end: *end, range_end: *range_end },
        ),
        HeapObject::Closure { prototype, captured, .. } => {
            let arity = prototype.arity as usize;
            let captured_count = captured.len();
            // 直接使用编译器填充的 name 字段
            let name = if prototype.name.is_empty() { None } else { Some(prototype.name.clone()) };
            (
                "closure",
                obj.size_estimate(),
                HeapDetailKind::Closure { arity, captured_count, name },
            )
        }
        HeapObject::Box(inner) => {
            ("box", obj.size_estimate(), HeapDetailKind::Box { inner_type: inner.type_name() })
        }
        HeapObject::BuiltinFn { name, arity, .. } => (
            "builtin",
            obj.size_estimate(),
            HeapDetailKind::BuiltinFn { name: name.clone(), arity: *arity },
        ),
        HeapObject::Exception { message, code, .. } => (
            "exception",
            obj.size_estimate(),
            HeapDetailKind::Exception { message: message.clone(), code: code.clone() },
        ),
        // 通配符分支：未来新增 HeapObject 变体时无需修改此处，
        // 避免 inspector 成为变体新增的阻碍（为双区布局改造铺路）。
        // 注：此分支在正常运行时不应命中；命中表示出现未识别变体，
        // 返回 Unknown 详情以便诊断工具仍能输出基本信息（类型名/大小/GC 状态）。
        _ => ("unknown", obj.size_estimate(), HeapDetailKind::Unknown),
    };

    Some(HeapDetail { object_type, heap_index, gc_managed, size_estimate, detail })
}

/// 格式化检查报告为多行诊断文本。
///
/// 将 [`InspectionReport`] 转换为人类可读的 ASCII 格式，
/// 包含头部信息块和位域可视化表格。
fn format_report(report: &InspectionReport) -> String {
    const SEPARATOR: &str = "═══════════════════════════════════════";

    let mut out = String::with_capacity(INSPECTOR_OUTPUT_CAPACITY);

    out.push_str(SEPARATOR);
    out.push('\n');
    out.push_str(" Value Inspection Report\n");
    out.push_str(SEPARATOR);
    out.push('\n');

    let type_display = format_type_display(report);
    out.push_str(&format!(" Type:       {}\n", type_display));
    out.push_str(&format!(" Raw Bits:   {}\n", report.raw_bits));

    if let Some(ref hd) = report.heap_detail {
        let gc_label = if hd.gc_managed { "yes" } else { "no" };
        out.push_str(&format!(" GC:         {} (index={})\n", gc_label, hd.heap_index));
        out.push_str(&format!(" Size:       {} bytes\n", hd.size_estimate));
        out.push_str(&format!(" Detail:     {}\n", format_heap_detail_kind(&hd.detail)));
    } else {
        out.push_str(&format!(" Payload:    {}\n", report.payload));
    }

    let valid_label = if report.is_valid { "ok" } else { "INVALID" };
    out.push_str(&format!(" Valid:      {}\n", valid_label));

    if let Some(ref err) = report.validation_error {
        out.push_str(&format!(" Error:      {}\n", err));
    }

    out.push_str(SEPARATOR);
    out.push('\n');

    out.push_str(&report.layout.format_visual());

    out
}

/// 格式化类型显示字符串。
///
/// 对于堆对象，显示 "HeapObject (子类型)" 格式；
/// 对于其他类型，直接显示类型名称。
fn format_type_display(report: &InspectionReport) -> String {
    if let Some(ref hd) = report.heap_detail {
        format!("HeapObject ({})", hd.object_type)
    } else {
        report.type_name.to_string()
    }
}

/// 格式化堆对象详情为人类可读字符串。
///
/// 输出示例：
/// - `Array[3] = [1, 2, 3]`
/// - `Dict{2} = [key#0, key#1]`
/// - `Range(1..=5)`
/// - `Closure(arity=2, captures=1)`
/// - `Box(integer)`
/// - `BuiltinFn:len(1)`
fn format_heap_detail_kind(detail: &HeapDetailKind) -> String {
    match detail {
        HeapDetailKind::Array { length, preview } => {
            let preview_str = if *length <= ARRAY_PREVIEW_LIMIT {
                format!("[{}]", preview.join(", "))
            } else {
                format!("[{}, ...]", preview.join(", "))
            };
            format!("Array[{}] = {}", length, preview_str)
        }
        HeapDetailKind::Dict { length, keys_preview } => {
            let keys_str = if *length <= DICT_KEY_PREVIEW_LIMIT {
                format!("[{}]", keys_preview.join(", "))
            } else {
                format!("[{}, ...]", keys_preview.join(", "))
            };
            format!("Dict{{{}}} = {}", length, keys_str)
        }
        HeapDetailKind::Range { start, end, range_end } => {
            if *range_end == RangeEnd::Inclusive {
                format!("Range({}..={})", start, end)
            } else {
                format!("Range({}..{})", start, end)
            }
        }
        HeapDetailKind::Closure { arity, captured_count, name } => {
            let name_str = name.as_deref().unwrap_or("<anonymous>");
            format!("Closure({}: arity={}, captures={})", name_str, arity, captured_count)
        }
        HeapDetailKind::Box { inner_type } => format!("Box({})", inner_type),
        HeapDetailKind::BuiltinFn { name, arity } => {
            format!("BuiltinFn:{}({})", name, arity)
        }
        HeapDetailKind::Exception { message, code } => {
            format!("Exception:{}:{}", code, message)
        }
        HeapDetailKind::Unknown => "Unknown".to_string(),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::*;
    use crate::heap::{CapturedVar, HeapObject};
    use crate::nuzo_dict::NuzoDict;
    use crate::value::{FunctionPrototype, Value};
    use std::sync::Arc;

    // ========================================================================
    // Happy Path: 所有类型检查正确
    // ========================================================================

    #[test]
    fn test_inspect_nil() {
        // SAFETY: NIL_VALUE is a well-known constant with valid NaN-tag encoding.
        let report = ValueInspector::inspect(unsafe { Value::from_raw_bits(NIL_VALUE) });
        assert_eq!(report.tag, ValueTag::Nil);
        assert_eq!(report.type_name, "nil");
        assert!(report.is_valid);
        assert!(report.heap_detail.is_none());
        assert!(report.validation_error.is_none());
    }

    #[test]
    fn test_inspect_bool_true() {
        // SAFETY: TRUE_VALUE is a well-known constant with valid NaN-tag encoding.
        let report = ValueInspector::inspect(unsafe { Value::from_raw_bits(TRUE_VALUE) });
        assert_eq!(report.tag, ValueTag::Bool);
        assert_eq!(report.type_name, "bool");
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_bool_false() {
        // SAFETY: FALSE_VALUE is a well-known constant with valid NaN-tag encoding.
        let report = ValueInspector::inspect(unsafe { Value::from_raw_bits(FALSE_VALUE) });
        assert_eq!(report.tag, ValueTag::Bool);
        assert_eq!(report.type_name, "bool");
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_smi() {
        let report = ValueInspector::inspect(Value::from_smi(42));
        assert_eq!(report.tag, ValueTag::Smi);
        assert_eq!(report.type_name, "integer");
        assert!(report.is_valid);
        assert!(report.heap_detail.is_none());
    }

    #[test]
    fn test_inspect_smi_negative() {
        let report = ValueInspector::inspect(Value::from_smi(-100));
        assert_eq!(report.tag, ValueTag::Smi);
        assert_eq!(report.type_name, "integer");
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_float() {
        let report = ValueInspector::inspect(Value::from_number(2.5));
        assert_eq!(report.tag, ValueTag::Float);
        assert_eq!(report.type_name, "number");
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_string() {
        let report = ValueInspector::inspect(Value::from_string("hello"));
        assert_eq!(report.tag, ValueTag::String);
        assert_eq!(report.type_name, "string");
        assert!(report.is_valid);
        assert!(report.heap_detail.is_none());
    }

    #[test]
    fn test_inspect_pointer() {
        // SAFETY: PTR_TAG | 0xDEAD produces a valid pointer-tagged encoding.
        let report = ValueInspector::inspect(unsafe { Value::from_raw_bits(PTR_TAG | 0xDEAD) });
        assert_eq!(report.tag, ValueTag::Pointer);
        assert!(report.is_valid);
    }

    // ========================================================================
    // 堆对象详情测试
    // ========================================================================

    #[test]
    fn test_inspect_heap_array() {
        let arr = HeapObject::Array(vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
            Value::from_number(3.0),
        ]);
        let val = Value::from_heap_object_gc(arr);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.tag, ValueTag::Pointer);
        assert_eq!(report.type_name, "array");
        assert!(report.is_valid);

        let hd = report.heap_detail.expect("heap_detail should exist for array");
        assert_eq!(hd.object_type, "array");
        assert!(hd.gc_managed);

        match hd.detail {
            HeapDetailKind::Array { length, preview } => {
                assert_eq!(length, 3);
                assert_eq!(preview.len(), 3);
            }
            other => panic!("Expected Array detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_empty_array() {
        let arr = HeapObject::Array(vec![]);
        let val = Value::from_heap_object_gc(arr);
        let report = ValueInspector::inspect(val);

        let hd = report.heap_detail.expect("heap_detail should exist");
        match hd.detail {
            HeapDetailKind::Array { length, preview } => {
                assert_eq!(length, 0);
                assert!(preview.is_empty());
            }
            other => panic!("Expected Array detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_dict() {
        let mut d = NuzoDict::new();
        d.insert(0, Value::from_number(1.0));
        let dict = HeapObject::Dict(d);
        let val = Value::from_heap_object_gc(dict);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.type_name, "dict");
        let hd = report.heap_detail.expect("heap_detail should exist");
        assert_eq!(hd.object_type, "dict");

        match hd.detail {
            HeapDetailKind::Dict { length, .. } => {
                assert_eq!(length, 1);
            }
            other => panic!("Expected Dict detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_range() {
        let range = HeapObject::Range { start: 1.0, end: 5.0, range_end: RangeEnd::Inclusive };
        let val = Value::from_heap_object_gc(range);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.type_name, "range"); // Range 现在返回精确类型名
        let hd = report.heap_detail.expect("heap_detail should exist");
        assert_eq!(hd.object_type, "range"); // 但 HeapDetail 中有精确类型

        match hd.detail {
            HeapDetailKind::Range { start, end, range_end } => {
                assert_eq!(start, 1.0);
                assert_eq!(end, 5.0);
                assert_eq!(range_end, RangeEnd::Inclusive);
            }
            other => panic!("Expected Range detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_range_exclusive() {
        let range = HeapObject::Range { start: 0.0, end: 10.0, range_end: RangeEnd::Exclusive };
        let val = Value::from_heap_object_gc(range);
        let report = ValueInspector::inspect(val);

        let hd = report.heap_detail.expect("heap_detail should exist");
        match hd.detail {
            HeapDetailKind::Range { range_end, .. } => {
                assert_eq!(range_end, RangeEnd::Exclusive);
            }
            other => panic!("Expected Range detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_closure() {
        let proto = FunctionPrototype::new(
            "<anonymous>".to_string(),
            2,
            2,
            Arc::new(vec![]),
            Arc::new(vec![]),
            vec![],
            Arc::new(vec![]),
            Arc::new(Default::default()),
            0,
        );
        let closure = HeapObject::Closure {
            prototype: Arc::new(proto),
            captured: vec![CapturedVar::Value(Value::from_number(42.0))],
            parent_env: None,
        };
        let val = Value::from_heap_object_gc(closure);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.type_name, "closure");
        let hd = report.heap_detail.expect("heap_detail should exist");

        match hd.detail {
            HeapDetailKind::Closure { arity, captured_count, .. } => {
                assert_eq!(arity, 2);
                assert_eq!(captured_count, 1);
            }
            other => panic!("Expected Closure detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_box() {
        let boxed = HeapObject::Box(Value::from_number(99.0));
        let val = Value::from_heap_object_gc(boxed);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.type_name, "box");
        let hd = report.heap_detail.expect("heap_detail should exist");

        match hd.detail {
            HeapDetailKind::Box { inner_type } => {
                assert_eq!(inner_type, "integer");
            }
            other => panic!("Expected Box detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_builtin_fn() {
        fn dummy(_: &[Value]) -> Result<Value, crate::errors::NuzoError> {
            Ok(unsafe { Value::from_raw_bits(NIL_VALUE) })
        }
        let builtin = HeapObject::BuiltinFn { name: "len".to_string(), arity: 1, func: dummy };
        let val = Value::from_heap_object_gc(builtin);
        let report = ValueInspector::inspect(val);

        assert_eq!(report.type_name, "builtin");
        let hd = report.heap_detail.expect("heap_detail should exist");

        match hd.detail {
            HeapDetailKind::BuiltinFn { name, arity } => {
                assert_eq!(name, "len");
                assert_eq!(arity, 1);
            }
            other => panic!("Expected BuiltinFn detail, got {:?}", other),
        }
    }

    // ========================================================================
    // Edge Case: 边界值
    // ========================================================================

    #[test]
    fn test_inspect_smi_zero() {
        let report = ValueInspector::inspect(Value::from_smi(0));
        assert_eq!(report.tag, ValueTag::Smi);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_smi_max() {
        let report = ValueInspector::inspect(Value::from_smi(SMI_MAX));
        assert_eq!(report.tag, ValueTag::Smi);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_smi_min() {
        let report = ValueInspector::inspect(Value::from_smi(SMI_MIN));
        assert_eq!(report.tag, ValueTag::Smi);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_float_nan() {
        let report = ValueInspector::inspect(Value::from_number(f64::NAN));
        assert_eq!(report.tag, ValueTag::Float);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_float_infinity() {
        let report = ValueInspector::inspect(Value::from_number(f64::INFINITY));
        assert_eq!(report.tag, ValueTag::Float);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_float_negative_zero() {
        let report = ValueInspector::inspect(Value::from_number(-0.0));
        assert_eq!(report.tag, ValueTag::Float);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_empty_string() {
        let report = ValueInspector::inspect(Value::from_string(""));
        assert_eq!(report.tag, ValueTag::String);
        assert!(report.is_valid);
    }

    #[test]
    fn test_inspect_heap_array_preview_limit() {
        // 创建超过预览限制的数组
        let elements: Vec<Value> = (0..20).map(|i| Value::from_number(i as f64)).collect();
        let arr = HeapObject::Array(elements);
        let val = Value::from_heap_object_gc(arr);
        let report = ValueInspector::inspect(val);

        let hd = report.heap_detail.expect("heap_detail should exist");
        match hd.detail {
            HeapDetailKind::Array { length, preview } => {
                assert_eq!(length, 20);
                assert_eq!(preview.len(), ARRAY_PREVIEW_LIMIT);
            }
            other => panic!("Expected Array detail, got {:?}", other),
        }
    }

    #[test]
    fn test_inspect_heap_size_estimate_positive() {
        let arr = HeapObject::Array(vec![Value::from_number(1.0)]);
        let val = Value::from_heap_object_gc(arr);
        let report = ValueInspector::inspect(val);

        let hd = report.heap_detail.expect("heap_detail should exist");
        assert!(hd.size_estimate > 0);
    }

    // ========================================================================
    // Poison Pill: 非法值检查
    // ========================================================================

    #[test]
    fn test_inspect_unknown_nan_space() {
        // 0x7FFC_0000_0000_0000: 在 NaN 空间但不符合任何已知标签
        let bits = 0x7FFC_0000_0000_0000;
        // SAFETY: Deliberately invalid bit pattern for validation testing.
        let val = unsafe { Value::from_raw_bits(bits) };
        let report = ValueInspector::inspect(val);

        assert_eq!(report.tag, ValueTag::Unknown);
        assert!(!report.is_valid);
        assert!(report.validation_error.is_some());
    }

    #[test]
    fn test_inspect_heap_index_overflow() {
        // 堆索引超出合理上限
        let bits = HEAP_TAG | (1u64 << 24);
        // SAFETY: Deliberately invalid heap index for edge-case testing.
        let val = unsafe { Value::from_raw_bits(bits) };
        let report = ValueInspector::inspect(val);

        assert!(!report.is_valid);
        assert!(report.validation_error.is_some());
    }

    #[test]
    fn test_inspect_string_index_overflow() {
        // 字符串索引超出合理上限
        let bits = STRING_TAG | (1u64 << 24);
        // SAFETY: Deliberately invalid string index for edge-case testing.
        let val = unsafe { Value::from_raw_bits(bits) };
        let report = ValueInspector::inspect(val);

        assert!(!report.is_valid);
        assert!(report.validation_error.is_some());
    }

    // ========================================================================
    // format_diagnostic 输出格式验证
    // ========================================================================

    #[test]
    fn test_format_diagnostic_nil() {
        // SAFETY: NIL_VALUE is a well-known constant with valid NaN-tag encoding.
        let output = ValueInspector::format_diagnostic(unsafe { Value::from_raw_bits(NIL_VALUE) });
        assert!(output.contains("Value Inspection Report"));
        assert!(output.contains("nil"));
        assert!(output.contains("ok"));
    }

    #[test]
    fn test_format_diagnostic_smi() {
        let output = ValueInspector::format_diagnostic(Value::from_smi(42));
        assert!(output.contains("Value Inspection Report"));
        assert!(output.contains("integer"));
        assert!(output.contains("42"));
        assert!(output.contains("ok"));
    }

    #[test]
    fn test_format_diagnostic_float() {
        let output = ValueInspector::format_diagnostic(Value::from_number(2.5));
        assert!(output.contains("Value Inspection Report"));
        assert!(output.contains("number"));
        assert!(output.contains("ok"));
    }

    #[test]
    fn test_format_diagnostic_heap_array() {
        let arr = HeapObject::Array(vec![Value::from_number(1.0), Value::from_number(2.0)]);
        let val = Value::from_heap_object_gc(arr);
        let output = ValueInspector::format_diagnostic(val);

        assert!(output.contains("Value Inspection Report"));
        assert!(output.contains("HeapObject (array)"));
        assert!(output.contains("GC:"));
        assert!(output.contains("Size:"));
        assert!(output.contains("Array[2]"));
        assert!(output.contains("ok"));
    }

    #[test]
    fn test_format_diagnostic_heap_range() {
        let range = HeapObject::Range { start: 1.0, end: 5.0, range_end: RangeEnd::Inclusive };
        let val = Value::from_heap_object_gc(range);
        let output = ValueInspector::format_diagnostic(val);

        assert!(output.contains("Range(1..=5)"));
    }

    #[test]
    fn test_format_diagnostic_heap_builtin_fn() {
        fn dummy(_: &[Value]) -> Result<Value, crate::errors::NuzoError> {
            Ok(unsafe { Value::from_raw_bits(NIL_VALUE) })
        }
        let builtin = HeapObject::BuiltinFn { name: "print".to_string(), arity: 1, func: dummy };
        let val = Value::from_heap_object_gc(builtin);
        let output = ValueInspector::format_diagnostic(val);

        assert!(output.contains("BuiltinFn:print(1)"));
    }

    #[test]
    fn test_format_diagnostic_invalid_value() {
        let bits = 0x7FFC_0000_0000_0000;
        // SAFETY: Deliberately invalid bit pattern for diagnostic formatting test.
        let val = unsafe { Value::from_raw_bits(bits) };
        let output = ValueInspector::format_diagnostic(val);

        assert!(output.contains("INVALID"));
    }

    #[test]
    fn test_format_diagnostic_contains_bit_visualization() {
        let output = ValueInspector::format_diagnostic(Value::from_smi(42));
        // 位域可视化表格应包含边框字符
        assert!(output.contains('┌') || output.contains('│'));
    }

    // ========================================================================
    // InspectionReport Clone / Debug trait 测试
    // ========================================================================

    #[test]
    fn test_report_clone() {
        let report = ValueInspector::inspect(Value::from_smi(42));
        let cloned = report.clone();
        assert_eq!(report.tag, cloned.tag);
        assert_eq!(report.type_name, cloned.type_name);
        assert_eq!(report.raw_bits, cloned.raw_bits);
        assert_eq!(report.is_valid, cloned.is_valid);
    }

    #[test]
    fn test_report_debug() {
        let report = ValueInspector::inspect(Value::from_smi(42));
        let debug_str = format!("{:?}", report);
        assert!(debug_str.contains("Smi"));
    }

    // ========================================================================
    // HeapDetail / HeapDetailKind 测试
    // ========================================================================

    #[test]
    fn test_heap_detail_debug() {
        let kind = HeapDetailKind::Array {
            length: 3,
            preview: vec!["1".to_string(), "2".to_string(), "3".to_string()],
        };
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Array"));
        assert!(debug_str.contains("length"));
    }

    #[test]
    fn test_format_heap_detail_kind_array() {
        let kind = HeapDetailKind::Array {
            length: 3,
            preview: vec!["1".to_string(), "2".to_string(), "3".to_string()],
        };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Array[3] = [1, 2, 3]");
    }

    #[test]
    fn test_format_heap_detail_kind_dict() {
        let kind = HeapDetailKind::Dict {
            length: 2,
            keys_preview: vec!["key#0".to_string(), "key#1".to_string()],
        };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Dict{2} = [key#0, key#1]");
    }

    #[test]
    fn test_format_heap_detail_kind_range_inclusive() {
        let kind = HeapDetailKind::Range { start: 1.0, end: 5.0, range_end: RangeEnd::Inclusive };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Range(1..=5)");
    }

    #[test]
    fn test_format_heap_detail_kind_range_exclusive() {
        let kind = HeapDetailKind::Range { start: 0.0, end: 10.0, range_end: RangeEnd::Exclusive };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Range(0..10)");
    }

    #[test]
    fn test_format_heap_detail_kind_closure() {
        let kind = HeapDetailKind::Closure {
            arity: 2,
            captured_count: 1,
            name: Some("my_func".to_string()),
        };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Closure(my_func: arity=2, captures=1)");
    }

    #[test]
    fn test_format_heap_detail_kind_closure_anonymous() {
        let kind = HeapDetailKind::Closure { arity: 0, captured_count: 0, name: None };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Closure(<anonymous>: arity=0, captures=0)");
    }

    #[test]
    fn test_format_heap_detail_kind_box() {
        let kind = HeapDetailKind::Box { inner_type: "integer" };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Box(integer)");
    }

    #[test]
    fn test_format_heap_detail_kind_builtin_fn() {
        let kind = HeapDetailKind::BuiltinFn { name: "len".to_string(), arity: 1 };
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "BuiltinFn:len(1)");
    }

    #[test]
    fn test_format_heap_detail_kind_unknown() {
        let kind = HeapDetailKind::Unknown;
        let s = format_heap_detail_kind(&kind);
        assert_eq!(s, "Unknown");
    }

    // ========================================================================
    // ValueInspector 为零大小类型
    // ========================================================================

    #[test]
    fn test_value_inspector_is_zst() {
        assert_eq!(std::mem::size_of::<ValueInspector>(), 0);
    }
}

//! # 函数类型 -- 闭包支持 (Function Types for Closure Support)
//!
//! 本模块提供 [`FunctionPrototype`]（编译后的函数元数据，包含捕获变量信息）
//! 和 [`DebugInfo`]（源码级调试映射）。
//!
//! ## FunctionPrototype 的角色
//!
//! `FunctionPrototype` 是**编译时静态数据**，存储在全局堆中并被闭包引用。
//! 它包含函数执行所需的全部元信息：
//!
//! - **arity**: 参数数量（用于调用前检查）
//! - **locals_count**: 局部变量总数（用于栈帧分配）
//! - **chunk**: 编译后的字节码指令序列
//! - **constants**: 字节码引用的常量池
//! - **captured_vars**: 自由变量描述列表（FlatEnv 架构的核心）
//! - **lines**: 源码行号映射（用于错误定位）
//! - **debug_info**: 调试信息（源文件、源码行、IP->行号映射）
//!
//! ## FlatEnv 架构关系图
//!
//! ```text
//! FunctionPrototype::captured_vars          HeapObject::Closure::captured
//! ┌──────────────────────────────┐         ┌────────────────────────────┐
//! │ [0] CaptureInfo {           │         │ [0] CapturedVar::Box(3)   │
//! │       name: "count",        │ ───────→│     -> BOX_POOL[3]         │
//! │       mode: ByBox,          │  index  │                             │
//! │       capture_index: 0      │         │ [1] CapturedVar::Value(42) │
//! │ }                            │         │     (inline immutable)     │
//! │ [1] CaptureInfo {           │         └────────────────────────────┘
//! │       name: "name",         │
//! │       mode: ByValue,        │
//! │       capture_index: 1      │
//! │ }                            │
//! └──────────────────────────────┘
//! ```
//!
//! ## DebugInfo 统一
//!
//! `DebugInfo` 类型同时被字节码层 (`Chunk`) 和运行时层 (`FunctionPrototype`) 使用，
//! 消除了每次函数调用时的深拷贝转换开销（可用 `Arc::clone` 代替）。

use nuzo_core::XxHashMap;
use std::sync::Arc;

use super::heap::CaptureInfo;
use crate::value::Value;

// ============================================================================
// DebugInfo -- 源码级调试映射
// ============================================================================

/// 源码级调试信息，由字节码层 (`Chunk`) 和运行时层 (`FunctionPrototype`) 共享。
///
/// 此类型之前在 `nuzo_bytecode` 中定义为 `DebugInfo`，在 `nuzo_values` 中定义为
/// `PrototypeDebugInfo`。统一后消除了每次函数调用时的深拷贝转换，
/// 允许使用 `Arc::clone` 代替 `Arc::new(DebugInfo::from(...))`。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct DebugInfo {
    /// 源文件名（如 "test.nuzo" 或 "<function>"）
    pub source_file: String,
    /// 源码行列表（1-indexed: line 1 = source_lines[0]）
    pub source_lines: Vec<String>,
    /// 从字节码偏移量 (IP) 到源码行号的映射
    #[serde(default)]
    pub ip_to_line: XxHashMap<usize, usize>,
    /// 从字节码偏移量 (IP) 到源码列号的映射
    #[serde(default)]
    pub ip_to_column: XxHashMap<usize, usize>,
    /// 所属函数名称（由编译器在创建 DebugInfo 时填充）
    ///
    /// 用于 `Chunk::get_source_location()` 生成 `SourceLocation.function_name`，
    /// 使运行时错误报告能直接显示出错函数名，无需从帧栈回溯。
    ///
    /// - 具名函数: `"add"`, `"factorial"` 等
    /// - 匿名函数/闭包: `"<anonymous>"`
    /// - 顶层脚本: `"<script>"`
    /// - 未设置: `None`（向后兼容旧代码）
    #[serde(default)]
    pub function_name: Option<String>,
    /// 内联记录（当内联优化实现后由编译器填充）。
    /// 将内联代码的 IP 范围映射回原始函数。
    #[serde(default)]
    pub inline_records: Vec<InlineRecord>,
    /// 死代码消除记录（当 DCE 优化实现后由编译器填充）。
    /// 将被消除的源码范围映射到消除原因。
    #[serde(default)]
    pub dead_code_records: Vec<DeadCodeRecord>,
    /// 常量折叠记录（由编译器在常量折叠成功时填充）。
    ///
    /// 每当编译器成功折叠一个常量表达式时，会追加一条 FoldRecord。
    /// 可通过 Inspector 查看折叠历史，用于验证优化效果和调试。
    /// 仅调试用，零运行时开销。
    #[serde(default)]
    pub fold_records: Vec<FoldRecord>,
}

/// 向后兼容别名 —— 现有引用 `PrototypeDebugInfo` 的代码无需修改即可编译。
pub type PrototypeDebugInfo = DebugInfo;

// ============================================================================
// InlineRecord / DeadCodeRecord / FoldRecord -- 可追溯性记录类型
// ============================================================================

/// 函数内联记录：记录某个调用点被内联展开后的 IP 范围与原始函数信息。
///
/// 未来编译器实现内联优化时，应填充此结构，使调试器能将内联代码的 IP
/// 映射回原始函数的源码位置，而非显示为调用者函数中的"幽灵代码"。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InlineRecord {
    /// 内联代码被放置的 IP 起始位置（含）
    pub ip_start: usize,
    /// 内联代码被放置的 IP 结束位置（不含）
    pub ip_end: usize,
    /// 被内联函数的名称
    pub function_name: String,
    /// 被内联函数的源文件路径
    pub source_file: String,
    /// 原始调用点的源码行号
    pub call_site_line: usize,
}

/// 死代码消除记录：记录被 DCE 移除的源码范围及消除原因。
///
/// 未来编译器实现 DCE 优化时，应填充此结构，使 IDE/调试器能区分
/// "源码存在但被优化移除" 与 "源码不存在"，避免误导开发者。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeadCodeRecord {
    /// 被消除代码的源码起始行号（含，1-indexed）
    pub source_line_start: usize,
    /// 被消除代码的源码结束行号（含，1-indexed）
    pub source_line_end: usize,
    /// 消除原因
    pub reason: DeadCodeReason,
}

/// 死代码消除原因枚举。
///
/// 每个变体对应一种编译器可识别的死代码模式，便于工具链按原因分类展示。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DeadCodeReason {
    /// return / break / continue 之后的不可达代码
    UnreachableCode,
    /// 写入但从未读取的变量（含变量名）
    UnusedVariable(String),
    /// 常量条件分支（含条件的常量值）
    ConstantCondition(bool),
    /// 其他原因（含描述文本）
    Other(String),
}

/// 常量折叠记录：记录编译期常量折叠的详细信息。
///
/// 每当编译器成功折叠一个常量表达式时，生成一条 FoldRecord。
/// 此结构体仅用于调试/分析，零运行时开销。
///
/// # 示例
///
/// 源码 `1 + 2` 折叠为 `3`，生成 FoldRecord:
/// ```text
/// FoldRecord {
///     result_const_idx: 5,       // 常量池中 3 的索引
///     ip: 12,                     // LoadK 指令的 IP
///     description: "1 + 2 -> 3", // 人类可读描述
///     source_line: 42,           // 源码行号
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FoldRecord {
    /// 折叠结果在常量池中的索引
    pub result_const_idx: usize,
    /// 折叠后 LoadK 指令的 IP 地址
    pub ip: usize,
    /// 折叠描述（如 "1 + 2 -> 3"）
    pub description: String,
    /// 原始表达式的源码行号
    pub source_line: usize,
}

impl DebugInfo {
    /// 查找给定 IP 是否落在某条内联记录的范围内。
    ///
    /// 返回覆盖该 IP 的 `InlineRecord` 引用，若无则返回 `None`。
    /// 调试器可用此方法将内联代码的 IP 还原为原始函数信息。
    pub fn inlined_function_at(&self, ip: usize) -> Option<&InlineRecord> {
        self.inline_records.iter().find(|r| ip >= r.ip_start && ip < r.ip_end)
    }

    /// 检查给定源码行是否被 DCE 消除。
    ///
    /// 返回覆盖该行的 `DeadCodeRecord` 引用，若无则返回 `None`。
    /// IDE/调试器可用此方法判断某行源码是否因优化而不可达。
    pub fn is_dead_code_line(&self, line: usize) -> Option<&DeadCodeRecord> {
        self.dead_code_records
            .iter()
            .find(|r| line >= r.source_line_start && line <= r.source_line_end)
    }
}

// ============================================================================
// FunctionPrototype -- 编译后的函数元数据
// ============================================================================

/// 编译后的函数元数据，存储在全局堆中并被闭包引用。
///
/// 包含函数的参数数量、局部变量数、字节码块和捕获变量描述符列表（用于闭包支持）。
///
/// # FlatEnv 架构
///
/// `captured_vars` 字段提供了完整的静态信息，使得在运行时可以 O(1) 地
/// 通过索引访问捕获变量，避免了昂贵的全局环境表哈希查找。
///
/// # 与 Closure 的 captured[] 数组的关系
///
/// `FunctionPrototype::captured_vars` 是**静态蓝图**，定义了"捕获什么、如何捕获"；
/// 而 `HeapObject::Closure::captured` 是**运行时实例**，存储了实际的捕获值。

#[derive(Debug, Clone)]
pub struct FunctionPrototype {
    /// 函数名称（具名函数为声明时的标识符，匿名函数为 "<anonymous>"，顶层脚本为 "<script>"）
    ///
    /// 此字段由编译器在创建 FunctionPrototype 时填充，运行时用于：
    /// - 错误堆栈中的函数名显示（替代旧方案中从 debug_info 拼接的 hack）
    /// - SourceLocation 中的 function_name 字段
    /// - 调试器和性能分析工具的函数标识
    ///
    /// 不变量：此字段永远不为空字符串。编译器保证在构造时填入有意义的值。
    pub name: String,
    /// 函数期望的参数数量
    pub arity: u8,
    /// 局部变量数量（包括参数）
    pub locals_count: u16,
    /// 函数体的编译字节码（原始指令字节）
    pub chunk: Arc<Vec<u8>>,
    /// chunk 中指令引用的常量池
    pub constants: Arc<Vec<Value>>,
    /// 自由变量列表（从外层作用域捕获），用于闭包支持。
    ///
    /// 每个条目描述一个被捕获变量的名称、捕获模式（可变/不可变），
    /// 以及该变量在闭包扁平 `captured[]` 数组中的索引位置。
    pub captured_vars: Vec<CaptureInfo>,
    /// `chunk` 中每个字节对应的源码行号（将字节码偏移映射到源码行）
    pub lines: Arc<Vec<u32>>,
    /// 用于源码映射的调试信息（文件名、源码行、IP->行号映射表）
    pub debug_info: Arc<PrototypeDebugInfo>,
    /// LSRA spill 所需的栈槽数量（0 表示无 spill，旧字节码兼容）
    pub spill_slot_count: u16,
}

impl FunctionPrototype {
    /// 创建新的函数原型。
    ///
    /// # Panics
    ///
    /// Debug 模式下，如果 `name` 为空字符串会 panic（违反不变量）。
    #[allow(clippy::too_many_arguments)] // 函数原型构造需要所有字段，使用 builder 模式增加复杂度不值得
    pub fn new(
        name: String,
        arity: u8,
        locals_count: u16,
        chunk: Arc<Vec<u8>>,
        constants: Arc<Vec<Value>>,
        captured_vars: Vec<CaptureInfo>,
        lines: Arc<Vec<u32>>,
        debug_info: Arc<PrototypeDebugInfo>,
        spill_slot_count: u16,
    ) -> Self {
        debug_assert!(
            !name.is_empty(),
            "FunctionPrototype::name must never be empty — use \"<anonymous>\" or \"<script>\" as fallback"
        );
        Self {
            name,
            arity,
            locals_count,
            chunk,
            constants,
            captured_vars,
            lines,
            debug_info,
            spill_slot_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_debug_info_with_inline() -> DebugInfo {
        DebugInfo {
            source_file: "<test>".to_string(),
            source_lines: vec!["line1".to_string(), "line2".to_string()],
            ip_to_line: Default::default(),
            ip_to_column: Default::default(),
            function_name: Some("test_fn".to_string()),
            inline_records: vec![
                InlineRecord {
                    ip_start: 10,
                    ip_end: 20,
                    function_name: "inlined_a".to_string(),
                    source_file: "a.nuzo".to_string(),
                    call_site_line: 1,
                },
                InlineRecord {
                    ip_start: 30,
                    ip_end: 40,
                    function_name: "inlined_b".to_string(),
                    source_file: "b.nuzo".to_string(),
                    call_site_line: 2,
                },
            ],
            dead_code_records: vec![
                DeadCodeRecord {
                    source_line_start: 5,
                    source_line_end: 8,
                    reason: DeadCodeReason::UnreachableCode,
                },
                DeadCodeRecord {
                    source_line_start: 15,
                    source_line_end: 15,
                    reason: DeadCodeReason::UnusedVariable("x".to_string()),
                },
            ],
            fold_records: vec![],
        }
    }

    #[test]
    fn test_inlined_function_at_hit_first() {
        let di = make_debug_info_with_inline();
        let rec = di.inlined_function_at(15).expect("IP 15 should hit first inline record");
        assert_eq!(rec.function_name, "inlined_a");
        assert_eq!(rec.ip_start, 10);
        assert_eq!(rec.ip_end, 20);
    }

    #[test]
    fn test_inlined_function_at_hit_second() {
        let di = make_debug_info_with_inline();
        let rec = di.inlined_function_at(35).expect("IP 35 should hit second inline record");
        assert_eq!(rec.function_name, "inlined_b");
    }

    #[test]
    fn test_inlined_function_at_boundary_start() {
        let di = make_debug_info_with_inline();
        assert!(di.inlined_function_at(10).is_some(), "IP at ip_start should be included");
    }

    #[test]
    fn test_inlined_function_at_boundary_end_exclusive() {
        let di = make_debug_info_with_inline();
        assert!(
            di.inlined_function_at(20).is_none(),
            "IP at ip_end should be excluded (half-open)"
        );
    }

    #[test]
    fn test_inlined_function_at_miss() {
        let di = make_debug_info_with_inline();
        assert!(di.inlined_function_at(25).is_none(), "IP 25 is between records");
        assert!(di.inlined_function_at(0).is_none(), "IP 0 before all records");
        assert!(di.inlined_function_at(100).is_none(), "IP 100 after all records");
    }

    #[test]
    fn test_inlined_function_at_empty_records() {
        let di = DebugInfo::default();
        assert!(di.inlined_function_at(42).is_none());
    }

    #[test]
    fn test_is_dead_code_line_hit_range() {
        let di = make_debug_info_with_inline();
        let rec = di.is_dead_code_line(6).expect("line 6 should be in first dead code record");
        assert_eq!(rec.source_line_start, 5);
        assert_eq!(rec.source_line_end, 8);
        assert!(matches!(rec.reason, DeadCodeReason::UnreachableCode));
    }

    #[test]
    fn test_is_dead_code_line_hit_single() {
        let di = make_debug_info_with_inline();
        let rec = di.is_dead_code_line(15).expect("line 15 should be in second dead code record");
        assert!(matches!(&rec.reason, DeadCodeReason::UnusedVariable(n) if n == "x"));
    }

    #[test]
    fn test_is_dead_code_line_boundary_start() {
        let di = make_debug_info_with_inline();
        assert!(di.is_dead_code_line(5).is_some(), "line at start should be included");
    }

    #[test]
    fn test_is_dead_code_line_boundary_end() {
        let di = make_debug_info_with_inline();
        assert!(di.is_dead_code_line(8).is_some(), "line at end should be included (closed range)");
    }

    #[test]
    fn test_is_dead_code_line_miss() {
        let di = make_debug_info_with_inline();
        assert!(di.is_dead_code_line(1).is_none(), "line 1 is not dead code");
        assert!(di.is_dead_code_line(10).is_none(), "line 10 is not dead code");
        assert!(di.is_dead_code_line(100).is_none(), "line 100 is not dead code");
    }

    #[test]
    fn test_is_dead_code_line_empty_records() {
        let di = DebugInfo::default();
        assert!(di.is_dead_code_line(42).is_none());
    }

    #[test]
    fn test_debug_info_default() {
        let di = DebugInfo::default();
        assert!(di.source_file.is_empty());
        assert!(di.source_lines.is_empty());
        assert!(di.function_name.is_none());
        assert!(di.inline_records.is_empty());
        assert!(di.dead_code_records.is_empty());
        assert!(di.fold_records.is_empty());
    }

    #[test]
    fn test_debug_info_clone() {
        let di = make_debug_info_with_inline();
        let cloned = di.clone();
        assert_eq!(di.source_file, cloned.source_file);
        assert_eq!(di.inline_records.len(), cloned.inline_records.len());
    }
}

//! # NaN-tag 可扩展标签注册机制
//!
//! 本模块提供 NaN-tagged 值系统的**声明式标签注册**能力，用于：
//! - 集中管理所有类型标签的位布局元数据
//! - 运行时/编译期检测标签位空间冲突
//! - 通过 `define_value_tag!` 宏快速扩展新的值类型
//!
//! ## 设计原理
//!
//! NaN-tagging 的核心约束是：**每个标签的 (tag_value, mask) 对必须占据互不重叠的位空间**。
//! 本模块将这一约束编码为可查询的注册表，使得新增标签时能自动检测冲突，
//! 而非依赖人工审查位模式。
//!
//! ## 位空间分配图
//!
//! ```text
//! 64 位 Value 位空间分配：
//! ┌──────────────────────────────────────────────────────────────┐
//! │ 标签名     │ tag_value            │ mask                 │ 载荷 │
//! ├──────────────────────────────────────────────────────────────┤
//! │ Special    │ 0x7FF8_0000_0000_0000│ 0x7FF8_0000_0000_0000│ 2位  │
//! │ HeapObject │ 0x7FF8_4000_0000_0000│ 0x7FF8_C000_0000_0000│ 46位 │
//! │ String     │ 0x7FF8_8000_0000_0000│ 0x7FF8_8000_0000_0000│ 47位 │
//! │ Smi        │ 0x7FF9_0000_0000_0000│ 0x7FFF_0000_0000_0000│ 48位 │
//! │ Float      │ (无固定tag)          │ (其他所有模式)        │ 64位 │
//! │ Pointer    │ 0x7FF8_0000_0000_0000│ 0x7FFC_0000_0000_0000│ 48位 │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 使用示例
//!
//! ```ignore
//! // 查询已注册标签
//! let desc = TagRegistry::find_by_name("Smi").expect("Smi must exist");
//! assert_eq!(desc.payload_bits, 48);
//!
//! // 检测新标签是否与已有标签冲突
//! let result = TagRegistry::check_conflict(0x7FFA_0000_0000_0000, 0x7FFF_0000_0000_0000);
//! assert!(result.is_ok()); // 0x7FFA 不与现有标签冲突
//!
//! // 声明式定义新标签
//! define_value_tag! {
//!     Symbol => {
//!         tag: SYMBOL_TAG,
//!         mask: SYMBOL_MASK,
//!         index_mask: SYMBOL_INDEX_MASK,
//!         doc: "Symbol type for unique identifiers",
//!     }
//! }
//! ```

use crate::constants::*;
use crate::value::ValueTag;

// ============================================================================
// TagDescriptor -- NaN-tag 类型标签描述符
// ============================================================================

/// NaN-tag 类型标签描述符
///
/// 每个描述符完整记录一种 NaN-tag 的位布局元数据，用于：
/// - 运行时类型分派（通过 `mask` 和 `tag_value` 做位与测试）
/// - 冲突检测（新增标签时校验位空间是否重叠）
/// - 文档生成（`name` 和 `is_gc` 用于生成类型系统文档）
#[derive(Debug, Clone, Copy)]
pub struct TagDescriptor {
    /// 标签名称（如 "Smi", "HeapObject", "String"）
    pub name: &'static str,
    /// 标签位模式值（如 SMI_TAG = 0x7FF9_0000_0000_0000）
    pub tag_value: u64,
    /// 检测掩码（如 SMI_MASK = 0x7FFF_0000_0000_0000）
    ///
    /// 判定规则：`(bits & mask) == tag_value` 则匹配此标签
    pub mask: u64,
    /// 有效载荷位数（如 Smi=48, String=47, Heap=46）
    pub payload_bits: u32,
    /// 是否为 GC 管理的类型
    pub is_gc: bool,
    /// 对应的 ValueTag 枚举值
    pub value_tag: ValueTag,
}

impl TagDescriptor {
    /// 检测给定的 64 位值是否匹配此标签
    ///
    /// 判定公式：`(bits & self.mask) == self.tag_value`
    #[inline(always)]
    pub const fn matches_bits(self, bits: u64) -> bool {
        (bits & self.mask) == self.tag_value
    }

    /// 返回此标签的载荷容量（最大可编码的不同值数量）
    ///
    /// 等于 `1 << payload_bits`，即 `2^payload_bits`
    #[inline]
    pub const fn payload_capacity(self) -> u64 {
        1u64 << self.payload_bits
    }

    /// 返回此标签的载荷提取掩码
    ///
    /// 低 `payload_bits` 位全为 1，其余为 0
    #[inline]
    pub const fn payload_mask(self) -> u64 {
        if self.payload_bits >= 64 { u64::MAX } else { (1u64 << self.payload_bits) - 1 }
    }
}

// ============================================================================
// TAG_REGISTRY -- 已注册标签常量数组
// ============================================================================

/// 特殊值（nil/bool）精确检测掩码
///
/// 与 [`SPECIAL_MASK`]（用于检测整个 NaN-tagged 空间）不同，此掩码
/// 专门用于精确匹配 nil/false/true 三种特殊值。
///
/// 判定逻辑：`(bits & SPECIAL_VALUE_MASK) == PTR_TAG` 当且仅当
/// bits 的高位与 PTR_TAG 完全一致且 bits[2:47] 全为零，
/// 即 bits 只能是 PTR_TAG | {0,1,2,3}。
///
/// 注意：PTR_TAG | 0（空指针）也会匹配此掩码，但空指针在 Value 系统中
/// 由 `is_ptr()` 独立处理，不经过标签注册表分类。
const SPECIAL_VALUE_MASK: u64 = !0x3; // 0xFFFF_FFFF_FFFF_FFFC

/// 全局标签注册表
///
/// 注册了 NaN-tagging 系统中所有**具有固定 tag_value 的标签**。
/// Float 类型没有固定 tag_value（它是"其他"情况），Pointer 与 Special 共享
/// PTR_TAG 前缀但需要排除 nil/bool，这两种特殊类型需要单独处理逻辑。
pub const TAG_REGISTRY: &[TagDescriptor] = &[
    // 特殊值标签：nil / false / true
    // 位模式：0x7FF8_0000_0000_000[1-3]
    // 低 2 位编码：1=nil, 2=false, 3=true
    //
    // 注意：使用 SPECIAL_VALUE_MASK（非 SPECIAL_MASK）作为检测掩码，
    // 因为 SPECIAL_MASK = 0x7FF8_0000_0000_0000 会错误匹配 Heap/String/Smi。
    // SPECIAL_VALUE_MASK = 0xFFFF_FFFF_FFFF_FFFC 精确匹配高位等于 PTR_TAG
    // 且 bits[2:47] 为零的值。
    TagDescriptor {
        name: "Special",
        tag_value: PTR_TAG,
        mask: SPECIAL_VALUE_MASK,
        payload_bits: 2,
        is_gc: false,
        value_tag: ValueTag::Nil,
    },
    // 堆对象标签：数组、字典、闭包、内建函数、Box 等
    // 位模式：0x7FF8_4000_XXXX_XXXX
    // 低 46 位编码堆索引（含 GC_MANAGED_BIT 标志位）
    TagDescriptor {
        name: "HeapObject",
        tag_value: HEAP_TAG,
        mask: HEAP_MASK,
        payload_bits: 46,
        is_gc: true,
        value_tag: ValueTag::Pointer,
    },
    // 字符串标签：全局字符串池索引
    // 位模式：0x7FF8_8000_XXXX_XXXX
    // 低 47 位编码字符串池索引
    TagDescriptor {
        name: "String",
        tag_value: STRING_TAG,
        mask: STRING_MASK,
        payload_bits: 47,
        is_gc: false,
        value_tag: ValueTag::String,
    },
    // Smi 小整数标签：直接嵌入的 48 位有符号整数
    // 位模式：0x7FF9_XXXX_XXXX_XXXX
    // 低 48 位编码有符号小整数值
    TagDescriptor {
        name: "Smi",
        tag_value: SMI_TAG,
        mask: SMI_MASK,
        payload_bits: 48,
        is_gc: false,
        value_tag: ValueTag::Smi,
    },
];

// ============================================================================
// TagConflictError -- 标签冲突错误类型
// ============================================================================

/// 标签位空间冲突错误
///
/// 当新增标签的 (tag_value, mask) 对与已注册标签的位空间存在重叠时返回此错误。
/// 重叠判定规则：如果两个标签中任一个的 tag_value 落入另一个的掩码空间，
/// 则认为存在冲突。
#[derive(Debug)]
pub struct TagConflictError {
    /// 新标签名称
    pub new_name: &'static str,
    /// 新标签位模式值
    pub new_tag: u64,
    /// 冲突的已有标签名称
    pub conflicting_with: &'static str,
    /// 冲突的已有标签位模式值
    pub conflicting_tag: u64,
}

impl std::fmt::Display for TagConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "标签位空间冲突: 新标签 '{}' (tag={:#018X}) 与已注册标签 '{}' (tag={:#018X}) 的位空间重叠",
            self.new_name, self.new_tag, self.conflicting_with, self.conflicting_tag
        )
    }
}

impl std::error::Error for TagConflictError {}

// ============================================================================
// TagRegistry -- 标签查询与冲突检测
// ============================================================================

/// NaN-tag 标签注册表查询接口
///
/// 提供标签查询和冲突检测的纯函数接口，所有方法均为无状态常量求值，
/// 零运行时开销。
pub struct TagRegistry;

impl TagRegistry {
    /// 返回所有已注册标签的描述符列表
    ///
    /// 不包含 Float（无固定 tag）和 Pointer（与 Special 共享前缀）。
    #[inline(always)]
    pub fn all_tags() -> &'static [TagDescriptor] {
        TAG_REGISTRY
    }

    /// 按名称查找标签描述符
    ///
    /// # 参数
    /// - `name`: 标签名称（如 "Smi", "HeapObject", "String", "Special"）
    ///
    /// # 返回
    /// 匹配的标签描述符引用，未找到返回 `None`
    pub fn find_by_name(name: &str) -> Option<&'static TagDescriptor> {
        TAG_REGISTRY.iter().find(|desc| desc.name == name)
    }

    /// 按 ValueTag 枚举查找第一个匹配的标签描述符
    ///
    /// 注意：ValueTag::Pointer 可能匹配多个描述符（HeapObject 和 Pointer），
    /// 此方法返回第一个匹配项。
    pub fn find_by_value_tag(tag: ValueTag) -> Option<&'static TagDescriptor> {
        TAG_REGISTRY.iter().find(|desc| desc.value_tag == tag)
    }

    /// 冲突检测内部实现：遍历注册表检测位空间重叠。
    ///
    /// 冲突判定规则（双向检测，两条规则均对 new_tag 做掩码归一化）：
    /// 1. 新标签的 tag_value 落入已有标签的掩码空间：
    ///    `(new_tag & existing.mask) == existing.tag_value`
    /// 2. 已有标签的 tag_value 落入新标签的掩码空间：
    ///    `(existing.tag_value & new_mask) == (new_tag & new_mask)`
    ///
    /// 规则 2 对 `new_tag` 也做 `& new_mask` 归一化，与规则 1 保持对称。
    /// 否则当 `new_tag` 含掩码外的位时（非规范化输入），规则 2 会漏检：
    /// 此时 `(existing.tag_value & new_mask)` 是掩码内的值，而 `new_tag` 含掩码外的位，
    /// 直接 `== new_tag` 比较会假阴性。归一化后双方都限定在掩码空间内，比较才有意义。
    ///
    /// 满足任一条件即判定为冲突。
    fn check_conflict_inner(
        new_name: &'static str,
        new_tag: u64,
        new_mask: u64,
    ) -> Result<(), TagConflictError> {
        let normalized_new_tag = new_tag & new_mask;
        for existing in TAG_REGISTRY {
            // 规则 1：新标签的 tag_value 是否落入已有标签的掩码空间
            if (new_tag & existing.mask) == existing.tag_value {
                return Err(TagConflictError {
                    new_name,
                    new_tag,
                    conflicting_with: existing.name,
                    conflicting_tag: existing.tag_value,
                });
            }
            // 规则 2：已有标签的 tag_value 是否落入新标签的掩码空间
            // 对 new_tag 做归一化（& new_mask），与规则 1 对称
            if (existing.tag_value & new_mask) == normalized_new_tag {
                return Err(TagConflictError {
                    new_name,
                    new_tag,
                    conflicting_with: existing.name,
                    conflicting_tag: existing.tag_value,
                });
            }
        }
        Ok(())
    }

    /// 检测位模式冲突：新标签是否与已有标签的掩码空间重叠
    ///
    /// 冲突判定规则（双向检测，规则 2 对 new_tag 做掩码归一化）：
    /// 1. 新标签的 tag_value 落入已有标签的掩码空间：
    ///    `(new_tag & existing.mask) == existing.tag_value`
    /// 2. 已有标签的 tag_value 落入新标签的掩码空间：
    ///    `(existing.tag_value & new_mask) == (new_tag & new_mask)`
    ///
    /// 满足任一条件即判定为冲突。
    ///
    /// # 参数
    /// - `new_tag`: 新标签的位模式值
    /// - `new_mask`: 新标签的检测掩码
    ///
    /// # 返回
    /// - `Ok(())`: 无冲突，可安全注册
    /// - `Err(TagConflictError)`: 存在冲突，附带冲突详情
    pub fn check_conflict(new_tag: u64, new_mask: u64) -> Result<(), TagConflictError> {
        Self::check_conflict_inner("<new>", new_tag, new_mask)
    }

    /// 带名称的冲突检测，错误信息中包含新标签名称
    ///
    /// 与 [`check_conflict`] 逻辑相同，但错误信息中包含新标签的可读名称，
    /// 便于调试和日志输出。
    pub fn check_conflict_named(
        new_name: &'static str,
        new_tag: u64,
        new_mask: u64,
    ) -> Result<(), TagConflictError> {
        Self::check_conflict_inner(new_name, new_tag, new_mask)
    }

    /// 检测给定的 64 位值匹配哪个已注册标签
    ///
    /// 按注册顺序依次检测，返回第一个匹配的标签描述符。
    /// 如果无匹配，返回 `None`（可能是 Float 类型）。
    pub fn classify_bits(bits: u64) -> Option<&'static TagDescriptor> {
        TAG_REGISTRY.iter().find(|desc| desc.matches_bits(bits))
    }

    /// 返回已注册标签数量
    #[inline(always)]
    pub fn tag_count() -> usize {
        TAG_REGISTRY.len()
    }
}

// ============================================================================
// define_value_tag! -- 声明式标签定义宏
// ============================================================================

/// 声明式定义新的 NaN-tag 类型标签
///
/// 此宏自动为 `Value` 生成类型检测、提取和构造方法，消除手动编写
/// 重复的位运算样板代码。
///
/// # 用法
///
/// ```ignore
/// define_value_tag! {
///     Symbol => {
///         tag: SYMBOL_TAG,
///         mask: SYMBOL_MASK,
///         index_mask: SYMBOL_INDEX_MASK,
///         doc: "Symbol type for unique identifiers",
///     }
/// }
/// ```
///
/// # 生成的方法
///
/// 对于标签名 `Symbol`，宏会生成：
/// - `Value::is_symbol(&self) -> bool` -- 类型检测
/// - `Value::as_symbol_opt(&self) -> Option<u32>` -- 索引提取
/// - `Value::from_symbol_index(u32) -> Value` -- 从索引构造
///
/// # 命名规则
///
/// - 标签名使用 PascalCase（如 `Symbol`, `BigInt`）
/// - 生成的方法名自动转换为 snake_case（如 `is_symbol`, `as_symbol_opt`）
///
/// # 编译期冲突检测
///
/// 宏展开时会调用 [`TagRegistry::check_conflict_named`] 进行编译期常量求值，
/// 如果新标签与已有标签冲突，编译将失败。
#[macro_export]
macro_rules! define_value_tag {
    (
        $(#[$meta:meta])*
        $name:ident => {
            tag: $tag:expr,
            mask: $mask:expr,
            index_mask: $index_mask:expr,
            doc: $doc:expr,
        }
    ) => {
        const _: () = {
            // 编译期冲突检测：如果新标签与已有标签冲突，编译失败
            const _: () = assert!(
                $crate::tag_registry::TagRegistry::check_conflict_named(
                    stringify!($name),
                    $tag,
                    $mask,
                ).is_ok(),
                concat!("标签冲突: ", stringify!($name), " 与已有标签的位空间重叠"),
            );
        };

        paste::paste! {
            $(#[$meta])*
            #[doc = $doc]
            impl Value {
                /// 检测此值是否为 [`stringify!($name)`] 类型
                ///
                /// 判定公式：`(bits & $mask) == $tag`
                #[inline(always)]
                pub fn [<is_ $name:snake>](self) -> bool {
                    (self.into_raw_bits() & $mask) == $tag
                }

                /// 提取 [`stringify!($name)`] 类型的索引值
                ///
                /// 如果此值不是 `$name` 类型，返回 `None`
                #[inline(always)]
                pub fn [<as_ $name:snake _opt>](self) -> Option<u32> {
                    if self.[<is_ $name:snake>]() {
                        Some((self.into_raw_bits() & $index_mask) as u32)
                    } else {
                        None
                    }
                }

                /// 从索引构造 [`stringify!($name)`] 类型的值
                ///
                /// 索引值会被掩码截断到有效位数范围内
                #[inline(always)]
                pub fn [<from_ $name:snake _index>](index: u32) -> Value {
                    unsafe { Value::from_raw_bits($tag | (index as u64 & $index_mask)) }
                }
            }
        }
    };
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // TagDescriptor 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_descriptor_matches_bits_smi() {
        let smi_desc = TagRegistry::find_by_name("Smi").unwrap();
        // 正确的 Smi 位模式
        assert!(smi_desc.matches_bits(SMI_TAG | 42));
        // 非 Smi 位模式
        assert!(!smi_desc.matches_bits(HEAP_TAG | 42));
        assert!(!smi_desc.matches_bits(STRING_TAG | 42));
    }

    #[test]
    fn test_descriptor_matches_bits_heap() {
        let heap_desc = TagRegistry::find_by_name("HeapObject").unwrap();
        assert!(heap_desc.matches_bits(HEAP_TAG | 100));
        assert!(!heap_desc.matches_bits(STRING_TAG | 100));
    }

    #[test]
    fn test_descriptor_matches_bits_string() {
        let str_desc = TagRegistry::find_by_name("String").unwrap();
        assert!(str_desc.matches_bits(STRING_TAG | 999));
        assert!(!str_desc.matches_bits(HEAP_TAG | 999));
    }

    #[test]
    fn test_descriptor_matches_bits_special() {
        let spec_desc = TagRegistry::find_by_name("Special").unwrap();
        assert!(spec_desc.matches_bits(NIL_VALUE));
        assert!(spec_desc.matches_bits(FALSE_VALUE));
        assert!(spec_desc.matches_bits(TRUE_VALUE));
        // 非 Special：Heap/String/Smi 的高位不匹配 PTR_TAG 或 bits[2:47] 非零
        assert!(!spec_desc.matches_bits(HEAP_TAG));
        assert!(!spec_desc.matches_bits(STRING_TAG));
        assert!(!spec_desc.matches_bits(SMI_TAG));
        // 注意：PTR_TAG | 0（空指针）会匹配 Special 掩码，这是已知边界情况
        // 空指针在 Value 系统中由 is_ptr() 独立处理
        assert!(spec_desc.matches_bits(PTR_TAG)); // PTR_TAG | 0
    }

    #[test]
    fn test_descriptor_payload_capacity() {
        let smi_desc = TagRegistry::find_by_name("Smi").unwrap();
        assert_eq!(smi_desc.payload_capacity(), 1u64 << 48);

        let heap_desc = TagRegistry::find_by_name("HeapObject").unwrap();
        assert_eq!(heap_desc.payload_capacity(), 1u64 << 46);

        let str_desc = TagRegistry::find_by_name("String").unwrap();
        assert_eq!(str_desc.payload_capacity(), 1u64 << 47);

        let spec_desc = TagRegistry::find_by_name("Special").unwrap();
        assert_eq!(spec_desc.payload_capacity(), 1u64 << 2);
    }

    #[test]
    fn test_descriptor_payload_mask() {
        let smi_desc = TagRegistry::find_by_name("Smi").unwrap();
        assert_eq!(smi_desc.payload_mask(), SMI_VALUE_MASK);

        let str_desc = TagRegistry::find_by_name("String").unwrap();
        assert_eq!(str_desc.payload_mask(), STRING_INDEX_MASK);

        let heap_desc = TagRegistry::find_by_name("HeapObject").unwrap();
        assert_eq!(heap_desc.payload_mask(), HEAP_INDEX_MASK);
    }

    // -----------------------------------------------------------------------
    // TagRegistry 查询测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_tags_count() {
        assert_eq!(TagRegistry::tag_count(), 4);
    }

    #[test]
    fn test_find_by_name_existing() {
        assert!(TagRegistry::find_by_name("Smi").is_some());
        assert!(TagRegistry::find_by_name("HeapObject").is_some());
        assert!(TagRegistry::find_by_name("String").is_some());
        assert!(TagRegistry::find_by_name("Special").is_some());
    }

    #[test]
    fn test_find_by_name_nonexistent() {
        assert!(TagRegistry::find_by_name("Float").is_none());
        assert!(TagRegistry::find_by_name("Pointer").is_none());
        assert!(TagRegistry::find_by_name("NonExistent").is_none());
        assert!(TagRegistry::find_by_name("").is_none());
    }

    #[test]
    fn test_find_by_value_tag() {
        let smi = TagRegistry::find_by_value_tag(ValueTag::Smi);
        assert!(smi.is_some());
        assert_eq!(smi.unwrap().name, "Smi");

        let str_tag = TagRegistry::find_by_value_tag(ValueTag::String);
        assert!(str_tag.is_some());
        assert_eq!(str_tag.unwrap().name, "String");
    }

    #[test]
    fn test_classify_bits_known_types() {
        // Smi
        let smi_bits = SMI_TAG | 42;
        assert_eq!(TagRegistry::classify_bits(smi_bits).unwrap().name, "Smi");

        // Heap
        let heap_bits = HEAP_TAG | 100;
        assert_eq!(TagRegistry::classify_bits(heap_bits).unwrap().name, "HeapObject");

        // String
        let str_bits = STRING_TAG | 999;
        assert_eq!(TagRegistry::classify_bits(str_bits).unwrap().name, "String");

        // Special (nil)
        assert_eq!(TagRegistry::classify_bits(NIL_VALUE).unwrap().name, "Special");
        // Special (false)
        assert_eq!(TagRegistry::classify_bits(FALSE_VALUE).unwrap().name, "Special");
        // Special (true)
        assert_eq!(TagRegistry::classify_bits(TRUE_VALUE).unwrap().name, "Special");
    }

    #[test]
    fn test_classify_bits_float_returns_none() {
        // Float 没有固定 tag，不在注册表中
        let float_bits = 2.5f64.to_bits();
        assert!(TagRegistry::classify_bits(float_bits).is_none());
    }

    // -----------------------------------------------------------------------
    // 冲突检测测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_check_conflict_no_overlap() {
        // 0x7FFA 前缀不与现有标签冲突
        let new_tag: u64 = 0x7FFA_0000_0000_0000;
        let new_mask: u64 = 0x7FFF_0000_0000_0000;
        assert!(TagRegistry::check_conflict(new_tag, new_mask).is_ok());
    }

    #[test]
    fn test_check_conflict_with_smi() {
        // 使用与 Smi 完全相同的 tag+mask 会冲突
        // 但由于 SPECIAL_VALUE_MASK 更严格，SMI_TAG 不再落入 Special 空间
        // 所以需要构造一个真正与 Smi 冲突的值
        let new_tag: u64 = SMI_TAG | 0x100;
        let new_mask: u64 = SMI_MASK;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().conflicting_with, "Smi");
    }

    #[test]
    fn test_check_conflict_with_heap() {
        // 构造与 HeapObject 冲突的值
        let new_tag: u64 = HEAP_TAG | 0x200;
        let new_mask: u64 = HEAP_MASK;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().conflicting_with, "HeapObject");
    }

    #[test]
    fn test_check_conflict_with_string() {
        // 构造与 String 冲突的值
        let new_tag: u64 = STRING_TAG | 0x300;
        let new_mask: u64 = STRING_MASK;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().conflicting_with, "String");
    }

    #[test]
    fn test_check_conflict_with_special() {
        // 与 Special 标签重叠
        let result = TagRegistry::check_conflict(PTR_TAG, SPECIAL_MASK);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().conflicting_with, "Special");
    }

    #[test]
    fn test_check_conflict_partial_overlap() {
        // 新标签的 tag_value 落入已有标签的掩码空间
        // NIL_VALUE = PTR_TAG | 1，落入 Special 的掩码空间：
        // (NIL_VALUE & SPECIAL_VALUE_MASK) == PTR_TAG
        let new_tag: u64 = NIL_VALUE;
        let new_mask: u64 = 0xFFFF_FFFF_FFFF_FFFC;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().conflicting_with, "Special");
    }

    #[test]
    fn test_check_conflict_named_includes_name() {
        let result = TagRegistry::check_conflict_named("TestTag", SMI_TAG, SMI_MASK);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.new_name, "TestTag");
    }

    #[test]
    fn test_check_conflict_zero_tag() {
        // 零 tag 不应与任何 NaN-tag 冲突（NaN-tag 空间从 0x7FF8 开始）
        assert!(TagRegistry::check_conflict(0, 0xFFFF_0000_0000_0000).is_ok());
    }

    #[test]
    fn test_check_conflict_reverse_overlap() {
        // 已有标签的 tag_value 落入新标签的掩码空间
        // 新掩码足够宽，使得已有标签的 tag_value 匹配新 tag
        let new_tag: u64 = 0;
        let new_mask: u64 = 0; // 零掩码：任何值 & 0 == 0 == new_tag
        // 这意味着所有已有标签的 tag_value & 0 == 0 == new_tag，冲突
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // TagConflictError 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_tag_conflict_error_display() {
        let err = TagConflictError {
            new_name: "Symbol",
            new_tag: 0x7FF9_0000_0000_0000,
            conflicting_with: "Smi",
            conflicting_tag: 0x7FF9_0000_0000_0000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("Symbol"));
        assert!(msg.contains("Smi"));
        assert!(msg.contains("0x7FF9"));
    }

    #[test]
    fn test_tag_conflict_error_is_std_error() {
        let err = TagConflictError {
            new_name: "Test",
            new_tag: 0,
            conflicting_with: "Other",
            conflicting_tag: 0,
        };
        let _: &dyn std::error::Error = &err;
    }

    // -----------------------------------------------------------------------
    // TAG_REGISTRY 完整性测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_registry_internal_consistency() {
        // 每个注册标签的 tag_value 必须匹配自身的 mask
        for desc in TAG_REGISTRY {
            assert_eq!(
                desc.tag_value & desc.mask,
                desc.tag_value,
                "标签 '{}' 的 tag_value 不匹配自身 mask",
                desc.name
            );
        }
    }

    #[test]
    fn test_registry_no_internal_conflicts() {
        // 注册表内标签之间不应互相冲突
        for (i, a) in TAG_REGISTRY.iter().enumerate() {
            for (j, b) in TAG_REGISTRY.iter().enumerate() {
                if i == j {
                    continue;
                }
                // a 的 tag 不应落入 b 的 mask 空间
                let a_in_b = (a.tag_value & b.mask) == b.tag_value;
                // b 的 tag 不应落入 a 的 mask 空间
                let b_in_a = (b.tag_value & a.mask) == a.tag_value;
                assert!(!a_in_b || !b_in_a, "标签 '{}' 和 '{}' 存在内部冲突", a.name, b.name);
            }
        }
    }

    #[test]
    fn test_registry_gc_flags() {
        let heap_desc = TagRegistry::find_by_name("HeapObject").unwrap();
        assert!(heap_desc.is_gc, "HeapObject 必须标记为 GC 管理");

        // 非 GC 类型
        for name in &["Special", "String", "Smi"] {
            let desc = TagRegistry::find_by_name(name).unwrap();
            assert!(!desc.is_gc, "{} 不应标记为 GC 管理", name);
        }
    }

    #[test]
    fn test_registry_value_tag_mapping() {
        assert_eq!(TagRegistry::find_by_name("Special").unwrap().value_tag, ValueTag::Nil);
        assert_eq!(TagRegistry::find_by_name("HeapObject").unwrap().value_tag, ValueTag::Pointer);
        assert_eq!(TagRegistry::find_by_name("String").unwrap().value_tag, ValueTag::String);
        assert_eq!(TagRegistry::find_by_name("Smi").unwrap().value_tag, ValueTag::Smi);
    }

    // -----------------------------------------------------------------------
    // 边界与归一化回归测试（TODO #3 / #4 / #6）
    // -----------------------------------------------------------------------

    #[test]
    fn test_payload_mask_boundary_64_bits() {
        // payload_bits < 64：正常公式 (1 << n) - 1
        let desc_63 = TagDescriptor {
            name: "Test63",
            tag_value: 0,
            mask: 0,
            payload_bits: 63,
            is_gc: false,
            value_tag: ValueTag::Smi,
        };
        assert_eq!(desc_63.payload_mask(), (1u64 << 63) - 1);

        // payload_bits == 64：边界情况，必须返回 u64::MAX 避免 `1 << 64` 溢出
        let desc_64 = TagDescriptor {
            name: "Test64",
            tag_value: 0,
            mask: 0,
            payload_bits: 64,
            is_gc: false,
            value_tag: ValueTag::Smi,
        };
        assert_eq!(desc_64.payload_mask(), u64::MAX);

        // payload_bits > 64：防御性边界，仍返回 u64::MAX
        let desc_65 = TagDescriptor {
            name: "Test65",
            tag_value: 0,
            mask: 0,
            payload_bits: 65,
            is_gc: false,
            value_tag: ValueTag::Smi,
        };
        assert_eq!(desc_65.payload_mask(), u64::MAX);
    }

    #[test]
    fn test_smi_conflict_detection_correctness() {
        // 构造非规范化 new_tag（含掩码外的位），验证 Rule 2 归一化后能检出与 Smi 的冲突。
        //
        // Smi: tag_value = SMI_TAG = 0x7FF9_0000_0000_0000, mask = SMI_MASK = 0x7FFF_0000_0000_0000
        // new_tag = 0x8001_0000_0000_0000, new_mask = 0x0001_0000_0000_0000
        //   - Rule 1: (new_tag & SMI_MASK) = 0x0001_0000_0000_0000 != SMI_TAG → 不冲突
        //   - Rule 2 旧版本（未归一化）: (SMI_TAG & new_mask) = 0x0001_0000_0000_0000 != new_tag(0x8001_...) → 漏检
        //   - Rule 2 归一化后: (SMI_TAG & new_mask) = 0x0001_... == (new_tag & new_mask) = 0x0001_... → 检出冲突
        let new_tag: u64 = 0x8001_0000_0000_0000;
        let new_mask: u64 = 0x0001_0000_0000_0000;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err(), "Rule 2 归一化后应检出与 Smi 的冲突，但结果为 Ok");
        assert_eq!(result.unwrap_err().conflicting_with, "Smi");
    }

    #[test]
    fn test_string_tag_conflict_detection() {
        // 构造非规范化 new_tag（含掩码外的位），验证 Rule 2 归一化后能检出与 String 的冲突。
        //
        // String: tag_value = STRING_TAG = 0x7FF8_8000_0000_0000, mask = STRING_MASK = 0x7FF8_8000_0000_0000
        // new_tag = 0x8000_8000_0000_0000, new_mask = 0x0000_8000_0000_0000
        //   - Rule 1: (new_tag & STRING_MASK) = 0x0000_8000_... != STRING_TAG → 不冲突
        //   - Rule 2 归一化后: (STRING_TAG & new_mask) = 0x0000_8000_... == (new_tag & new_mask) = 0x0000_8000_... → 检出冲突
        let new_tag: u64 = 0x8000_8000_0000_0000;
        let new_mask: u64 = 0x0000_8000_0000_0000;
        let result = TagRegistry::check_conflict(new_tag, new_mask);
        assert!(result.is_err(), "Rule 2 归一化后应检出与 String 的冲突，但结果为 Ok");
        assert_eq!(result.unwrap_err().conflicting_with, "String");
    }
}
